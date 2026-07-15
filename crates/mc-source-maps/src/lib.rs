use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceMapError {
    #[error("source map not found for module: {0}")]
    NotFound(String),
    #[error("invalid source map: {0}")]
    Invalid(String),
}

pub struct SourceMaps;

impl SourceMaps {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SourceMaps {
    fn default() -> Self {
        Self::new()
    }
}
