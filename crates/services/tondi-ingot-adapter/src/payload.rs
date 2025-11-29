//! Payload construction for Tondi Ingot transactions.
//!
//! This module builds TLV-encoded payloads for FuelVM block batches.

use crate::{
    error::{
        Result,
        TondiAdapterError,
    },
    types::{
        BatchCommitment,
        BatchInfo,
    },
    Config,
};
use fuel_core_types::blockchain::SealedBlock;
use serde::{
    Deserialize,
    Serialize,
};

/// TLV type constants for FuelVM batch payloads.
pub mod tlv_types {
    /// Operation kind (Mint/Transfer).
    pub const TLV_OP_KIND: u16 = 0x0110;
    /// Parent reference (previous batch instance_id).
    pub const TLV_PARENT_REF: u16 = 0x0111;
    /// FuelVM batch info.
    pub const TLV_FUEL_BATCH_INFO: u16 = 0x1001;
    /// FuelVM block data list.
    pub const TLV_FUEL_BLOCKS: u16 = 0x1002;
    /// FuelVM state commitment.
    pub const TLV_FUEL_COMMITMENT: u16 = 0x1003;
}

/// Operation kind for Ingot transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpKind {
    /// Create new artifact (genesis batch).
    Mint = 0,
    /// Transfer/update existing artifact.
    Transfer = 1,
}

impl OpKind {
    /// Convert to byte representation.
    pub fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Serialized FuelVM block data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelBlockData {
    /// Block height.
    pub height: u64,
    /// Block hash.
    pub block_hash: [u8; 32],
    /// Previous block hash.
    pub prev_block_hash: [u8; 32],
    /// Transactions root.
    pub transactions_root: [u8; 32],
    /// Compressed transactions data.
    pub transactions_data: Vec<u8>,
    /// Block timestamp.
    pub timestamp: u64,
}

impl FuelBlockData {
    /// Create FuelBlockData from a SealedBlock.
    pub fn from_sealed_block(block: &SealedBlock) -> Result<Self> {
        let header = block.entity.header();
        let consensus_header = header.consensus();

        // Serialize transactions
        let transactions_data = postcard::to_allocvec(block.entity.transactions())
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))?;

        // Get block ID bytes
        let block_id = block.entity.id();
        let block_hash: [u8; 32] = block_id.into();

        Ok(Self {
            height: (*consensus_header.height).into(),
            block_hash,
            prev_block_hash: (*consensus_header.prev_root).into(),
            transactions_root: (*header.transactions_root()).into(),
            transactions_data,
            timestamp: consensus_header.time.0,
        })
    }

    /// Estimate the serialized size.
    pub fn estimated_size(&self) -> usize {
        // Fixed fields: height(8) + hashes(32*3) + timestamp(8) = 112
        // Plus transactions data length prefix (4) + data
        112 + 4 + self.transactions_data.len()
    }
}

/// Complete FuelVM block batch payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelBlockBatchPayload {
    /// Batch information.
    pub batch_info: BatchInfo,
    /// Block data list.
    pub blocks: Vec<FuelBlockData>,
    /// State commitment.
    pub commitment: BatchCommitment,
    /// Parent batch instance ID (None for genesis batch).
    pub parent_ref: Option<[u8; 32]>,
}

impl FuelBlockBatchPayload {
    /// Estimate the serialized size of the payload.
    pub fn estimated_size(&self) -> usize {
        // BatchInfo: ~40 bytes
        // BatchCommitment: 96 bytes (3 * 32)
        // ParentRef: 32 bytes if present
        // TLV overhead per field: 6 bytes (type: 2, length: 4)
        let base_size = 40 + 96 + 6 * 4; // 4 TLV fields
        let parent_size = if self.parent_ref.is_some() { 32 + 6 } else { 0 };
        let blocks_size: usize = self.blocks.iter().map(|b| b.estimated_size()).sum();

        base_size + parent_size + blocks_size
    }
}

