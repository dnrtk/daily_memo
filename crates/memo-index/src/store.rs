//! インデックスの永続化(`<作業フォルダ>/.memo/index.sqlite`)。
//!
//! - `files`      : ファイルごとのサイズ・更新時刻・ハッシュ(差分検出用)
//! - `chunks`     : チャンク本文と位置情報、埋め込み済みフラグ
//! - `chunks_fts` : FTS5(バイグラム化したトークン列、contentless)
//! - `vec_chunks` : sqlite-vec(コサイン距離)。次元はモデルが決まった時点で作る
//!
//! WAL モードなので、インデクサが書いている間も検索用の別接続から読める。

use std::{collections::HashMap, path::Path, sync::Once};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{chunk::Chunk, tokenize, Result};

const SCHEMA_VERSION: i64 = 1;

static VEC_INIT: Once = Once::new();

fn register_sqlite_vec() {
    VEC_INIT.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileState {
    pub id: i64,
    pub size: i64,
    pub mtime: i64,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkRow {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub mtime: i64,
    pub label: String,
    pub line: Option<u32>,
    pub page: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Stats {
    pub files: i64,
    pub chunks: i64,
    pub pending_embeddings: i64,
    pub errors: i64,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        register_sqlite_vec();
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")?;
        let s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        register_sqlite_vec();
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        // 互換性のない旧スキーマは作り直す(インデックスは再生成できる)
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS vec_chunks; DROP TABLE IF EXISTS chunks_fts;
             DROP TABLE IF EXISTS chunks; DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS meta;",
        )?;
        self.conn.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE files(
                 id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE,
                 kind TEXT NOT NULL,
                 size INTEGER NOT NULL,
                 mtime INTEGER NOT NULL,
                 hash TEXT,
                 error TEXT,
                 indexed_at INTEGER NOT NULL
             );
             CREATE TABLE chunks(
                 id INTEGER PRIMARY KEY,
                 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 ord INTEGER NOT NULL,
                 label TEXT NOT NULL,
                 line INTEGER,
                 page INTEGER,
                 text TEXT NOT NULL,
                 embedded INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX chunks_file ON chunks(file_id);
             CREATE INDEX chunks_pending ON chunks(embedded) WHERE embedded = 0;
             CREATE VIRTUAL TABLE chunks_fts USING fts5(
                 tokens, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 0'
             );",
        )?;
        self.conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(())
    }

    // ---- meta --------------------------------------------------------------------

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- files -------------------------------------------------------------------

    pub fn file_states(&self) -> Result<HashMap<String, FileState>> {
        let mut stmt = self.conn.prepare("SELECT path, id, size, mtime, hash FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, FileState { id: r.get(1)?, size: r.get(2)?, mtime: r.get(3)?, hash: r.get(4)? }))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn file_state(&self, path: &str) -> Result<Option<FileState>> {
        Ok(self
            .conn
            .query_row("SELECT id, size, mtime, hash FROM files WHERE path = ?1", [path], |r| {
                Ok(FileState { id: r.get(0)?, size: r.get(1)?, mtime: r.get(2)?, hash: r.get(3)? })
            })
            .optional()?)
    }

    /// 内容は同じでサイズ・時刻だけ変わった場合
    pub fn touch_file(&self, id: i64, size: i64, mtime: i64) -> Result<()> {
        self.conn.execute("UPDATE files SET size = ?2, mtime = ?3 WHERE id = ?1", params![id, size, mtime])?;
        Ok(())
    }

    /// ファイルのチャンクを丸ごと置き換える(1 トランザクション)
    #[allow(clippy::too_many_arguments)]
    pub fn replace_file(
        &mut self,
        path: &str,
        kind: &str,
        size: i64,
        mtime: i64,
        hash: &str,
        error: Option<&str>,
        chunks: &[Chunk],
    ) -> Result<()> {
        let has_vec = self.vec_dim()?.is_some();
        let tx = self.conn.transaction()?;
        let old: Option<i64> = tx.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        if let Some(id) = old {
            delete_chunks(&tx, id, has_vec)?;
            tx.execute(
                "UPDATE files SET kind=?2, size=?3, mtime=?4, hash=?5, error=?6, indexed_at=strftime('%s','now') WHERE id=?1",
                params![id, kind, size, mtime, hash, error],
            )?;
        } else {
            tx.execute(
                "INSERT INTO files(path, kind, size, mtime, hash, error, indexed_at) VALUES (?1,?2,?3,?4,?5,?6,strftime('%s','now'))",
                params![path, kind, size, mtime, hash, error],
            )?;
        }
        let file_id: i64 = tx.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))?;
        {
            let mut ins = tx.prepare("INSERT INTO chunks(file_id, ord, label, line, page, text) VALUES (?1,?2,?3,?4,?5,?6)")?;
            let mut fts = tx.prepare("INSERT INTO chunks_fts(rowid, tokens) VALUES (?1, ?2)")?;
            let name = path.rsplit('/').next().unwrap_or(path);
            for c in chunks {
                ins.execute(params![file_id, c.ord, c.loc.label, c.loc.line, c.loc.page, c.text])?;
                let id = tx.last_insert_rowid();
                // ファイル名と位置ラベルも検索対象に含める
                let tokens = tokenize::index_tokens(&format!("{name} {} {}", c.loc.label, c.text));
                fts.execute(params![id, tokens])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_file(&mut self, path: &str) -> Result<bool> {
        let has_vec = self.vec_dim()?.is_some();
        let tx = self.conn.transaction()?;
        let id: Option<i64> = tx.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0)).optional()?;
        if let Some(id) = id {
            delete_chunks(&tx, id, has_vec)?;
            tx.execute("DELETE FROM files WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(id.is_some())
    }

    // ---- 埋め込み -------------------------------------------------------------------

    pub fn vec_dim(&self) -> Result<Option<usize>> {
        Ok(self.meta("embed_dim")?.and_then(|s| s.parse().ok()))
    }

    pub fn ensure_vec_table(&self, dim: usize) -> Result<()> {
        if self.vec_dim()?.is_none() {
            self.conn.execute_batch(&format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(embedding float[{dim}] distance_metric=cosine);"
            ))?;
            self.set_meta("embed_dim", &dim.to_string())?;
        }
        Ok(())
    }

    /// 埋め込みを全部捨てて、指定モデルで作り直す状態にする
    pub fn reset_vectors(&self, model_key: &str) -> Result<()> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS vec_chunks; UPDATE chunks SET embedded = 0; DELETE FROM meta WHERE key = 'embed_dim';",
        )?;
        self.set_meta("embed_model", model_key)
    }

    /// 埋め込み待ちのチャンク(id, 埋め込み用テキスト)
    pub fn pending_chunks(&self, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, f.path, c.label, c.text FROM chunks c JOIN files f ON f.id = c.file_id
             WHERE c.embedded = 0 ORDER BY c.id LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            let path: String = r.get(1)?;
            let label: String = r.get(2)?;
            let text: String = r.get(3)?;
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            // 文脈としてファイル名と位置ラベルを前置き。長すぎる本文は切る
            let body: String = text.chars().take(2000).collect();
            Ok((r.get(0)?, format!("{name} {label}\n{body}")))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn set_embeddings(&mut self, items: &[(i64, Vec<f32>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM vec_chunks WHERE rowid = ?1")?;
            let mut ins = tx.prepare("INSERT INTO vec_chunks(rowid, embedding) VALUES (?1, ?2)")?;
            let mut mark = tx.prepare("UPDATE chunks SET embedded = 1 WHERE id = ?1")?;
            for (id, v) in items {
                del.execute([id])?;
                ins.execute(params![id, f32_bytes(v)])?;
                mark.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- 検索 ----------------------------------------------------------------------

    /// BM25 の上位(rank は小さいほど良い)
    pub fn fts_search(&self, expr: &str, limit: usize) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![expr, limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// コサイン距離の近い順
    pub fn vec_search(&self, query: &[f32], k: usize) -> Result<Vec<(i64, f64)>> {
        if self.vec_dim()?.is_none() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT rowid, distance FROM vec_chunks WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
        )?;
        let rows = stmt.query_map(params![f32_bytes(query), k as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn chunk_rows(&self, ids: &[i64]) -> Result<HashMap<i64, ChunkRow>> {
        let mut out = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT c.id, f.path, f.kind, f.mtime, c.label, c.line, c.page, c.text
             FROM chunks c JOIN files f ON f.id = c.file_id WHERE c.id = ?1",
        )?;
        for id in ids {
            if let Some(row) = stmt
                .query_row([id], |r| {
                    Ok(ChunkRow {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        kind: r.get(2)?,
                        mtime: r.get(3)?,
                        label: r.get(4)?,
                        line: r.get(5)?,
                        page: r.get(6)?,
                        text: r.get(7)?,
                    })
                })
                .optional()?
            {
                out.insert(*id, row);
            }
        }
        Ok(out)
    }

    /// ファイルの全チャンクを並び順で返す(閲覧用)
    pub fn file_chunks(&self, path: &str) -> Result<Vec<ChunkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, f.path, f.kind, f.mtime, c.label, c.line, c.page, c.text
             FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.path = ?1 ORDER BY c.ord",
        )?;
        let rows = stmt.query_map([path], |r| {
            Ok(ChunkRow {
                id: r.get(0)?,
                path: r.get(1)?,
                kind: r.get(2)?,
                mtime: r.get(3)?,
                label: r.get(4)?,
                line: r.get(5)?,
                page: r.get(6)?,
                text: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn stats(&self) -> Result<Stats> {
        Ok(self.conn.query_row(
            "SELECT (SELECT count(*) FROM files WHERE error IS NULL),
                    (SELECT count(*) FROM chunks),
                    (SELECT count(*) FROM chunks WHERE embedded = 0),
                    (SELECT count(*) FROM files WHERE error IS NOT NULL)",
            [],
            |r| Ok(Stats { files: r.get(0)?, chunks: r.get(1)?, pending_embeddings: r.get(2)?, errors: r.get(3)? }),
        )?)
    }

    /// 抽出に失敗したファイル(状態表示用)
    pub fn errors(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT path, error FROM files WHERE error IS NOT NULL ORDER BY path LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

fn delete_chunks(tx: &rusqlite::Transaction, file_id: i64, has_vec: bool) -> Result<()> {
    tx.execute("DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE file_id = ?1)", [file_id])?;
    if has_vec {
        tx.execute("DELETE FROM vec_chunks WHERE rowid IN (SELECT id FROM chunks WHERE file_id = ?1)", [file_id])?;
    }
    tx.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
    Ok(())
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Loc;

    fn chunk(text: &str) -> Chunk {
        Chunk { ord: 0, text: text.into(), loc: Loc { label: "L".into(), line: Some(1), page: None } }
    }

    #[test]
    fn fts_bigram_roundtrip_and_delete() {
        let mut s = Store::open_in_memory().unwrap();
        s.replace_file("a.md", "markdown", 1, 1, "h", None, &[chunk("今日の会議議事録をまとめた")]).unwrap();
        assert_eq!(s.file_chunks("a.md").unwrap()[0].text, "今日の会議議事録をまとめた");
        assert!(s.file_chunks("none.md").unwrap().is_empty());
        let q = tokenize::query_expr(&["議事録"]).unwrap();
        assert_eq!(s.fts_search(&q, 10).unwrap().len(), 1);
        // 置き換えると古いチャンクは消える
        s.replace_file("a.md", "markdown", 2, 2, "h2", None, &[chunk("別の内容")]).unwrap();
        assert!(s.fts_search(&q, 10).unwrap().is_empty());
        assert_eq!(s.stats().unwrap().chunks, 1);
        s.delete_file("a.md").unwrap();
        assert_eq!(s.stats().unwrap().files, 0);
        assert_eq!(s.stats().unwrap().chunks, 0);
    }

    #[test]
    fn vectors_knn_and_reset() {
        let mut s = Store::open_in_memory().unwrap();
        s.replace_file("a.md", "markdown", 1, 1, "h", None, &[chunk("a"), chunk("b")]).unwrap();
        let pending = s.pending_chunks(10).unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending[0].1.starts_with("a.md L\n"));
        s.ensure_vec_table(3).unwrap();
        s.set_embeddings(&[(pending[0].0, vec![1.0, 0.0, 0.0]), (pending[1].0, vec![0.0, 1.0, 0.0])]).unwrap();
        assert_eq!(s.stats().unwrap().pending_embeddings, 0);
        let hits = s.vec_search(&[0.9, 0.1, 0.0], 2).unwrap();
        assert_eq!(hits[0].0, pending[0].0);
        assert!(hits[0].1 < hits[1].1);
        // ファイルの置き換えでベクトルも消える
        s.replace_file("a.md", "markdown", 2, 2, "h2", None, &[chunk("c")]).unwrap();
        assert!(s.vec_search(&[1.0, 0.0, 0.0], 5).unwrap().is_empty());
        s.reset_vectors("m2").unwrap();
        assert_eq!(s.vec_dim().unwrap(), None);
        assert_eq!(s.meta("embed_model").unwrap().as_deref(), Some("m2"));
    }
}
