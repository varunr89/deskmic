use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Maximum number of texts to embed in a single API call.
const BATCH_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// Request / Response types (private)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    usage: Option<EmbeddingUsage>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Debug, Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: u64,
    total_tokens: u64,
}

// ---------------------------------------------------------------------------
// EmbeddingClient
// ---------------------------------------------------------------------------

pub struct EmbeddingClient {
    endpoint: String,
    api_key: String,
    deployment: String,
    client: reqwest::blocking::Client,
}

impl std::fmt::Debug for EmbeddingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingClient")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("deployment", &self.deployment)
            .finish_non_exhaustive()
    }
}

impl EmbeddingClient {
    /// Create a new embedding client from the application config.
    ///
    /// Uses the Azure OpenAI endpoint/api_key from `[transcription.azure]`
    /// and the deployment name from `[search].embedding_deployment`.
    pub fn from_config(config: &Config) -> Result<Self> {
        let azure = &config.transcription.azure;

        let endpoint = if azure.endpoint.is_empty() {
            anyhow::bail!(
                "Azure OpenAI endpoint not configured. \
                 Set [transcription.azure] endpoint in deskmic.toml"
            );
        } else {
            azure.endpoint.trim_end_matches('/').to_string()
        };

        let api_key = if !azure.api_key.is_empty() {
            azure.api_key.clone()
        } else {
            std::env::var("DESKMIC_AZURE_KEY").context(
                "Azure API key not configured. \
                 Set [transcription.azure] api_key or DESKMIC_AZURE_KEY",
            )?
        };

        let deployment = if config.search.embedding_deployment.is_empty() {
            anyhow::bail!(
                "Embedding deployment not configured. \
                 Set [search] embedding_deployment in deskmic.toml"
            );
        } else {
            config.search.embedding_deployment.clone()
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self {
            endpoint,
            api_key,
            deployment,
            client,
        })
    }

    /// Embed a single piece of text, returning its embedding vector.
    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let mut results = self.embed_batch(&[text])?;
        results
            .pop()
            .context("Expected exactly one embedding result")
    }

    /// Embed a batch of texts, returning one embedding vector per input text
    /// in the same order as the input slice.
    ///
    /// Texts are sent to the API in groups of [`BATCH_SIZE`]. The response
    /// items are sorted by their `index` field so that the returned vectors
    /// always match the input order.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!(
            "{}/openai/deployments/{}/embeddings?api-version=2024-06-01",
            self.endpoint, self.deployment
        );

        let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(BATCH_SIZE) {
            let request = EmbeddingRequest {
                input: chunk.iter().map(|t| t.to_string()).collect(),
                model: self.deployment.clone(),
            };

            tracing::debug!(
                "Sending embedding request for {} texts to {}/{}",
                chunk.len(),
                self.endpoint,
                self.deployment
            );

            // Retry with exponential backoff for rate limiting.
            let mut last_error = String::new();
            let mut response_ok = None;
            for attempt in 0..5 {
                let resp = self
                    .client
                    .post(&url)
                    .header("api-key", &self.api_key)
                    .json(&request)
                    .send()
                    .context("Failed to send embedding request")?;

                let status = resp.status();
                if status.is_success() {
                    response_ok = Some(resp);
                    break;
                }

                let error_body = resp
                    .text()
                    .unwrap_or_else(|_| "unable to read response body".to_string());

                if status.as_u16() == 429 && attempt < 4 {
                    // Rate limited — extract retry-after or use exponential backoff.
                    let wait_secs = 2u64.pow(attempt + 1); // 2, 4, 8, 16
                    tracing::warn!(
                        "Rate limited (429), retrying in {}s (attempt {}/5)",
                        wait_secs,
                        attempt + 1
                    );
                    std::thread::sleep(std::time::Duration::from_secs(wait_secs));
                    last_error = error_body;
                    continue;
                }

                anyhow::bail!(
                    "Azure OpenAI returned HTTP {}: {}",
                    status.as_u16(),
                    error_body
                );
            }

            let response = response_ok.context(format!(
                "All 5 retry attempts failed. Last error: {}",
                last_error
            ))?;

            let embed_response: EmbeddingResponse = response
                .json()
                .context("Failed to parse embedding response")?;

            if let Some(usage) = &embed_response.usage {
                tracing::debug!(
                    "Embedding token usage: prompt={}, total={}",
                    usage.prompt_tokens,
                    usage.total_tokens
                );
            }

            // Sort by index to guarantee order matches the input.
            let mut data = embed_response.data;
            data.sort_by_key(|d| d.index);

            for item in data {
                all_embeddings.push(item.embedding);
            }
        }

        Ok(all_embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AzureConfig, SearchConfig};

    #[test]
    fn test_from_config_missing_endpoint() {
        let mut config = Config::default();
        config.search.embedding_deployment = "text-embedding-3-large".to_string();
        let result = EmbeddingClient::from_config(&config);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("endpoint"),
            "Error should mention endpoint"
        );
    }

    #[test]
    fn test_from_config_missing_deployment() {
        let mut config = Config::default();
        config.transcription.azure = AzureConfig {
            endpoint: "https://example.openai.azure.com".to_string(),
            api_key: "test-key".to_string(),
            deployment: "whisper".to_string(),
        };
        // embedding_deployment is empty by default
        let result = EmbeddingClient::from_config(&config);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("deployment"),
            "Error should mention deployment"
        );
    }

    #[test]
    fn test_from_config_success() {
        let mut config = Config::default();
        config.transcription.azure = AzureConfig {
            endpoint: "https://example.openai.azure.com".to_string(),
            api_key: "test-key".to_string(),
            deployment: "whisper".to_string(),
        };
        config.search = SearchConfig {
            embedding_deployment: "text-embedding-3-large".to_string(),
            ..Default::default()
        };
        let client = EmbeddingClient::from_config(&config);
        assert!(client.is_ok());
    }

    #[test]
    fn test_embedding_request_serialization() {
        let request = EmbeddingRequest {
            input: vec!["hello world".to_string(), "goodbye".to_string()],
            model: "text-embedding-3-large".to_string(),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["input"], serde_json::json!(["hello world", "goodbye"]));
        assert_eq!(json["model"], "text-embedding-3-large");
    }

    #[test]
    fn test_embedding_response_deserialization() {
        let json = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "object": "embedding",
                    "embedding": [0.1, 0.2, 0.3],
                    "index": 1
                },
                {
                    "object": "embedding",
                    "embedding": [0.4, 0.5, 0.6],
                    "index": 0
                }
            ],
            "model": "text-embedding-3-large",
            "usage": {
                "prompt_tokens": 10,
                "total_tokens": 10
            }
        });

        let response: EmbeddingResponse = serde_json::from_value(json).unwrap();
        assert_eq!(response.data.len(), 2);
        // Data comes back unsorted by index in this mock
        assert_eq!(response.data[0].index, 1);
        assert_eq!(response.data[1].index, 0);
        assert_eq!(response.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(response.data[1].embedding, vec![0.4, 0.5, 0.6]);

        let usage = response.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.total_tokens, 10);
    }
}
