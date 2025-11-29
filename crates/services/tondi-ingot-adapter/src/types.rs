//! Core types for the Tondi Ingot Adapter.

use fuel_core_types::fuel_types::BlockHeight;
use serde::{
    Deserialize,
    Serialize,
};
use std::time::Instant;

/// FuelVM batch header for quick validation.
///
/// This header is placed at the **start of the FuelBatchPayload** (inside IngotWitness.payload)
/// to allow L2 indexers to quickly check batch relevance before full deserialization.
///
/// ## Wire Format (45 bytes, fixed size)
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │  version: u8           (1 byte)  - Protocol version         │
/// │  start_height: u64     (8 bytes) - First block height       │
/// │  block_count: u32      (4 bytes) - Number of blocks         │
/// │  parent_hash: [u8; 32] (32 bytes) - L2 chain continuity     │
/// └─────────────────────────────────────────────────────────────┘
/// ```
///
/// ## Security Model
/// - **Authentication**: Handled by IngotOutput.lock = PubKey { sequencer_pubkey }
///   and IngotWitness.auth_sigs containing the Schnorr signature
/// - **Chain Continuity**: parent_hash must match the last confirmed L2 block hash
/// - **Data Binding**: IngotOutput.hash_payload = blake3(entire FuelBatchPayload)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchHeader {
    /// Protocol version (for future upgrades).
    pub version: u8,
    /// First block height in the batch.
    pub start_height: u64,
    /// Number of blocks in the batch.
    pub block_count: u32,
    /// Parent hash of the first block (L2 chain continuity verification).
    /// This is the block hash of the last L2 block before this batch.
    pub parent_hash: [u8; 32],
}

impl BatchHeader {
    /// Current protocol version.
    pub const VERSION: u8 = 1;

    /// Size of the serialized header in bytes.
    /// 1 + 8 + 4 + 32 = 45 bytes
    pub const SERIALIZED_SIZE: usize = 1 + 8 + 4 + 32;

    /// Create a new BatchHeader.
    pub fn new(
        start_height: u64,
        block_count: u32,
        parent_hash: [u8; 32],
    ) -> Self {
        Self {
            version: Self::VERSION,
            start_height,
            block_count,
            parent_hash,
        }
    }

    /// Get the end height (calculated from start_height + block_count - 1).
    pub fn end_height(&self) -> u64 {
        self.start_height
            .saturating_add(self.block_count as u64)
            .saturating_sub(1)
    }

    /// Serialize the header to a fixed-size byte array.
    pub fn to_bytes(&self) -> [u8; Self::SERIALIZED_SIZE] {
        let mut bytes = [0u8; Self::SERIALIZED_SIZE];
        bytes[0] = self.version;
        bytes[1..9].copy_from_slice(&self.start_height.to_le_bytes());
        bytes[9..13].copy_from_slice(&self.block_count.to_le_bytes());
        bytes[13..45].copy_from_slice(&self.parent_hash);
        bytes
    }

    /// Deserialize a header from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SERIALIZED_SIZE {
            return None;
        }

        let version = bytes[0];
        let start_height = u64::from_le_bytes(bytes[1..9].try_into().ok()?);
        let block_count = u32::from_le_bytes(bytes[9..13].try_into().ok()?);
        let parent_hash: [u8; 32] = bytes[13..45].try_into().ok()?;

        Some(Self {
            version,
            start_height,
            block_count,
            parent_hash,
        })
    }

    /// Validate basic header consistency.
    pub fn is_valid(&self) -> bool {
        // Version check
        if self.version == 0 || self.version > Self::VERSION {
            return false;
        }

        // Block count must be at least 1
        if self.block_count == 0 {
            return false;
        }

        true
    }

    /// Check if this batch follows the given parent.
    pub fn follows_parent(&self, parent_last_hash: &[u8; 32]) -> bool {
        self.parent_hash == *parent_last_hash
    }

    /// Check if this batch contains a specific height.
    pub fn contains_height(&self, height: u64) -> bool {
        height >= self.start_height && height <= self.end_height()
    }
}

