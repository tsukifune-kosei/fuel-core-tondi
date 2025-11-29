//! Core types for the Tondi Ingot Adapter.

use fuel_core_types::fuel_types::BlockHeight;
use serde::{
    Deserialize,
    Serialize,
};

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
}


