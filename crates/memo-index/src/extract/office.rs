//! Word / Excel / PowerPoint
//!
//! docx / pptx は ZIP + XML なので必要な部分だけを quick-xml で読む。
//! Excel は calamine(xlsx / xlsm / xls / ods)。

use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, Read},
    path::Path,
};

use calamine::{open_workbook_auto, Data, Reader as _};
use quick_xml::{events::Event, Reader, XmlVersion};
use zip::ZipArchive;

use super::{Loc, Segment};
use crate::{IndexError, Result};

type Zip = ZipArchive<BufReader<File>>;

fn open_zip(path: &Path) -> Result<Zip> {
    Ok(ZipArchive::new(BufReader::new(File::open(path)?))?)
}

fn read_entry(zip: &mut Zip, name: &str) -> Result<Option<String>> {
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(Some(s))
}

fn attr(e: &quick_xml::events::BytesStart, local: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == local)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// OOXML の段落を順に取り出す汎用ループ。
/// `para` は段落要素名(docx: `p` / pptx: `p`)、`text` はテキスト要素名(`t`)。
/// 戻り値は (段落スタイル ID, テキスト) の列。
fn paragraphs(xml: &str, style_tag: Option<&str>) -> Result<Vec<(Option<String>, String)>> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut style: Option<String> = None;
    let mut in_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(e) => match e.local_name().as_ref() {
                "t" => in_text = true,
                "p" => {
                    cur.clear();
                    style = None;
                }
                "tab" => cur.push('\t'),
                name if Some(name) == style_tag => style = attr(&e, "val"),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" => cur.push('\t'),
                "br" | "cr" => cur.push('\n'),
                name if Some(name) == style_tag => style = attr(&e, "val"),
                _ => {}
            },
            Event::Text(t) if in_text => cur.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => {
                let name = r.xml10_content();
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    cur.push(c);
                } else if let Some(c) = entity(&name) {
                    cur.push(c);
                }
            }
            Event::CData(t) if in_text => cur.push_str(&t),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = false,
                "p" => out.push((style.take(), std::mem::take(&mut cur))),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

// ---- Word ----------------------------------------------------------------------

