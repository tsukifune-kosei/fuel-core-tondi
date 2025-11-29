//! Payload construction for Tondi Ingot transactions (Sovereign Rollup).
//!
//! ## Ingot Protocol Integration
//!
//! FuelVM batches are stored using the standard Ingot protocol:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  INGOT OUTPUT (ScriptPublicKey.script) - L1 Commitment      │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │  IngotOutput (Borsh serialized, ~100-200 bytes)         ││
//! │  │  - version: u8 = 0x01                                   ││
//! │  │  - schema_id: [u8; 32]  (FuelVM batch schema)           ││
//! │  │  - hash_payload: [u8; 32]  (blake3 of witness payload)  ││
//! │  │  - flags: u16  (REVEAL_REQUIRED etc)                    ││
//! │  │  - lock: Lock::PubKey { sequencer_pubkey }              ││
//! │  │  - mast_root: Option<Hash>                              ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//!
//! ┌─────────────────────────────────────────────────────────────┐
//! │  INGOT WITNESS (花费时) - Large data payload                │
//! │  ┌─────────────────────────────────────────────────────────┐│
//! │  │  IngotWitness                                           ││
//! │  │  - payload: FuelBatchPayload (TLV encoded, ≤85KB)       ││
//! │  │  - auth_sigs: [sequencer_signature] (64B Schnorr)       ││
//! │  │  - script_reveal: None                                  ││
//! │  │  - mast_proof: None                                     ││
//! │  └─────────────────────────────────────────────────────────┘│
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## FuelBatchPayload Structure (inside IngotWitness.payload)
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │  FuelBatchHeader (45 bytes, fixed, at start for fast scan)  │
//! │  - version: u8                                              │
//! │  - start_height: u64                                        │
//! │  - block_count: u32                                         │
//! │  - parent_hash: [u8; 32]  (L2 chain continuity)             │
//! ├─────────────────────────────────────────────────────────────┤
//! │  TLV Fields                                                 │
//! │  - FUEL_BLOCKS: compressed Fuel block data                  │
//! │  - FUEL_COMMITMENT: state roots (for ZK/bridges)            │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Security Model (Sovereign Mode)
//! - **Authentication**: Lock = PubKey { sequencer_pubkey }, signature in auth_sigs
//! - **Chain Continuity**: parent_hash in FuelBatchHeader (L2 fork prevention)
//! - **Data Binding**: hash_payload = blake3(FuelBatchPayload) in IngotOutput

use crate::{
    error::{
        Result,
        TondiAdapterError,
    },
    types::{
        BatchCommitment,
        BatchHeader,
        BatchValidationResult,
        BlockValidationError,
    },
    Config,
};
use fuel_core_types::blockchain::SealedBlock;
use serde::{
    Deserialize,
    Serialize,
};

/// TLV type constants for FuelVM batch payload (inside IngotWitness.payload).
pub mod tlv_types {
    /// FuelVM compressed block data.
    pub const TLV_FUEL_BLOCKS: u16 = 0x1001;
    /// FuelVM state commitment (state roots for ZK proofs / light clients).
    pub const TLV_FUEL_COMMITMENT: u16 = 0x1002;
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
            prev_block_hash: *consensus_header.prev_root,
            transactions_root: *header.transactions_root(),
            transactions_data,
            timestamp: consensus_header.time.0,
        })
    }

    /// Estimate the serialized size.
    pub fn estimated_size(&self) -> usize {
        // Fixed fields: height(8) + hashes(32*3) + timestamp(8) = 112
        // Plus transactions data length prefix (4) + data
        112_usize
            .saturating_add(4)
            .saturating_add(self.transactions_data.len())
    }
}

