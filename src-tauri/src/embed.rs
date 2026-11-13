//! M3 — embedding client.
//!
//! Thin wrapper over the local embedding llama-server's OpenAI-compatible
//! `/v1/embeddings` endpoint.

use serde::Deserialize;

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

/// Embed `inputs` (in order) using the embedding server on `port`.
pub async fn embed(port: u16, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{port}/v1/embeddings"))
        .json(&serde_json::json!({ "input": inputs }))
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("embedding request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("embedding server returned {status}: {body}"));
    }
    let mut parsed: EmbeddingsResponse = resp
        .json()
        .await
        .map_err(|e| format!("bad embedding response: {e}"))?;
    if parsed.data.len() != inputs.len() {
        return Err(format!(
            "embedding count mismatch: sent {}, got {}",
            inputs.len(),
            parsed.data.len()
        ));
    }
    parsed.data.sort_by_key(|d| d.index);
    Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
}

/// Embed a single string.
pub async fn embed_one(port: u16, input: &str) -> Result<Vec<f32>, String> {
    let mut v = embed(port, &[input.to_string()]).await?;
    v.pop().ok_or_else(|| "empty embedding response".to_string())
}