/// styles.xml から「見出し扱いのスタイル ID → 見出しレベル」を作る。
/// 日本語版 Word は styleId が "1" "2" のような数字になるため、名前(heading 1 等)で判定する。
fn heading_styles(styles_xml: &str) -> Result<HashMap<String, u8>> {
    let mut reader = Reader::from_str(styles_xml);
    let mut map = HashMap::new();
    let mut cur_id: Option<String> = None;
    loop {
        match reader.read_event()? {
            Event::Start(e) if e.local_name().as_ref() == "style" => cur_id = attr(&e, "styleId"),
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == "name" => {
                if let (Some(id), Some(name)) = (&cur_id, attr(&e, "val")) {
                    if let Some(level) = heading_level(&name) {
                        map.insert(id.clone(), level);
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref() == "style" => cur_id = None,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(map)
}

fn heading_level(style_name: &str) -> Option<u8> {
    let n = style_name.to_lowercase();
    if n == "title" || n == "表題" {
        return Some(0);
    }
    let rest = n.strip_prefix("heading").or_else(|| n.strip_prefix("見出し"))?;
    rest.trim().parse::<u8>().ok()
}

pub fn docx(path: &Path) -> Result<Vec<Segment>> {
    let mut zip = open_zip(path)?;
    let body = read_entry(&mut zip, "word/document.xml")?
        .ok_or_else(|| IndexError::Extract("word/document.xml がありません".into()))?;
    let styles = match read_entry(&mut zip, "word/styles.xml")? {
        Some(s) => heading_styles(&s)?,
        None => HashMap::new(),
    };
    let level_of = |id: &str| styles.get(id).copied().or_else(|| heading_level(id));

    let mut out = Vec::new();
    let mut stack: Vec<(u8, String)> = Vec::new();
    let mut buf = String::new();
    let label = |stack: &[(u8, String)]| stack.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(" > ");

    // 本文を含まない(見出しだけの)まとまりは次の節に持ち越して、見出しだけのチャンクを作らない
    let mut body_seen = false;
    for (style, text) in paragraphs(&body, Some("pStyle"))? {
        let heading = style.as_deref().and_then(level_of);
        if let Some(level) = heading {
            if body_seen {
                out.push(Segment::new(std::mem::take(&mut buf), label(&stack)));
                body_seen = false;
            }
            stack.retain(|(l, _)| *l < level);
            stack.push((level, text.trim().to_string()));
        }
        if !text.trim().is_empty() {
            buf.push_str(&text);
            buf.push('\n');
            body_seen |= heading.is_none();
        }
    }
    if !buf.trim().is_empty() {
        out.push(Segment::new(buf, label(&stack)));
    }
    Ok(out)
}

// ---- PowerPoint ----------------------------------------------------------------

fn rels(zip: &mut Zip, rels_path: &str) -> Result<Vec<(String, String, String)>> {
    let Some(xml) = read_entry(zip, rels_path)? else { return Ok(vec![]) };
    let mut reader = Reader::from_str(&xml);
    let mut out = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == "Relationship" => {
                if let (Some(id), Some(ty), Some(target)) = (attr(&e, "Id"), attr(&e, "Type"), attr(&e, "Target")) {
                    out.push((id, ty, target));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// `ppt/slides/../notesSlides/x.xml` → `ppt/notesSlides/x.xml`
fn resolve(base_dir: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// プレゼンテーション上の表示順でスライドのパスを返す
fn slide_order(zip: &mut Zip) -> Result<Vec<String>> {
    let pres = read_entry(zip, "ppt/presentation.xml")?.unwrap_or_default();
    let rel_map: HashMap<String, String> = rels(zip, "ppt/_rels/presentation.xml.rels")?
        .into_iter()
        .filter(|(_, ty, _)| ty.ends_with("/slide"))
        .map(|(id, _, t)| (id, resolve("ppt", &t)))
        .collect();
    let mut reader = Reader::from_str(&pres);
    let mut order = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) | Event::Start(e) if e.local_name().as_ref() == "sldId" => {
                if let Some(path) = attr(&e, "id").and_then(|id| rel_map.get(&id)) {
                    order.push(path.clone());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if order.is_empty() {
        // presentation.xml が読めない場合はファイル名の番号順
        let mut names: Vec<String> = zip
            .file_names()
            .filter_map(|n| n.ok().map(|n| n.into_owned()))
            .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
            .collect();
        names.sort_by_key(|n| n.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml").parse::<u32>().unwrap_or(u32::MAX));
        order = names;
    }
    Ok(order)
}

pub fn pptx(path: &Path) -> Result<Vec<Segment>> {
    let mut zip = open_zip(path)?;
    let mut out = Vec::new();
    for (i, slide_path) in slide_order(&mut zip)?.into_iter().enumerate() {
        let n = i as u32 + 1;
        let Some(xml) = read_entry(&mut zip, &slide_path)? else { continue };
        let paras: Vec<String> = paragraphs(&xml, None)?
            .into_iter()
            .map(|(_, t)| t)
            .filter(|t| !t.trim().is_empty())
            .collect();
        let title = paras.first().cloned().unwrap_or_default();
        let mut text = paras.join("\n");

        // スピーカーノート
        let (dir, file) = slide_path.rsplit_once('/').unwrap_or(("", &slide_path));
        let rel_path = format!("{dir}/_rels/{file}.rels");
        if let Some((_, _, target)) = rels(&mut zip, &rel_path)?.into_iter().find(|(_, ty, _)| ty.ends_with("/notesSlide")) {
            if let Some(notes_xml) = read_entry(&mut zip, &resolve(dir, &target))? {
                let notes: Vec<String> = paragraphs(&notes_xml, None)?
                    .into_iter()
                    .map(|(_, t)| t)
                    // ノートのスライド番号プレースホルダ(数字だけの段落)は除く
                    .filter(|t| !t.trim().is_empty() && !t.trim().chars().all(|c| c.is_ascii_digit()))
                    .collect();
                if !notes.is_empty() {
                    text.push_str("\n[ノート]\n");
                    text.push_str(&notes.join("\n"));
                }
            }
        }
        let label = if title.is_empty() { format!("スライド {n}") } else { format!("スライド {n}: {}", title.trim()) };
        out.push(Segment { text, loc: Loc { label, line: None, page: Some(n) } });
    }
    Ok(out)
}

// ---- Excel ---------------------------------------------------------------------

pub fn excel(path: &Path) -> Result<Vec<Segment>> {
    const BLOCK_CHARS: usize = 700;
    let mut wb = open_workbook_auto(path)?;
    let mut out = Vec::new();
    for sheet in wb.sheet_names().to_vec() {
        let range = match wb.worksheet_range(&sheet) {
            Ok(r) => r,
            Err(_) => continue, // グラフシート等
        };
        let row0 = range.start().map(|(r, _)| r).unwrap_or(0);
        let mut header: Option<String> = None;
        let mut buf = String::new();
        let mut first = 0u32;
        let mut last = 0u32;
        let mut pushed = false;
        for (i, row) in range.rows().enumerate() {
            let cells: Vec<String> = row.iter().map(cell_text).collect();
            if cells.iter().all(|c| c.is_empty()) {
                continue;
            }
            let line = trim_trailing_empty(&cells).join("\t");
            let rowno = row0 + i as u32 + 1;
            if header.is_none() {
                header = Some(line);
                first = rowno;
                last = rowno;
                continue;
            }
            if buf.is_empty() {
                first = rowno;
            }
            last = rowno;
            buf.push_str(&line);
            buf.push('\n');
            if buf.len() >= BLOCK_CHARS {
                out.push(sheet_segment(&sheet, header.as_deref(), &buf, first, last));
                buf.clear();
                pushed = true;
            }
        }
        if !buf.is_empty() || (header.is_some() && !pushed) {
            out.push(sheet_segment(&sheet, header.as_deref(), &buf, first, last));
        }
    }
    Ok(out)
}

fn trim_trailing_empty(cells: &[String]) -> &[String] {
    let end = cells.iter().rposition(|c| !c.is_empty()).map(|i| i + 1).unwrap_or(0);
    &cells[..end]
}

fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::DateTime(dt) if dt.is_datetime() => {
            let (y, m, d, hh, mm, _, _) = dt.to_ymd_hms_milli();
            if hh == 0 && mm == 0 {
                format!("{y:04}-{m:02}-{d:02}")
            } else {
                format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
            }
        }
        other => other.to_string(),
    }
}

fn sheet_segment(sheet: &str, header: Option<&str>, rows: &str, first: u32, last: u32) -> Segment {
    let text = match header {
        Some(h) if !rows.is_empty() => format!("[{sheet}]\n{h}\n{rows}"),
        Some(h) => format!("[{sheet}]\n{h}\n"),
        None => format!("[{sheet}]\n{rows}"),
    };
    let label = if first == last { format!("{sheet} 行{first}") } else { format!("{sheet} 行{first}–{last}") };
    Segment { text, loc: Loc { label, line: None, page: None } }
}

#[cfg(test)]
mod tests {
    use super::super::fixture;
    use super::*;

    #[test]
    fn docx_sections_follow_headings() {
        let segs = docx(&fixture("proposal.docx")).unwrap();
        let labels: Vec<_> = segs.iter().map(|s| s.loc.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "オンプレLLM導入提案書 > 背景",
                "オンプレLLM導入提案書 > 構成案",
                "オンプレLLM導入提案書 > 構成案 > 見積もり"
            ]
        );
        assert!(segs[0].text.starts_with("オンプレLLM導入提案書\n背景\n"), "表題は最初の節に含める");
        assert!(segs[1].text.contains("vLLM"));
        assert!(segs[1].text.contains("GPUサーバー"), "表のセルも拾う");
        assert!(segs[2].text.contains("680 万円 & 保守費"), "実体参照を復元する");
    }

    #[test]
    fn heading_level_names() {
        assert_eq!(heading_level("heading 2"), Some(2));
        assert_eq!(heading_level("見出し 1"), Some(1));
        assert_eq!(heading_level("Title"), Some(0));
        assert_eq!(heading_level("Normal"), None);
    }

    #[test]
    fn pptx_slides_with_notes() {
        let segs = pptx(&fixture("overview.pptx")).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].loc.label, "スライド 1: RAG 検索基盤の概要");
        assert_eq!(segs[0].loc.page, Some(1));
        assert!(segs[0].text.contains("ハイブリッド検索"));
        assert!(segs[0].text.contains("[ノート]\nここで社内事例を口頭で補足する"));
        assert!(segs[1].text.contains("埋め込みモデルの日本語性能"));
    }

    #[test]
    fn excel_sheets_with_header() {
        let segs = excel(&fixture("estimate.xlsx")).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].loc.label, "見積 行2–3");
        assert_eq!(segs[0].text, "[見積]\n品目\t数量\t単価\nRTX 6000 Ada\t2\t1200000\n構築作業\t1\t800000\n");
        assert!(segs[1].text.contains("本番移行\t2027年1月"));
    }

    #[test]
    fn resolve_relative_targets() {
        assert_eq!(resolve("ppt/slides", "../notesSlides/notesSlide1.xml"), "ppt/notesSlides/notesSlide1.xml");
        assert_eq!(resolve("ppt", "slides/slide1.xml"), "ppt/slides/slide1.xml");
    }
}
