//! The embedding and reranking servers: what each model family is sent,
//! the indexer's batches of passages, search's query and reranking.

use std::time::Duration;

use serde_json::{Value, json};

/// Each family's query prefix, from its model card: without it results get
/// worse. Pooling and the end token are in each GGUF.
pub fn query_prefix(family: &str) -> Option<&'static str> {
    match family {
        "embeddinggemma" => Some("task: search result | query: "),
        "jina-embeddings-v5-text-nano" => Some("Query: "),
        "qwen3-embedding" => {
            Some("Instruct: Given a search query, retrieve passages that answer it\nQuery:")
        }
        _ => None,
    }
}

/// A passage as its family is sent it, from its model card.
pub fn passage(family: &str, title: &str, text: &str) -> String {
    match family {
        "embeddinggemma" => format!("title: {title} | text: {text}"),
        "jina-embeddings-v5-text-nano" => format!("Document: {text}"),
        _ => text.to_string(),
    }
}

const BATCH: usize = 8;
/// A GPU behind a network hop does better with larger batches.
const REMOTE_BATCH: usize = 32;
const MIN_EMBED_CHARS: usize = 200;

/// Why a batch got no vectors. `Status` names the server's answer, which a
/// file's status keeps as "HTTPStatusError: ..." so the next run retries it.
#[derive(Debug)]
pub enum EmbedError {
    Transport(String),
    Status(u16),
    Shape(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Transport(e) => write!(f, "TransportError: {e}"),
            EmbedError::Status(code) => {
                write!(f, "HTTPStatusError: the embedding server answered {code}.")
            }
            EmbedError::Shape(e) => write!(f, "ValueError: {e}"),
        }
    }
}

/// An embedding server: llama-server on the Pi, or Whoopy, which needs a key
/// and a model name. Waking the desktop takes up to two minutes, within the timeout.
pub struct Embedder {
    client: reqwest::blocking::Client,
    url: String,
    /// Whoopy's model; None for the Pi.
    pub model: Option<String>,
    key: Option<String>,
    batch: usize,
}

impl Embedder {
    pub fn local(url: &str) -> Self {
        Embedder {
            client: blocking_client(),
            url: url.trim_end_matches('/').to_string(),
            model: None,
            key: None,
            batch: BATCH,
        }
    }

    pub fn remote(url: &str, model: &str, key: &str) -> Self {
        Embedder {
            client: blocking_client(),
            url: url.trim_end_matches('/').to_string(),
            model: Some(model.to_string()),
            key: Some(key.to_string()),
            batch: REMOTE_BATCH,
        }
    }

    pub fn is_remote(&self) -> bool {
        self.model.is_some()
    }

    /// The embedding server loads its model at start; wait up to two minutes.
    pub fn wait_ready(&self) -> bool {
        for _ in 0..60 {
            if let Ok(r) = self.client.get(format!("{}/health", self.url)).send()
                && r.status().as_u16() == 200
            {
                return true;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        false
    }

    /// Raw vectors for texts, in order. A text the server refuses, as one past
    /// the model's 2048 tokens (music notation takes several a character), is
    /// embedded by its first half, down to MIN_EMBED_CHARS. Whoopy gets the
    /// model's name; there a 500 means the desktop is out of reach, since
    /// Ollama cuts a long text itself.
    fn vectors(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut body = json!({"input": texts});
        if let Some(model) = &self.model {
            body["model"] = json!(model);
        }
        let mut req = self
            .client
            .post(format!("{}/v1/embeddings", self.url))
            .json(&body);
        if let Some(key) = &self.key {
            req = req.bearer_auth(key);
        }
        let resp = req
            .send()
            .map_err(|e| EmbedError::Transport(e.to_string()))?;
        let code = resp.status().as_u16();
        if code == 500 && self.model.is_none() && texts.len() > 1 {
            let mut out = Vec::new();
            for t in texts {
                out.extend(self.vectors(std::slice::from_ref(t))?);
            }
            return Ok(out);
        }
        let first: Vec<char> = texts
            .first()
            .map(|t| t.chars().collect())
            .unwrap_or_default();
        if code == 500 && self.model.is_none() && first.len() > MIN_EMBED_CHARS {
            return self.vectors(&[first[..first.len() / 2].iter().collect()]);
        }
        if !resp.status().is_success() {
            return Err(EmbedError::Status(code));
        }
        let data: Value = resp.json().map_err(|e| EmbedError::Shape(e.to_string()))?;
        match parse_vectors(&data) {
            Some(rows) if same_shape(&rows, texts.len()) => Ok(rows),
            _ => Err(EmbedError::Shape(format!(
                "the embedding server's answer does not hold {} vectors of one size.",
                texts.len()
            ))),
        }
    }

    /// Unit vectors for texts, in order.
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut rows = Vec::new();
        for batch in texts.chunks(self.batch) {
            rows.extend(self.vectors(batch)?);
        }
        Ok(rows.into_iter().map(normalized).collect())
    }
}

fn blocking_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .no_proxy()
        .build()
        .expect("an HTTP client")
}