/// Result of batch validation with truncation support.
#[derive(Debug, Clone)]
pub struct BatchValidationResult {
    /// Number of valid blocks (starting from the first).
    pub valid_count: u32,
    /// First invalid block height (if any).
    pub first_invalid_height: Option<u64>,
    /// Validation error for the first invalid block.
    pub error: Option<BlockValidationError>,
    /// Whether the batch was truncated.
    pub truncated: bool,
}

impl BatchValidationResult {
    /// Create a successful validation result (all blocks valid).
    pub fn success(block_count: u32) -> Self {
        Self {
            valid_count: block_count,
            first_invalid_height: None,
            error: None,
            truncated: false,
        }
    }

    /// Create a truncated validation result.
    pub fn truncated(
        valid_count: u32,
        first_invalid_height: u64,
        error: BlockValidationError,
    ) -> Self {
        Self {
            valid_count,
            first_invalid_height: Some(first_invalid_height),
            error: Some(error),
            truncated: true,
        }
    }

    /// Check if any blocks were accepted.
    pub fn has_valid_blocks(&self) -> bool {
        self.valid_count > 0
    }

    /// Check if all blocks were valid.
    pub fn is_complete(&self) -> bool {
        !self.truncated
    }
}

/// Block validation error types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockValidationError {
    /// Block has an invalid signature.
    InvalidSignature {
        /// Block height.
        height: u64,
        /// Expected signer.
        expected_signer: Option<Vec<u8>>,
    },
    /// Transaction in block is invalid.
    InvalidTransaction {
        /// Block height.
        height: u64,
        /// Transaction index.
        tx_index: usize,
        /// Error details.
        details: String,
    },
    /// State transition is invalid.
    InvalidStateTransition {
        /// Block height.
        height: u64,
        /// Expected state root.
        expected_root: [u8; 32],
        /// Actual state root.
        actual_root: [u8; 32],
    },
    /// Double spend detected.
    DoubleSpend {
        /// Block height.
        height: u64,
        /// UTXO identifier.
        utxo_id: Vec<u8>,
    },
    /// Block header is invalid.
    InvalidBlockHeader {
        /// Block height.
        height: u64,
        /// Error details.
        details: String,
    },
    /// Missing parent block.
    MissingParent {
        /// Block height.
        height: u64,
        /// Expected parent hash.
        expected_parent: [u8; 32],
    },
    /// Height mismatch in contiguous block sequence.
    HeightMismatch {
        /// Expected height.
        expected: u64,
        /// Actual height.
        actual: u64,
    },
    /// Other validation error.
    Other(String),
}

impl std::fmt::Display for BlockValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature { height, .. } => {
                write!(f, "Invalid signature at block {}", height)
            }
            Self::InvalidTransaction { height, tx_index, details } => {
                write!(f, "Invalid tx {} at block {}: {}", tx_index, height, details)
            }
            Self::InvalidStateTransition { height, .. } => {
                write!(f, "Invalid state transition at block {}", height)
            }
            Self::DoubleSpend { height, .. } => {
                write!(f, "Double spend detected at block {}", height)
            }
            Self::InvalidBlockHeader { height, details } => {
                write!(f, "Invalid block header at {}: {}", height, details)
            }
            Self::MissingParent { height, .. } => {
                write!(f, "Missing parent for block {}", height)
            }
            Self::HeightMismatch { expected, actual } => {
                write!(f, "Height mismatch: expected {}, got {}", expected, actual)
            }
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

/// Information about a submitted batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchInfo {
    /// Batch sequence number (incrementing).
    pub batch_number: u64,
    /// First block height in the batch.
    pub start_height: BlockHeight,
    /// Last block height in the batch.
    pub end_height: BlockHeight,
    /// Number of blocks in the batch.
    pub block_count: u32,
    /// Unix timestamp when the batch was created.
    pub timestamp: u64,
}