/// Builder for FuelVM batch payloads.
pub struct PayloadBuilder {
    config: Config,
    batch_number: u64,
    blocks: Vec<SealedBlock>,
    parent_ref: Option<[u8; 32]>,
}

impl PayloadBuilder {
    /// Create a new PayloadBuilder.
    pub fn new(config: Config, batch_number: u64) -> Self {
        Self {
            config,
            batch_number,
            blocks: Vec::new(),
            parent_ref: None,
        }
    }

    /// Add a block to the batch.
    pub fn add_block(mut self, block: SealedBlock) -> Self {
        self.blocks.push(block);
        self
    }

    /// Add multiple blocks to the batch.
    pub fn add_blocks(mut self, blocks: impl IntoIterator<Item = SealedBlock>) -> Self {
        self.blocks.extend(blocks);
        self
    }

    /// Set the parent reference (previous batch instance_id).
    pub fn with_parent_ref(mut self, parent_ref: [u8; 32]) -> Self {
        self.parent_ref = Some(parent_ref);
        self
    }

    /// Build the batch payload.
    pub fn build(self) -> Result<FuelBlockBatchPayload> {
        if self.blocks.is_empty() {
            return Err(TondiAdapterError::InvalidBlockData(
                "No blocks to include in batch".to_string(),
            ));
        }

        // Sort blocks by height
        let mut blocks = self.blocks;
        blocks.sort_by_key(|b| b.entity.header().consensus().height);

        // Validate block continuity
        for window in blocks.windows(2) {
            let prev_height: u32 = (*window[0].entity.header().consensus().height).into();
            let curr_height: u32 = (*window[1].entity.header().consensus().height).into();
            if curr_height != prev_height.saturating_add(1) {
                return Err(TondiAdapterError::InvalidBlockData(format!(
                    "Non-contiguous blocks: {} -> {}",
                    prev_height, curr_height
                )));
            }
        }

        let first_block = blocks.first().unwrap();
        let last_block = blocks.last().unwrap();

        // Build batch info
        let batch_info = BatchInfo::new(
            self.batch_number,
            first_block.entity.header().consensus().height,
            last_block.entity.header().consensus().height,
            blocks.len() as u32,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );

        // Build block data list
        let block_data: Result<Vec<FuelBlockData>> = blocks
            .iter()
            .map(FuelBlockData::from_sealed_block)
            .collect();
        let block_data = block_data?;

        // Build state commitment
        let commitment = Self::compute_commitment(&blocks)?;

        let payload = FuelBlockBatchPayload {
            batch_info,
            blocks: block_data,
            commitment,
            parent_ref: self.parent_ref,
        };

        // Check size limit
        let estimated_size = payload.estimated_size();
        if estimated_size > self.config.max_batch_size as usize * 10 * 1024 {
            // ~10KB per block estimate
            return Err(TondiAdapterError::PayloadTooLarge {
                size: estimated_size,
                max: Config::MAX_RECOMMENDED_PAYLOAD_SIZE,
            });
        }

        Ok(payload)
    }

    /// Compute the state commitment for a batch of blocks.
    fn compute_commitment(blocks: &[SealedBlock]) -> Result<BatchCommitment> {
        let first_block = blocks.first().ok_or_else(|| {
            TondiAdapterError::InvalidBlockData("Empty block list".to_string())
        })?;
        let last_block = blocks.last().ok_or_else(|| {
            TondiAdapterError::InvalidBlockData("Empty block list".to_string())
        })?;

        // Get initial state root from first block's prev_root
        let initial_state_root =
            (*first_block.entity.header().consensus().prev_root).into();

        // Get final state root from last block's transactions root (as proxy)
        // Note: In a full implementation, this would be the actual state root
        let final_state_root = (*last_block.entity.header().transactions_root()).into();

        // Compute receipts root (Merkle root of all block message outbox roots)
        let receipts_root = Self::compute_receipts_root(blocks);

        Ok(BatchCommitment::new(
            initial_state_root,
            final_state_root,
            receipts_root,
        ))
    }

