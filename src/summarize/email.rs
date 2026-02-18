use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::SummarizationConfig;

/// ACS Email REST API client.
///
/// Uses HMAC-SHA256 authentication per the ACS REST API spec.
/// API reference: POST {endpoint}/emails:send?api-version=2023-03-31
pub struct EmailClient {
    endpoint: String,
    access_key: String,
    sender_address: String,
    recipient_address: String,
    client: reqwest::blocking::Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendEmailRequest {
    sender_address: String,
    recipients: EmailRecipients,
    content: EmailContent,
}

#[derive(Debug, Serialize)]
struct EmailRecipients {
    to: Vec<EmailAddress>,
}

#[derive(Debug, Serialize)]
struct EmailAddress {
    address: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmailContent {
    subject: String,
    plain_text: String,
    html: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SendEmailResponse {
    id: Option<String>,
    status: Option<String>,
    error: Option<serde_json::Value>,
}

impl EmailClient {
    pub fn from_config(config: &SummarizationConfig) -> Result<Self> {
        let acs_key = if !config.acs_api_key.is_empty() {
            config.acs_api_key.clone()
        } else {
            std::env::var("DESKMIC_ACS_KEY")
                .context("ACS API key not configured. Set [summarization] acs_api_key or DESKMIC_ACS_KEY")?
        };

        if config.acs_endpoint.is_empty() {
            anyhow::bail!("ACS endpoint not configured. Set [summarization] acs_endpoint in deskmic.toml");
        }
        if config.sender_address.is_empty() {
            anyhow::bail!("Sender address not configured. Set [summarization] sender_address in deskmic.toml");
        }
        if config.recipient_address.is_empty() {
            anyhow::bail!("Recipient address not configured. Set [summarization] recipient_address in deskmic.toml");
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self {
            endpoint: config.acs_endpoint.trim_end_matches('/').to_string(),
            access_key: acs_key,
            sender_address: config.sender_address.clone(),
            recipient_address: config.recipient_address.clone(),
            client,
        })
    }

    /// Send an email with the given subject and body (plain text + optional HTML).
    pub fn send_email(
        &self,
        subject: &str,
        plain_text: &str,
        html: Option<&str>,
    ) -> Result<String> {
        let url = format!(
            "{}/emails:send?api-version=2023-03-31",
            self.endpoint
        );

        let body = SendEmailRequest {
            sender_address: self.sender_address.clone(),
            recipients: EmailRecipients {
                to: vec![EmailAddress {
                    address: self.recipient_address.clone(),
                }],
            },
            content: EmailContent {
                subject: subject.to_string(),
                plain_text: plain_text.to_string(),
                html: html.map(|s| s.to_string()),
            },
        };

        let body_json = serde_json::to_string(&body)?;
        let content_hash = compute_content_hash(body_json.as_bytes());

        // Build the date string in RFC1123 / HTTP-date format
        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();

        // Parse the host from the endpoint URL
        let host = url::Url::parse(&self.endpoint)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_default();

        let path_and_query = "/emails:send?api-version=2023-03-31";

        // Build the string-to-sign per ACS HMAC-SHA256 spec
        let string_to_sign = format!(
            "POST\n{}\n{};{};{}",
            path_and_query, date, host, content_hash,
        );

        let signature = compute_hmac_sha256(&self.access_key, &string_to_sign)?;

        let auth_header = format!(
            "HMAC-SHA256 SignedHeaders=x-ms-date;host;x-ms-content-sha256&Signature={}",
            signature
        );

        tracing::info!("Sending email via ACS to {}", self.recipient_address);

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("x-ms-date", &date)
            .header("x-ms-content-sha256", &content_hash)
            .header("Authorization", &auth_header)
            // ACS requires a unique operation ID for idempotency
            .header(
                "x-ms-client-request-id",
                uuid_v4(),
            )
            .body(body_json)
            .send()
            .context("Failed to send email via ACS")?;

        let status = response.status();
        let response_text = response.text().unwrap_or_default();

        if !status.is_success() {
            anyhow::bail!("ACS email API returned HTTP {}: {}", status.as_u16(), response_text);
        }

        // Parse the response to get the operation ID
        if let Ok(resp) = serde_json::from_str::<SendEmailResponse>(&response_text) {
            if let Some(err) = resp.error {
                anyhow::bail!("ACS email API error: {}", err);
            }
            let op_id = resp.id.unwrap_or_else(|| "unknown".to_string());
            let op_status = resp.status.unwrap_or_else(|| "unknown".to_string());
            tracing::info!("Email sent: operation={}, status={}", op_id, op_status);
            Ok(op_id)
        } else {
            tracing::info!("Email sent (HTTP {}), response: {}", status, response_text);
            Ok("accepted".to_string())
        }
    }
}

/// Compute SHA-256 hash of content, base64-encoded.
fn compute_content_hash(content: &[u8]) -> String {
    use sha2::{Sha256, Digest};
    use base64::Engine;
    
    let mut hasher = Sha256::new();
    hasher.update(content);
    let hash = hasher.finalize();
    base64::engine::general_purpose::STANDARD.encode(hash)
}

/// Compute HMAC-SHA256 signature, base64-encoded.
fn compute_hmac_sha256(key_base64: &str, message: &str) -> Result<String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use base64::Engine;
    
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(key_base64)
        .context("Invalid base64 ACS access key")?;
    
    let mut mac = Hmac::<Sha256>::new_from_slice(&key_bytes)
        .map_err(|e| anyhow::anyhow!("HMAC key error: {}", e))?;
    mac.update(message.as_bytes());
    let result = mac.finalize();
    
    Ok(base64::engine::general_purpose::STANDARD.encode(result.into_bytes()))
}

/// Generate a simple UUID v4 string for request IDs.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple pseudo-UUID from timestamp + thread ID for uniqueness
    let thread_id = std::thread::current().id();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (nanos >> 96) as u32,
        (nanos >> 80) as u16,
        (nanos >> 64) as u16 & 0x0FFF,
        ((nanos >> 48) as u16 & 0x3FFF) | 0x8000,
        (nanos & 0xFFFF_FFFF_FFFF) ^ (format!("{:?}", thread_id).len() as u128),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_content_hash() {
        let hash = compute_content_hash(b"hello");
        // SHA-256 of "hello" is well-known
        assert!(!hash.is_empty());
        // Verify it's valid base64
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD.decode(&hash);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap().len(), 32); // SHA-256 = 32 bytes
    }

    #[test]
    fn test_compute_hmac_sha256() {
        use base64::Engine;
        // Use a known base64 key
        let key = base64::engine::general_purpose::STANDARD.encode(b"test-key-12345678");
        let sig = compute_hmac_sha256(&key, "test message").unwrap();
        assert!(!sig.is_empty());
        // Verify it's valid base64
        let decoded = base64::engine::general_purpose::STANDARD.decode(&sig);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap().len(), 32); // HMAC-SHA256 = 32 bytes
    }

    #[test]
    fn test_uuid_v4_format() {
        let id = uuid_v4();
        // Should look like a UUID (8-4-4-4-12 hex chars with dashes)
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5, "UUID should have 5 parts: {}", id);
    }

    #[test]
    fn test_from_config_missing_endpoint() {
        let config = SummarizationConfig::default();
        let result = EmailClient::from_config(&config);
        assert!(result.is_err());
    }
}
