//! Ollama / OpenAI 互換 API の最小クライアント(同期 API)。
//!
//! インデクサのバックグラウンドスレッドから呼ぶので blocking で統一している。
//! Tauri の async コマンドから使うときは spawn_blocking で包むこと。

use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    settings::{Provider, ProviderKind},
    CoreError, Result,
};

fn client(p: &Provider) -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(p.timeout_secs))
        // LAN 内サーバー前提。社内プロキシを経由させない
        .no_proxy()
        .build()?)
}

fn url(p: &Provider, path: &str) -> String {
    format!("{}/{}", p.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn auth(req: RequestBuilder, p: &Provider) -> RequestBuilder {
    match &p.api_key {
        Some(k) if !k.is_empty() => req.bearer_auth(k),
        _ => req,
    }
}

fn check(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        Ok(resp)
    } else {
        let body = resp.text().unwrap_or_default();
        Err(CoreError::Http(format!(
            "HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        )))
    }
}

/// 利用可能なモデル名の一覧(接続テストを兼ねる)
pub fn list_models(p: &Provider) -> Result<Vec<String>> {
    let c = client(p)?;
    let mut names = match p.kind {
        ProviderKind::Ollama => {
            #[derive(Deserialize)]
            struct Tags {
                models: Vec<Model>,
            }
            #[derive(Deserialize)]
            struct Model {
                name: String,
            }
            let resp = check(auth(c.get(url(p, "api/tags")), p).send()?)?;
            resp.json::<Tags>()?.models.into_iter().map(|m| m.name).collect::<Vec<_>>()
        }
        ProviderKind::OpenaiCompat => {
            #[derive(Deserialize)]
            struct List {
                data: Vec<Model>,
            }
            #[derive(Deserialize)]
            struct Model {
                id: String,
            }
            let resp = check(auth(c.get(url(p, "models")), p).send()?)?;
            resp.json::<List>()?.data.into_iter().map(|m| m.id).collect()
        }
    };
    names.sort();
    Ok(names)
}

/// 複数テキストの埋め込みを取得する
pub fn embed(p: &Provider, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
    let c = client(p)?;
    let vectors = match p.kind {
        ProviderKind::Ollama => {
            #[derive(Deserialize)]
            struct Resp {
                embeddings: Vec<Vec<f32>>,
            }
            let req = c.post(url(p, "api/embed")).json(&json!({ "model": model, "input": inputs }));
            check(auth(req, p).send()?)?.json::<Resp>()?.embeddings
        }
        ProviderKind::OpenaiCompat => {
            #[derive(Deserialize)]
            struct Resp {
                data: Vec<Item>,
            }
            #[derive(Deserialize)]
            struct Item {
                index: usize,
                embedding: Vec<f32>,
            }
            let req = c.post(url(p, "embeddings")).json(&json!({ "model": model, "input": inputs }));
            let mut data = check(auth(req, p).send()?)?.json::<Resp>()?.data;
            data.sort_by_key(|d| d.index);
            data.into_iter().map(|d| d.embedding).collect()
        }
    };
    if vectors.len() != inputs.len() {
        return Err(CoreError::Http(format!(
            "埋め込みの件数が一致しません(入力 {} / 出力 {})",
            inputs.len(),
            vectors.len()
        )));
    }
    Ok(vectors)
}

/// JSON オブジェクトを返させるチャット呼び出し。
///
/// - Ollama: `format: "json"`、思考モードは切る(`think: false`。非対応モデルなら外して再試行)
/// - OpenAI 互換: `response_format: json_object`(非対応サーバーなら外して再試行)
///
/// 応答に `<think>` やコードフェンスが混ざっても最初の JSON オブジェクトを取り出す。
pub fn chat_json(p: &Provider, model: &str, system: &str, user: &str) -> Result<Value> {
    let c = client(p)?;
    let messages = json!([
        { "role": "system", "content": system },
        { "role": "user", "content": user },
    ]);
    let content = match p.kind {
        ProviderKind::Ollama => {
            #[derive(Deserialize)]
            struct Resp {
                message: Msg,
            }
            #[derive(Deserialize)]
            struct Msg {
                content: String,
            }
            let mut body = json!({
                "model": model, "messages": messages, "stream": false, "format": "json",
                "think": false, "options": { "temperature": 0 }
            });
            let first = check(auth(c.post(url(p, "api/chat")).json(&body), p).send()?);
            let resp = match first {
                Err(CoreError::Http(msg)) if msg.contains("think") => {
                    body.as_object_mut().unwrap().remove("think");
                    check(auth(c.post(url(p, "api/chat")).json(&body), p).send()?)?
                }
                other => other?,
            };
            resp.json::<Resp>()?.message.content
        }
        ProviderKind::OpenaiCompat => {
            #[derive(Deserialize)]
            struct Resp {
                choices: Vec<Choice>,
            }
            #[derive(Deserialize)]
            struct Choice {
                message: Msg,
            }
            #[derive(Deserialize)]
            struct Msg {
                content: Option<String>,
            }
            let mut body = json!({
                "model": model, "messages": messages, "temperature": 0,
                "response_format": { "type": "json_object" }
            });
            let first = check(auth(c.post(url(p, "chat/completions")).json(&body), p).send()?);
            let resp = match first {
                Err(CoreError::Http(msg)) if msg.starts_with("HTTP 400") => {
                    body.as_object_mut().unwrap().remove("response_format");
                    check(auth(c.post(url(p, "chat/completions")).json(&body), p).send()?)?
                }
                other => other?,
            };
            resp.json::<Resp>()?
                .choices
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .unwrap_or_default()
        }
    };
    extract_json_object(&content)
        .ok_or_else(|| CoreError::Http(format!("LLM の応答が JSON ではありません: {}", truncate(&content, 200))))
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `<think>…</think>` やコードフェンスを除いて、最初の JSON オブジェクトを取り出す
pub fn extract_json_object(s: &str) -> Option<Value> {
    let s = match (s.find("<think>"), s.find("</think>")) {
        (Some(_), Some(end)) => &s[end + "</think>".len()..],
        _ => s,
    };
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str::<Value>(&s[start..=end])
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_join() {
        let p = Provider {
            id: "x".into(),
            kind: ProviderKind::OpenaiCompat,
            base_url: "http://10.0.0.5:8000/v1/".into(),
            api_key: None,
            timeout_secs: 5,
        };
        assert_eq!(url(&p, "/models"), "http://10.0.0.5:8000/v1/models");
    }

    #[test]
    fn unreachable_server_is_error() {
        let p = Provider {
            id: "x".into(),
            kind: ProviderKind::Ollama,
            base_url: "http://127.0.0.1:9".into(),
            api_key: None,
            timeout_secs: 2,
        };
        match list_models(&p) {
            Err(CoreError::Http(m)) => assert!(m.contains("127.0.0.1:9 に接続できません"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn json_extraction() {
        let v = extract_json_object("<think>考え中 {x}</think>\n```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        assert!(extract_json_object("no json").is_none());
        assert!(extract_json_object("[1,2]").is_none());
    }
}
