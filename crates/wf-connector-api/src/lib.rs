//! # wf-connector-api
//!
//! Minimal Arrow-native connector API for warp-fusion.
//!
//! ## Design
//!
//! `wp-connector-api` sources produce `SourceEvent { payload: RawData }`,
//! designed for downstream parse pipelines. CEP engines like warp-fusion
//! operate on Arrow `RecordBatch` directly.
//!
//! `wf-connector-api` fills this gap — one trait for sources, extensible
//! to sinks in the future (e.g. `BatchSink` for Arrow-native output).
//!
//! ## Relationship with `wp-connector-api`
//!
//! | | wp-connector-api | wf-connector-api |
//! |---|---|---|
//! | Source model | `SourceEvent { payload: RawData }` | `(stream, RecordBatch)` |
//! | Consumer | parse pipeline (WPL) | CEP engine (warp-fusion) |
//! | Sink model | `SinkFactory` (bytes/data records) | TBD (`BatchSink`) |
//!
//! `wp-connectors` (the implementation crate) can implement BOTH traits
//! for the same connector (Kafka / File / TCP), sharing connection logic.

use arrow::record_batch::RecordBatch;
use async_trait::async_trait;

// -- Source ------------------------------------------------------------------

/// Unified error type for connector operations.
#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("connect error: {0}")]
    Connect(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Other(String),
}

/// A batch-oriented data source that produces Arrow RecordBatches.
///
/// Each call returns zero or more `(stream_name, RecordBatch)` pairs.
/// An empty `Vec` means "no data available right now" (not EOF).
#[async_trait]
pub trait BatchSource: Send {
    /// Attempt to receive zero or more batches.
    async fn receive_batch(&mut self) -> Result<Vec<(String, RecordBatch)>, ConnectorError>;

    /// Human-readable source identifier for logging / metrics.
    fn source_name(&self) -> &str;
}

// -- Sink (TBD) --------------------------------------------------------------

// Future extension:
//
// ```ignore
// #[async_trait]
// pub trait BatchSink: Send {
//     async fn send_batch(&mut self, stream: &str, batch: &RecordBatch)
//         -> Result<(), ConnectorError>;
//     async fn flush(&mut self) -> Result<(), ConnectorError>;
// }
// ```