/// The vectors of an OpenAI-style answer, in the order of their index.
fn parse_vectors(data: &Value) -> Option<Vec<Vec<f32>>> {
    let mut rows: Vec<(i64, Vec<f32>)> = Vec::new();
    for d in data["data"].as_array()? {
        let vector = d["embedding"]
            .as_array()?
            .iter()
            .map(|x| x.as_f64().map(|x| x as f32))
            .collect::<Option<_>>()?;
        rows.push((d["index"].as_i64()?, vector));
    }
    rows.sort_by_key(|(i, _)| *i);
    Some(rows.into_iter().map(|(_, v)| v).collect())
}

/// Whether rows are n vectors of one non-zero size.
pub fn same_shape(rows: &[Vec<f32>], n: usize) -> bool {
    rows.len() == n
        && rows
            .first()
            .is_some_and(|r| !r.is_empty() && rows.iter().all(|x| x.len() == r.len()))
}

pub fn normalized(v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.into_iter().map(|x| x / norm).collect()
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Little-endian float32 rows, as the index stores them.
pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// The query's unit vector, or None when the embedding server does not answer.
pub async fn embed_query(client: &reqwest::Client, url: &str, text: &str) -> Option<Vec<f32>> {
    let resp = client
        .post(format!("{url}/v1/embeddings"))
        .json(&json!({"input": [text]}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let data: Value = resp.json().await.ok()?;
    parse_vectors(&data)?.into_iter().next().map(normalized)
}

const RERANK_TIMEOUT_S: u64 = 20;

/// Indexes of documents, most relevant to the query first; None when the
/// reranking server does not answer.
pub async fn rerank(
    client: &reqwest::Client,
    url: &str,
    query: &str,
    docs: &[String],
) -> Option<Vec<usize>> {
    let resp = client
        .post(format!("{url}/v1/rerank"))
        .json(&json!({"query": query, "documents": docs}))
        .timeout(Duration::from_secs(RERANK_TIMEOUT_S))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let data: Value = resp.json().await.ok()?;
    let mut scored: Vec<(f64, &Value)> = data["results"]
        .as_array()?
        .iter()
        .map(|r| r["relevance_score"].as_f64().map(|s| (s, &r["index"])))
        .collect::<Option<_>>()?;
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut order: Vec<usize> = scored
        .iter()
        .filter_map(|(_, i)| i.as_u64().map(|i| i as usize))
        .filter(|&i| i < docs.len())
        .collect();
    order.extend(
        (0..docs.len())
            .filter(|i| !order.contains(i))
            .collect::<Vec<_>>(),
    );
    Some(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_too_long_for_the_model_is_embedded_by_its_start() {
        // As a text past the model's tokens would, one holding "♪" and longer than 600 characters gets a 500.
        let (url, _) = crate::testing::serve(|_, _, body| {
            let texts: Vec<String> = serde_json::from_str::<Value>(body).unwrap()["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().to_string())
                .collect();
            if texts
                .iter()
                .any(|t| t.contains('♪') && t.chars().count() > 600)
            {
                return (500, "{}".into());
            }
            let data: Vec<Value> = (0..texts.len())
                .map(|i| json!({"index": i, "embedding": [1.0, 2.0]}))
                .collect();
            (200, json!({"data": data}).to_string())
        });
        let rows = Embedder::local(&url)
            .embed(&["court".into(), "♪ ".repeat(1000), "autre".into()])
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!((dot(&rows[1], &rows[1]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_answer_without_one_vector_per_text_is_refused() {
        let (url, _) = crate::testing::serve(|_, _, _| (200, json!({"data": []}).to_string()));
        let shape = Embedder::local(&url).embed(&["a".into(), "b".into()]);
        assert!(matches!(shape, Err(EmbedError::Shape(_))));
    }
}
