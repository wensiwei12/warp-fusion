//! # wf-source-api
//!
//! Minimal Arrow-native source API — one trait, one error type.
//!
//! ## Why separate from `wp-connector-api`
//!
//! `wp-connector-api` sources produce `SourceEvent { payload: RawData }`,
//! designed for downstream parse pipelines. CEP engines like warp-fusion
//! operate on Arrow `RecordBatch` directly — converting from `RawData` per
//! event adds overhead and loses columnar benefits.
//!
//! `BatchSource` fills this gap without touching the existing source model.

use arrow::record_batch::RecordBatch;
use async_trait::async_trait;

/// Unified error type for batch source operations.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("connect error: {0}")]
    Connect(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("source not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Other(String),
}

/// A batch-oriented data source that produces Arrow RecordBatches.
///
/// Each call returns zero or more `(stream_name, RecordBatch)` pairs.
/// An empty `Vec` means "no data available right now" (not EOF).
///
/// # Implementation notes
///
/// - File/NDJSON sources: parse lines into RecordBatch per stream
/// - Kafka sources: decode message payload into RecordBatch
/// - TCP sources: decode Arrow IPC frames, each frame = one pair
#[async_trait]
pub trait BatchSource: Send {
    /// Attempt to receive zero or more batches.
    async fn receive_batch(&mut self) -> Result<Vec<(String, RecordBatch)>, SourceError>;

    /// Human-readable source identifier for logging / metrics.
    fn source_name(&self) -> &str;
}