impl BatchInfo {
    /// Create a new BatchInfo.
    pub fn new(
        batch_number: u64,
        start_height: BlockHeight,
        end_height: BlockHeight,
        block_count: u32,
        timestamp: u64,
    ) -> Self {
        Self {
            batch_number,
            start_height,
            end_height,
            block_count,
            timestamp,
        }
    }
}

/// State commitment for a batch of blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCommitment {
    /// State root before processing the batch.
    pub initial_state_root: [u8; 32],
    /// State root after processing all blocks in the batch.
    pub final_state_root: [u8; 32],
    /// Merkle root of all receipts in the batch.
    pub receipts_root: [u8; 32],
}

impl BatchCommitment {
    /// Create a new BatchCommitment.
    pub fn new(
        initial_state_root: [u8; 32],
        final_state_root: [u8; 32],
        receipts_root: [u8; 32],
    ) -> Self {
        Self {
            initial_state_root,
            final_state_root,
            receipts_root,
        }
    }
}

/// Status of a batch submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmissionStatus {
    /// Batch is pending submission.
    Pending,
    /// Batch has been submitted but not yet confirmed.
    Submitted,
    /// Batch has been confirmed on Tondi L1.
    Confirmed,
    /// Batch submission failed.
    Failed,
}

/// Record of a submitted batch stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRecord {
    /// Batch information.
    pub info: BatchInfo,
    /// State commitment.
    pub commitment: BatchCommitment,
    /// Tondi transaction ID (if submitted).
    pub tondi_tx_id: Option<[u8; 32]>,
    /// Tondi block height where the batch was confirmed.
    pub tondi_block_height: Option<u64>,
    /// Instance ID of the Ingot output.
    pub instance_id: Option<[u8; 32]>,
    /// Submission status.
    pub status: SubmissionStatus,
    /// Parent batch's instance ID (for chain continuity).
    pub parent_instance_id: Option<[u8; 32]>,
}

impl BatchRecord {
    /// Create a new pending batch record.
    pub fn new_pending(info: BatchInfo, commitment: BatchCommitment) -> Self {
        Self {
            info,
            commitment,
            tondi_tx_id: None,
            tondi_block_height: None,
            instance_id: None,
            status: SubmissionStatus::Pending,
            parent_instance_id: None,
        }
    }

    /// Mark the batch as submitted.
    pub fn mark_submitted(
        &mut self,
        tx_id: [u8; 32],
        parent_instance_id: Option<[u8; 32]>,
    ) {
        self.tondi_tx_id = Some(tx_id);
        self.parent_instance_id = parent_instance_id;
        self.status = SubmissionStatus::Submitted;
    }

    /// Mark the batch as confirmed.
    pub fn mark_confirmed(
        &mut self,
        block_height: u64,
        instance_id: [u8; 32],
    ) {
        self.tondi_block_height = Some(block_height);
        self.instance_id = Some(instance_id);
        self.status = SubmissionStatus::Confirmed;
    }

    /// Mark the batch as failed.
    pub fn mark_failed(&mut self) {
        self.status = SubmissionStatus::Failed;
    }

    /// Check if the batch is pending submission.
    pub fn is_pending(&self) -> bool {
        self.status == SubmissionStatus::Pending
    }

    /// Check if the batch is submitted but not confirmed.
    pub fn is_submitted(&self) -> bool {
        self.status == SubmissionStatus::Submitted
    }

    /// Check if the batch is confirmed.
    pub fn is_confirmed(&self) -> bool {
        self.status == SubmissionStatus::Confirmed
    }
}

