//! 埋め込みの抽象化。実運用は設定の埋め込みモデル(Ollama / OpenAI 互換)、テストは偽実装。

use memo_core::settings::{EmbeddingRef, Provider};

use crate::{IndexError, Result};

pub trait Embedder: Send + Sync {
    /// インデックスに記録するモデル識別子。これが変わったらベクトルは作り直し
    fn model_key(&self) -> String;
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}

/// 設定に従って LLM サーバーで埋め込みを作る
pub struct ProviderEmbedder {
    pub provider: Provider,
    pub cfg: EmbeddingRef,
}

impl Embedder for ProviderEmbedder {
    fn model_key(&self) -> String {
        // 接続先が変わっても同じモデルなら互換とみなす。prefix は結果を変えるので含める
        format!("{}|{}", self.cfg.model, self.cfg.document_prefix)
    }

    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let inputs: Vec<String> = texts
            .iter()
            .map(|t| format!("{}{t}", self.cfg.document_prefix))
            .collect();
        memo_core::llm::embed(&self.provider, &self.cfg.model, &inputs)
            .map_err(|e| IndexError::Embed(e.to_string()))
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let input = vec![format!("{}{text}", self.cfg.query_prefix)];
        memo_core::llm::embed(&self.provider, &self.cfg.model, &input)
            .map_err(|e| IndexError::Embed(e.to_string()))?
            .pop()
            .ok_or_else(|| IndexError::Embed("空の応答".into()))
    }
}

/// テスト用: トークンのハッシュを足し合わせた正規化ベクトル。
/// 語の重なりが多いほど近くなるので、検索パイプラインの検証に使える。
pub struct HashEmbedder {
    pub dim: usize,
    pub key: String,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim, key: "hash-test".into() }
    }

    fn vec(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for tok in crate::tokenize::index_tokens(text).split(' ').filter(|t| !t.is_empty()) {
            let h = blake3::hash(tok.as_bytes());
            let b = h.as_bytes();
            let idx = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize % self.dim;
            v[idx] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }
}

impl Embedder for HashEmbedder {
    fn model_key(&self) -> String {
        self.key.clone()
    }
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.vec(t)).collect())
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.vec(text))
    }
}
