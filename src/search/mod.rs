pub mod chunker;
pub mod db;
pub mod embeddings;
pub mod indexer;

use anyhow::Result;
use crate::config::Config;

/// Run the indexing pipeline: scan transcripts, chunk, embed, store.
pub fn run_index(config: &Config) -> Result<()> {
    indexer::run_index(config)
}

/// Search result returned by the search pipeline.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub date: String,
    pub start_time: String,
    pub end_time: String,
    pub source: String,
    pub score: f32,
    pub text: String,
    pub files: Vec<String>,
}

/// Search parameters.
pub struct SearchParams {
    pub query: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub source: Option<String>,
    pub limit: usize,
}

/// Run a semantic search query against the index.
pub fn run_search(config: &Config, params: SearchParams) -> Result<Vec<SearchResult>> {
    let db_path = config.output.directory.join("deskmic-search.db");
    let db = db::SearchDb::open(&db_path)?;

    let embedding = embeddings::EmbeddingClient::from_config(config)?
        .embed_single(&params.query)?;

    db.search(&embedding, &params)
}