/// L1 status for a submitted batch.
///
/// Tracks the lifecycle of a batch on Tondi L1:
/// Submitted → InMempool → Included → Finalized
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchL1Status {
    /// Batch has been submitted but status unknown.
    Unknown,
    /// Batch is in the Tondi mempool.
    InMempool {
        /// When the batch entered the mempool.
        since: u64,
    },
    /// Batch is included in a block.
    Included {
        /// Block ID on L1.
        block_id: [u8; 32],
        /// DAA score of the block.
        daa_score: u64,
        /// Confirmations (DAA score difference from tip).
        confirmations: u64,
    },
    /// Batch is finalized (sufficient confirmations).
    Finalized {
        /// Block ID on L1.
        block_id: [u8; 32],
        /// DAA score of the block.
        daa_score: u64,
        /// Instance ID of the Ingot.
        instance_id: [u8; 32],
    },
    /// Batch was dropped from mempool.
    Dropped {
        /// Reason for dropping.
        reason: String,
    },
    /// Batch was rejected by L1.
    Rejected {
        /// Rejection reason.
        reason: String,
    },
    /// Batch was orphaned due to reorg.
    Orphaned {
        /// Original block ID.
        original_block_id: [u8; 32],
        /// Original DAA score.
        original_daa_score: u64,
    },
}

impl BatchL1Status {
    /// Check if the batch is finalized.
    pub fn is_finalized(&self) -> bool {
        matches!(self, BatchL1Status::Finalized { .. })
    }

    /// Check if the batch is included (but not yet finalized).
    pub fn is_included(&self) -> bool {
        matches!(self, BatchL1Status::Included { .. } | BatchL1Status::Finalized { .. })
    }

    /// Check if the batch needs resubmission.
    pub fn needs_resubmission(&self) -> bool {
        matches!(
            self,
            BatchL1Status::Dropped { .. }
                | BatchL1Status::Rejected { .. }
                | BatchL1Status::Orphaned { .. }
        )
    }

    /// Get the DAA score if included.
    pub fn daa_score(&self) -> Option<u64> {
        match self {
            BatchL1Status::Included { daa_score, .. } => Some(*daa_score),
            BatchL1Status::Finalized { daa_score, .. } => Some(*daa_score),
            BatchL1Status::Orphaned { original_daa_score, .. } => Some(*original_daa_score),
            _ => None,
        }
    }
}

/// Pending batch tracker for the sync service.
#[derive(Debug, Clone)]
pub struct PendingBatch {
    /// Batch number.
    pub batch_number: u64,
    /// Transaction ID on L1.
    pub tx_id: [u8; 32],
    /// When the batch was submitted (for timeout tracking).
    pub submitted_at: Instant,
    /// Current L1 status.
    pub l1_status: BatchL1Status,
    /// Number of resubmission attempts.
    pub resubmit_count: u8,
}

impl PendingBatch {
    /// Create a new pending batch tracker.
    pub fn new(batch_number: u64, tx_id: [u8; 32]) -> Self {
        Self {
            batch_number,
            tx_id,
            submitted_at: Instant::now(),
            l1_status: BatchL1Status::Unknown,
            resubmit_count: 0,
        }
    }

    /// Mark as in mempool.
    pub fn mark_in_mempool(&mut self) {
        self.l1_status = BatchL1Status::InMempool {
            since: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
    }

    /// Mark as included in a block.
    pub fn mark_included(&mut self, block_id: [u8; 32], daa_score: u64, confirmations: u64) {
        self.l1_status = BatchL1Status::Included {
            block_id,
            daa_score,
            confirmations,
        };
    }

    /// Mark as finalized.
    pub fn mark_finalized(&mut self, block_id: [u8; 32], daa_score: u64, instance_id: [u8; 32]) {
        self.l1_status = BatchL1Status::Finalized {
            block_id,
            daa_score,
            instance_id,
        };
    }

    /// Mark as orphaned.
    pub fn mark_orphaned(&mut self, block_id: [u8; 32], daa_score: u64) {
        self.l1_status = BatchL1Status::Orphaned {
            original_block_id: block_id,
            original_daa_score: daa_score,
        };
        self.resubmit_count = self.resubmit_count.saturating_add(1);
    }

    /// Check if the batch has timed out.
    pub fn has_timed_out(&self, timeout: std::time::Duration) -> bool {
        self.submitted_at.elapsed() > timeout
    }
}

