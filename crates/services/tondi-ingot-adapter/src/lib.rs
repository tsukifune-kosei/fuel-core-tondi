//! # Tondi Ingot Adapter
//!
//! This crate implements a Tondi L1 block submission adapter for FuelVM,
//! enabling FuelVM to operate as a Sovereign Rollup with Tondi as the DA layer.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              FuelVM (Sovereign Rollup)                      │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │  Block Producer (PoA)                                 │  │
//! │  │  - Produces blocks and notifies adapter               │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! │                          │                                  │
//! │                          ↓ Block notification               │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │  TondiIngotAdapter                                    │  │
//! │  │  - Batches blocks for submission                      │  │
//! │  │  - Builds Ingot payloads with state commitments       │  │
//! │  │  - Submits to Tondi L1                                │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//!                             │
//!                             ↓ Ingot Transaction
//! ┌─────────────────────────────────────────────────────────────┐
//! │              Tondi L1 (Data Availability)                   │
//! │  - Pay2Ingot Output with FuelVM batch payload               │
//! │  - State commitments and block data                         │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Features
//!
//! - **Batch Aggregation**: Aggregates multiple FuelVM blocks into batches
//! - **State Commitments**: Includes state roots and receipts roots
//! - **Chain Continuity**: Uses parent_ref for batch chain verification
//! - **Configurable Submission**: Flexible timing and batch size controls

#![deny(clippy::arithmetic_side_effects)]
#![deny(clippy::cast_possible_truncation)]
#![deny(unused_crate_dependencies)]
#![deny(missing_docs)]
#![deny(warnings)]

mod adapter;
mod config;
mod error;
mod payload;
pub mod ports;
mod service;
mod storage;
mod sync;
mod types;

pub use adapter::{BatchSubmittedEvent, TondiIngotAdapter};
pub use config::{Config, SyncConfig};
pub use error::TondiAdapterError;
pub use payload::{
    FuelBlockBatchPayload,
    FuelBlockData,
    PayloadBuilder,
};
pub use service::{
    new_service,
    new_service_with_sync,
    Service,
    ServiceWithSync,
    SharedState,
    SyncState,
};
pub use storage::TondiSubmissionDb;
pub use sync::{ConfirmationLevel, IndexerSyncService, SyncEvent};
pub use types::{
    BatchCommitment,
    BatchHeader,
    BatchInfo,
    BatchL1Status,
    BatchRecord,
    BatchValidationResult,
    BlockValidationError,
    PendingBatch,
    SubmissionStatus,
};