    /// Compute Merkle root of all message outputs in the batch.
    fn compute_receipts_root(blocks: &[SealedBlock]) -> [u8; 32] {
        // Collect all message outbox roots (as proxy for receipts)
        let roots: Vec<[u8; 32]> = blocks
            .iter()
            .map(|b| (*b.entity.header().message_outbox_root()).into())
            .collect();

        if roots.is_empty() {
            return [0u8; 32];
        }

        // Simple Merkle root computation
        Self::merkle_root(&roots)
    }

    /// Compute Merkle root of a list of hashes.
    fn merkle_root(hashes: &[[u8; 32]]) -> [u8; 32] {
        if hashes.is_empty() {
            return [0u8; 32];
        }
        if hashes.len() == 1 {
            return hashes[0];
        }

        let mut current_level = hashes.to_vec();

        while current_level.len() > 1 {
            let mut next_level = Vec::new();

            for chunk in current_level.chunks(2) {
                let combined = if chunk.len() == 2 {
                    let mut data = [0u8; 64];
                    data[..32].copy_from_slice(&chunk[0]);
                    data[32..].copy_from_slice(&chunk[1]);
                    *blake3::hash(&data).as_bytes()
                } else {
                    // Odd element, promote directly
                    chunk[0]
                };
                next_level.push(combined);
            }

            current_level = next_level;
        }

        current_level[0]
    }

    /// Encode the payload as TLV bytes.
    pub fn encode_tlv(payload: &FuelBlockBatchPayload) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();

        // Determine operation kind
        let op_kind = if payload.parent_ref.is_some() {
            OpKind::Transfer
        } else {
            OpKind::Mint
        };

        // Write OP_KIND
        Self::write_tlv(
            &mut buffer,
            tlv_types::TLV_OP_KIND,
            &[op_kind.to_byte()],
        );

        // Write PARENT_REF if present
        if let Some(ref parent) = payload.parent_ref {
            Self::write_tlv(&mut buffer, tlv_types::TLV_PARENT_REF, parent);
        }

        // Write FUEL_BATCH_INFO
        let batch_info_bytes = postcard::to_allocvec(&payload.batch_info)
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))?;
        Self::write_tlv(&mut buffer, tlv_types::TLV_FUEL_BATCH_INFO, &batch_info_bytes);

        // Write FUEL_BLOCKS
        let blocks_bytes = postcard::to_allocvec(&payload.blocks)
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))?;
        Self::write_tlv(&mut buffer, tlv_types::TLV_FUEL_BLOCKS, &blocks_bytes);

        // Write FUEL_COMMITMENT
        let commitment_bytes = postcard::to_allocvec(&payload.commitment)
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))?;
        Self::write_tlv(
            &mut buffer,
            tlv_types::TLV_FUEL_COMMITMENT,
            &commitment_bytes,
        );

        Ok(buffer)
    }

    /// Write a TLV field to the buffer.
    fn write_tlv(buffer: &mut Vec<u8>, tlv_type: u16, value: &[u8]) {
        buffer.extend_from_slice(&tlv_type.to_le_bytes());
        buffer.extend_from_slice(&(value.len() as u32).to_le_bytes());
        buffer.extend_from_slice(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_op_kind_bytes() {
        assert_eq!(OpKind::Mint.to_byte(), 0);
        assert_eq!(OpKind::Transfer.to_byte(), 1);
    }

    #[test]
    fn test_merkle_root_single() {
        let hash = [1u8; 32];
        let root = PayloadBuilder::merkle_root(&[hash]);
        assert_eq!(root, hash);
    }

    #[test]
    fn test_merkle_root_pair() {
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];

        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(&hash1);
        combined[32..].copy_from_slice(&hash2);
        let expected = *blake3::hash(&combined).as_bytes();

        let root = PayloadBuilder::merkle_root(&[hash1, hash2]);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_empty() {
        let root = PayloadBuilder::merkle_root(&[]);
        assert_eq!(root, [0u8; 32]);
    }
}

