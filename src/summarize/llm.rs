use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Serialize)]
struct ChatRequest {
    messages: Vec<ChatMessage>,
    max_completion_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

pub struct LlmClient {
    endpoint: String,
    api_key: String,
    deployment: String,
    client: reqwest::blocking::Client,
}

impl std::fmt::Debug for LlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClient")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("deployment", &self.deployment)
            .finish()
    }
}

impl LlmClient {
    /// Create a new LLM client from config.
    /// Uses the Azure OpenAI endpoint/api_key from [transcription.azure]
    /// and the deployment name from [summarization].
    pub fn from_config(config: &Config) -> Result<Self> {
        let azure = &config.transcription.azure;
        let summarization = &config.summarization;

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
            std::env::var("DESKMIC_AZURE_KEY")
                .context("Azure API key not configured. Set [transcription.azure] api_key or DESKMIC_AZURE_KEY")?
        };

        let deployment = if summarization.deployment.is_empty() {
            anyhow::bail!(
                "Summarization deployment not configured. \
                 Set [summarization] deployment in deskmic.toml"
            );
        } else {
            summarization.deployment.clone()
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        Ok(Self {
            endpoint,
            api_key,
            deployment,
            client,
        })
    }

    /// Send a chat completion request and return the response text.
    pub fn chat(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version=2024-06-01",
            self.endpoint, self.deployment
        );

        let request = ChatRequest {
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                },
            ],
            max_completion_tokens: 4096,
        };

        tracing::info!(
            "Sending chat completion request to {}/{}",
            self.endpoint,
            self.deployment
        );

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&request)
            .send()
            .context("Failed to send chat completion request")?;

        let status = response.status();
        if !status.is_success() {
            let error_body = response
                .text()
                .unwrap_or_else(|_| "unable to read response body".to_string());
            anyhow::bail!(
                "Azure OpenAI returned HTTP {}: {}",
                status.as_u16(),
                error_body
            );
        }

        let chat_response: ChatResponse = response
            .json()
            .context("Failed to parse chat completion response")?;

        if let Some(usage) = &chat_response.usage {
            tracing::info!(
                "Token usage: prompt={}, completion={}, total={}",
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens
            );
        }

        let choice = chat_response
            .choices
            .first()
            .context("No choices in chat completion response")?;

        if let Some(reason) = &choice.finish_reason {
            if reason != "stop" {
                tracing::warn!("Chat completion finish_reason: {}", reason);
            }
        }

        Ok(choice.message.content.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AzureConfig, SummarizationConfig};

    #[test]
    fn test_from_config_missing_endpoint() {
        let mut config = Config::default();
        config.summarization.deployment = "gpt-4o".to_string();
        let result = LlmClient::from_config(&config);
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
        // deployment is empty by default in SummarizationConfig
        let result = LlmClient::from_config(&config);
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
        config.summarization = SummarizationConfig {
            deployment: "gpt-4o".to_string(),
            ..Default::default()
        };
        let client = LlmClient::from_config(&config);
        assert!(client.is_ok());
    }
}