/// Complete FuelVM block batch payload for Sovereign Rollup.
///
/// This entire structure goes into `IngotWitness.payload`.
/// The `IngotOutput.hash_payload` commits to `blake3(this_serialized)`.
///
/// ## Wire Format
/// ```text
/// ┌─────────────────────────────────────────────────────────────┐
/// │  BatchHeader (45 bytes, fixed, at start for fast parsing)   │
/// │  - version: u8                                              │
/// │  - start_height: u64                                        │
/// │  - block_count: u32                                         │
/// │  - parent_hash: [u8; 32]                                    │
/// ├─────────────────────────────────────────────────────────────┤
/// │  TLV Fields (variable size)                                 │
/// │  - TLV_FUEL_BLOCKS: compressed block data                   │
/// │  - TLV_FUEL_COMMITMENT: state roots                         │
/// └─────────────────────────────────────────────────────────────┘
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuelBlockBatchPayload {
    /// Batch header (placed at start for quick validation).
    pub header: BatchHeader,
    /// Block data list.
    pub blocks: Vec<FuelBlockData>,
    /// State commitment (for ZK proofs and light client bridges).
    pub commitment: BatchCommitment,
}

impl FuelBlockBatchPayload {
    /// Estimate the serialized size of the entire payload.
    pub fn estimated_size(&self) -> usize {
        // BatchHeader: 45 bytes (fixed)
        // BatchCommitment: 96 bytes (3 * 32)
        // TLV overhead per field: 6 bytes (type: 2, length: 4)
        let header_size = BatchHeader::SERIALIZED_SIZE;
        let commitment_size = 96_usize.saturating_add(6);
        let blocks_size: usize = self.blocks.iter().map(|b| b.estimated_size()).sum();
        let blocks_tlv_overhead = 6_usize;

        header_size
            .saturating_add(commitment_size)
            .saturating_add(blocks_size)
            .saturating_add(blocks_tlv_overhead)
    }

    /// Get the batch header.
    pub fn get_header(&self) -> &BatchHeader {
        &self.header
    }

    /// Get the end height (calculated from header).
    pub fn end_height(&self) -> u64 {
        self.header.end_height()
    }

    /// Validate the header against expected parent hash.
    ///
    /// This is a quick check for L2 chain continuity.
    pub fn validate_header(&self, expected_parent_hash: Option<&[u8; 32]>) -> bool {
        // Check header consistency
        if !self.header.is_valid() {
            return false;
        }

        // Check parent hash if provided (L2 chain continuity)
        if expected_parent_hash.is_some_and(|expected| !self.header.follows_parent(expected)) {
            return false;
        }

        // Check block count matches actual blocks
        let block_count = u32::try_from(self.blocks.len()).unwrap_or(u32::MAX);
        if self.header.block_count != block_count {
            return false;
        }

        true
    }

    /// Validate all blocks in the payload and return a truncation result.
    ///
    /// This implements the truncation strategy:
    /// - Validates blocks sequentially from the first
    /// - Stops at the first invalid block
    /// - Returns which blocks are valid and can be accepted
    ///
    /// # Arguments
    /// * `validator` - Function to validate each block, returns Ok(()) or Err with error
    pub fn validate_with_truncation<F>(
        &self,
        mut validator: F,
    ) -> BatchValidationResult
    where
        F: FnMut(&FuelBlockData, usize) -> std::result::Result<(), BlockValidationError>,
    {
        let mut valid_count = 0u32;

        for (idx, block) in self.blocks.iter().enumerate() {
            match validator(block, idx) {
                Ok(()) => {
                    valid_count = valid_count.saturating_add(1);
                }
                Err(error) => {
                    // Found first invalid block - truncate here
                    return BatchValidationResult::truncated(
                        valid_count,
                        block.height,
                        error,
                    );
                }
            }
        }

        // All blocks valid
        BatchValidationResult::success(valid_count)
    }

    /// Get the valid blocks based on a validation result.
    ///
    /// Returns a slice of blocks up to (but not including) the first invalid one.
    pub fn get_valid_blocks(&self, result: &BatchValidationResult) -> &[FuelBlockData] {
        let count = std::cmp::min(result.valid_count as usize, self.blocks.len());
        &self.blocks[..count]
    }

    /// Extract valid blocks, consuming the payload.
    ///
    /// Returns only the valid blocks based on the validation result.
    pub fn into_valid_blocks(mut self, result: &BatchValidationResult) -> Vec<FuelBlockData> {
        let count = std::cmp::min(result.valid_count as usize, self.blocks.len());
        self.blocks.truncate(count);
        self.blocks
    }
}

