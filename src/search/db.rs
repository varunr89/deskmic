use crate::search::{SearchParams, SearchResult};
use anyhow::Result;
use std::path::Path;

pub struct SearchDb;

impl SearchDb {
    pub fn open(_path: &Path) -> Result<Self> {
        todo!("SearchDb::open")
    }

    pub fn search(&self, _embedding: &[f32], _params: &SearchParams) -> Result<Vec<SearchResult>> {
        todo!("SearchDb::search")
    }
}
