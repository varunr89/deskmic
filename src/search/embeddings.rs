use crate::config::Config;
use anyhow::Result;

pub struct EmbeddingClient;

impl EmbeddingClient {
    pub fn from_config(_config: &Config) -> Result<Self> {
        todo!("EmbeddingClient::from_config")
    }

    pub fn embed_single(&self, _text: &str) -> Result<Vec<f32>> {
        todo!("EmbeddingClient::embed_single")
    }

    pub fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        todo!("EmbeddingClient::embed_batch")
    }
}