/// Builder for FuelVM batch payloads (Sovereign Rollup).
pub struct PayloadBuilder {
    config: Config,
    blocks: Vec<SealedBlock>,
}

impl PayloadBuilder {
    /// Create a new PayloadBuilder.
    pub fn new(config: Config) -> Self {
        Self {
            config,
            blocks: Vec::new(),
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

    /// Build the batch payload.
    ///
    /// The payload contains:
    /// - BatchHeader (for quick L2 indexer validation)
    /// - Block data (compressed)
    /// - State commitments
    ///
    /// Note: Sequencer authentication is handled by:
    /// - IngotOutput.lock = PubKey { sequencer_pubkey }
    /// - IngotWitness.auth_sigs = [sequencer_signature]
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
            let prev_height: u32 = *window[0].entity.header().consensus().height;
            let curr_height: u32 = *window[1].entity.header().consensus().height;
            if curr_height != prev_height.saturating_add(1) {
                return Err(TondiAdapterError::InvalidBlockData(format!(
                    "Non-contiguous blocks: {} -> {}",
                    prev_height, curr_height
                )));
            }
        }

        let first_block = blocks.first().unwrap();
        let start_height: u64 = (*first_block.entity.header().consensus().height).into();
        let block_count = u32::try_from(blocks.len()).unwrap_or(u32::MAX);

        // Build block data list
        let block_data: Result<Vec<FuelBlockData>> = blocks
            .iter()
            .map(FuelBlockData::from_sealed_block)
            .collect();
        let block_data = block_data?;

        // Get parent hash from first block (L2 chain continuity)
        let parent_hash = block_data
            .first()
            .map(|b| b.prev_block_hash)
            .unwrap_or([0u8; 32]);

        // Build state commitment
        let commitment = Self::compute_commitment(&blocks)?;

        // Build header
        let header = BatchHeader::new(start_height, block_count, parent_hash);

        let payload = FuelBlockBatchPayload {
            header,
            blocks: block_data,
            commitment,
        };

        // Check size limit (Ingot Witness payload should be ≤85KB)
        let estimated_size = payload.estimated_size();
        let max_size = (self.config.max_batch_size as usize)
            .saturating_mul(10)
            .saturating_mul(1024);
        if estimated_size > max_size {
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
        let initial_state_root = *first_block.entity.header().consensus().prev_root;

        // Get final state root from last block's transactions root (as proxy)
        // Note: In a full implementation, this would be the actual state root
        let final_state_root = *last_block.entity.header().transactions_root();

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
            .map(|b| *b.entity.header().message_outbox_root())
            .collect();

        if roots.is_empty() {
            return [0u8; 32];
        }

        // Simple Merkle root computation
        Self::merkle_root(&roots)
    }

    /// Compute Merkle root of a list of hashes.
    ///
    /// # Security (CVE-2012-2459 Prevention)
    ///
    /// This implementation prevents the Merkle tree malleability vulnerability
    /// by including the list length in the final hash computation:
    ///
    /// `final_root = H(length_le_bytes || merkle_root)`
    ///
    /// This ensures that lists of different lengths produce different roots,
    /// even if the underlying tree structure is similar (e.g., [A,B,C] vs [A,B,C,C]).
    fn merkle_root(hashes: &[[u8; 32]]) -> [u8; 32] {
        if hashes.is_empty() {
            return [0u8; 32];
        }
        if hashes.len() == 1 {
            // For single element: H(length=1 || hash)
            let mut data = [0u8; 40];
            data[..8].copy_from_slice(&1u64.to_le_bytes());
            data[8..].copy_from_slice(&hashes[0]);
            return *blake3::hash(&data).as_bytes();
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
                    // Odd element: hash with itself (standard approach)
                    let mut data = [0u8; 64];
                    data[..32].copy_from_slice(&chunk[0]);
                    data[32..].copy_from_slice(&chunk[0]);
                    *blake3::hash(&data).as_bytes()
                };
                next_level.push(combined);
            }

            current_level = next_level;
        }

