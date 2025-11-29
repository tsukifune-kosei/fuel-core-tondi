//! Core types for the Tondi Ingot Adapter.

use fuel_core_types::fuel_types::BlockHeight;
use serde::{
    Deserialize,
    Serialize,
};
use std::time::Instant;

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