        // CVE-2012-2459 fix: Include original length in final hash
        // This prevents [A,B,C] and [A,B,C,C] from having the same root
        let tree_root = current_level[0];
        let original_len = hashes.len() as u64;
        let mut final_data = [0u8; 40];
        final_data[..8].copy_from_slice(&original_len.to_le_bytes());
        final_data[8..].copy_from_slice(&tree_root);
        *blake3::hash(&final_data).as_bytes()
    }

    /// Encode the complete FuelBatchPayload for IngotWitness.payload.
    ///
    /// Format:
    /// 1. BatchHeader (45 bytes, fixed)
    /// 2. TLV fields (FUEL_BLOCKS, FUEL_COMMITMENT)
    pub fn encode(payload: &FuelBlockBatchPayload) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();

        // Write BatchHeader first (fixed size, for quick parsing)
        buffer.extend_from_slice(&payload.header.to_bytes());

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

    /// Decode just the BatchHeader from the beginning of a payload.
    ///
    /// This is a fast operation for L2 indexers to check batch relevance
    /// before full deserialization.
    pub fn decode_header_only(data: &[u8]) -> Option<BatchHeader> {
        BatchHeader::from_bytes(data)
    }

    /// Decode the complete FuelBatchPayload from IngotWitness.payload.
    pub fn decode(data: &[u8]) -> Result<FuelBlockBatchPayload> {
        // First, extract the header
        let header = BatchHeader::from_bytes(data).ok_or_else(|| {
            TondiAdapterError::Deserialization("Invalid batch header".to_string())
        })?;

        // Parse TLV fields after the header
        let tlv_start = BatchHeader::SERIALIZED_SIZE;
        if data.len() < tlv_start {
            return Err(TondiAdapterError::Deserialization(
                "Payload too short".to_string(),
            ));
        }

        let mut offset = tlv_start;
        let mut blocks = None;
        let mut commitment = None;

        // TLV header is 6 bytes: type(2) + length(4)
        const TLV_HEADER_SIZE: usize = 6;

        while let Some(remaining) = data.len().checked_sub(offset) {
            if remaining < TLV_HEADER_SIZE {
                break;
            }

            let tlv_type = u16::from_le_bytes([data[offset], data[offset.saturating_add(1)]]);
            let tlv_len = u32::from_le_bytes([
                data[offset.saturating_add(2)],
                data[offset.saturating_add(3)],
                data[offset.saturating_add(4)],
                data[offset.saturating_add(5)],
            ]) as usize;

            offset = offset.saturating_add(TLV_HEADER_SIZE);

            let value_end = offset.saturating_add(tlv_len);
            if value_end > data.len() {
                break;
            }

            let value = &data[offset..value_end];

            match tlv_type {
                tlv_types::TLV_FUEL_BLOCKS => {
                    blocks = Some(
                        postcard::from_bytes(value)
                            .map_err(|e| TondiAdapterError::Deserialization(e.to_string()))?,
                    );
                }
                tlv_types::TLV_FUEL_COMMITMENT => {
                    commitment = Some(
                        postcard::from_bytes(value)
                            .map_err(|e| TondiAdapterError::Deserialization(e.to_string()))?,
                    );
                }
                _ => {
                    // Skip unknown TLV types (forward compatibility)
                }
            }

            offset = value_end;
        }

        Ok(FuelBlockBatchPayload {
            header,
            blocks: blocks.ok_or_else(|| {
                TondiAdapterError::Deserialization("Missing blocks".to_string())
            })?,
            commitment: commitment.ok_or_else(|| {
                TondiAdapterError::Deserialization("Missing commitment".to_string())
            })?,
        })
    }

    /// Compute the hash_payload for IngotOutput.
    ///
    /// This is what L1 commits to: blake3(entire FuelBatchPayload)
    pub fn compute_hash_payload(payload: &FuelBlockBatchPayload) -> Result<[u8; 32]> {
        let bytes = Self::encode(payload)?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }

    /// Write a TLV field to the buffer.
    fn write_tlv(buffer: &mut Vec<u8>, tlv_type: u16, value: &[u8]) {
        buffer.extend_from_slice(&tlv_type.to_le_bytes());
        let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
        buffer.extend_from_slice(&len.to_le_bytes());
        buffer.extend_from_slice(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_root_single() {
        let hash = [1u8; 32];
        let root = PayloadBuilder::merkle_root(&[hash]);

        // Single element root includes length prefix: H(1 || hash)
        let mut expected_data = [0u8; 40];
        expected_data[..8].copy_from_slice(&1u64.to_le_bytes());
        expected_data[8..].copy_from_slice(&hash);
        let expected = *blake3::hash(&expected_data).as_bytes();

        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_pair() {
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];

        // Tree root: H(hash1 || hash2)
        let mut combined = [0u8; 64];
        combined[..32].copy_from_slice(&hash1);
        combined[32..].copy_from_slice(&hash2);
        let tree_root = *blake3::hash(&combined).as_bytes();

        // Final root includes length: H(2 || tree_root)
        let mut final_data = [0u8; 40];
        final_data[..8].copy_from_slice(&2u64.to_le_bytes());
        final_data[8..].copy_from_slice(&tree_root);
        let expected = *blake3::hash(&final_data).as_bytes();

        let root = PayloadBuilder::merkle_root(&[hash1, hash2]);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_empty() {
        let root = PayloadBuilder::merkle_root(&[]);
        assert_eq!(root, [0u8; 32]);
    }

    #[test]
    fn test_merkle_root_deterministic() {
        // Same inputs should always produce the same root
        let hashes = [[1u8; 32], [2u8; 32], [3u8; 32]];
        let root1 = PayloadBuilder::merkle_root(&hashes);
        let root2 = PayloadBuilder::merkle_root(&hashes);
        assert_eq!(root1, root2);
    }

    #[test]
    fn test_merkle_root_cve_2012_2459_prevention() {
        // CVE-2012-2459: [A, B, C] should NOT produce the same root as [A, B, C, C]
        // Our implementation includes the list length in the final hash, so these
        // MUST be different even if the tree structure is similar.
        let a = [1u8; 32];
        let b = [2u8; 32];
        let c = [3u8; 32];

        let root_abc = PayloadBuilder::merkle_root(&[a, b, c]);
        let root_abcc = PayloadBuilder::merkle_root(&[a, b, c, c]);

        // These MUST be different to prevent the vulnerability
        assert_ne!(
            root_abc, root_abcc,
            "CVE-2012-2459: [A,B,C] and [A,B,C,C] must have different roots!"
        );

        // Also test: [A, B] vs [A, B, B] (should also differ)
        let root_ab = PayloadBuilder::merkle_root(&[a, b]);
        let root_abb = PayloadBuilder::merkle_root(&[a, b, b]);
        assert_ne!(
            root_ab, root_abb,
            "CVE-2012-2459: [A,B] and [A,B,B] must have different roots!"
        );

        // Test with identical consecutive elements
        let root_aa = PayloadBuilder::merkle_root(&[a, a]);
        let root_aaa = PayloadBuilder::merkle_root(&[a, a, a]);
        assert_ne!(
            root_aa, root_aaa,
            "CVE-2012-2459: [A,A] and [A,A,A] must have different roots!"
        );
    }

    #[test]
    fn test_batch_header_serialization() {
        let header = BatchHeader::new(
            100,         // start_height
            6,           // block_count
            [0xaa; 32],  // parent_hash
        );

        // Serialize
        let bytes = header.to_bytes();
        assert_eq!(bytes.len(), BatchHeader::SERIALIZED_SIZE);
        assert_eq!(bytes.len(), 45); // 1 + 8 + 4 + 32 = 45 bytes

        // Deserialize
        let decoded = BatchHeader::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.version, BatchHeader::VERSION);
        assert_eq!(decoded.start_height, 100);
        assert_eq!(decoded.block_count, 6);
        assert_eq!(decoded.end_height(), 105); // Calculated
        assert_eq!(decoded.parent_hash, [0xaa; 32]);
    }

    #[test]
    fn test_batch_header_validation() {
        // Valid header
        let valid = BatchHeader::new(100, 6, [0; 32]);
        assert!(valid.is_valid());
        assert_eq!(valid.end_height(), 105);

        // Invalid: block count = 0
        let mut invalid = BatchHeader::new(100, 1, [0; 32]);
        invalid.block_count = 0;
        assert!(!invalid.is_valid());

        // Invalid: version = 0
        let mut invalid_ver = BatchHeader::new(100, 1, [0; 32]);
        invalid_ver.version = 0;
        assert!(!invalid_ver.is_valid());
    }

    #[test]
    fn test_batch_header_parent_check() {
        let parent_hash = [0xaa; 32];
        let header = BatchHeader::new(100, 6, parent_hash);

        assert!(header.follows_parent(&parent_hash));
        assert!(!header.follows_parent(&[0xbb; 32]));
    }

    #[test]
    fn test_batch_header_contains_height() {
        let header = BatchHeader::new(100, 6, [0; 32]);

        assert!(header.contains_height(100));
        assert!(header.contains_height(102));
        assert!(header.contains_height(105)); // end_height = 100 + 6 - 1 = 105
        assert!(!header.contains_height(99));
        assert!(!header.contains_height(106));
    }

    #[test]
    fn test_batch_validation_result_success() {
        let result = BatchValidationResult::success(10);
        assert!(result.has_valid_blocks());
        assert!(result.is_complete());
        assert_eq!(result.valid_count, 10);
        assert!(!result.truncated);
    }

    #[test]
    fn test_batch_validation_result_truncated() {
        let error = BlockValidationError::InvalidSignature {
            height: 102,
            expected_signer: None,
        };
        let result = BatchValidationResult::truncated(2, 102, error);

        assert!(result.has_valid_blocks());
        assert!(!result.is_complete());
        assert_eq!(result.valid_count, 2);
        assert!(result.truncated);
        assert_eq!(result.first_invalid_height, Some(102));
        assert!(result.error.is_some());
    }

    #[test]
    fn test_header_from_bytes() {
        let header = BatchHeader::new(100, 6, [0xaa; 32]);
        let bytes = header.to_bytes();

        // Add some extra data (simulating rest of payload)
        let mut full_data = bytes.to_vec();
        full_data.extend_from_slice(&[0u8; 100]);

        // Should decode just the header
        let decoded = PayloadBuilder::decode_header_only(&full_data).unwrap();
        assert_eq!(decoded.start_height, 100);
        assert_eq!(decoded.end_height(), 105);
    }

    #[test]
    fn test_payload_hash() {
        // Create a minimal payload
        let header = BatchHeader::new(100, 1, [0; 32]);
        let payload = FuelBlockBatchPayload {
            header,
            blocks: vec![],
            commitment: BatchCommitment {
                initial_state_root: [1u8; 32],
                final_state_root: [2u8; 32],
                receipts_root: [3u8; 32],
            },
        };

        // Compute hash_payload (for IngotOutput)
        let hash1 = PayloadBuilder::compute_hash_payload(&payload).unwrap();
        let hash2 = PayloadBuilder::compute_hash_payload(&payload).unwrap();

        // Should be deterministic
        assert_eq!(hash1, hash2);

        // Should not be all zeros
        assert_ne!(hash1, [0u8; 32]);
    }

    #[test]
    fn test_payload_encode_decode_roundtrip() {
        let header = BatchHeader::new(100, 1, [0xaa; 32]);
        let payload = FuelBlockBatchPayload {
            header,
            blocks: vec![],
            commitment: BatchCommitment {
                initial_state_root: [1u8; 32],
                final_state_root: [2u8; 32],
                receipts_root: [3u8; 32],
            },
        };

        // Encode
        let encoded = PayloadBuilder::encode(&payload).unwrap();

        // Decode
        let decoded = PayloadBuilder::decode(&encoded).unwrap();

        // Verify
        assert_eq!(decoded.header.start_height, 100);
        assert_eq!(decoded.header.block_count, 1);
        assert_eq!(decoded.header.parent_hash, [0xaa; 32]);
        assert_eq!(decoded.commitment.initial_state_root, [1u8; 32]);
    }
}

