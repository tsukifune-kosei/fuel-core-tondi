//! Storage layer for the Tondi Ingot Adapter.
//!
//! This module defines the database operations for tracking batch submissions.

use crate::{
    error::{
        Result,
        TondiAdapterError,
    },
    ports::TondiSubmissionDatabase,
    types::{
        BatchRecord,
        SubmissionStatus,
    },
};
use fuel_core_types::fuel_types::BlockHeight;
use std::{
    collections::HashMap,
    sync::RwLock,
};

/// Metadata keys for the Tondi submission database.
pub mod metadata_keys {
    /// Last submitted batch number.
    pub const LAST_BATCH_NUMBER: &str = "tondi_last_batch_number";
    /// Last confirmed instance ID.
    pub const LAST_INSTANCE_ID: &str = "tondi_last_instance_id";
    /// Highest submitted block height.
    pub const SUBMITTED_HEIGHT: &str = "tondi_submitted_height";
    /// Last confirmed block hash (for parent_hash validation).
    pub const LAST_CONFIRMED_BLOCK_HASH: &str = "tondi_last_confirmed_block_hash";
}

/// In-memory storage implementation for testing and initial development.
/// In production, this should be replaced with a RocksDB-backed implementation.
#[derive(Default)]
pub struct TondiSubmissionDb {
    batches: RwLock<HashMap<u64, BatchRecord>>,
    metadata: RwLock<HashMap<String, Vec<u8>>>,
}

impl TondiSubmissionDb {
    /// Create a new in-memory TondiSubmissionDb.
    pub fn new() -> Self {
        Self::default()
    }
}

impl TondiSubmissionDatabase for TondiSubmissionDb {
    fn get_last_batch_number(&self) -> Result<Option<u64>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        let result = metadata
            .get(metadata_keys::LAST_BATCH_NUMBER)
            .filter(|bytes| bytes.len() >= 8)
            .and_then(|bytes| bytes[..8].try_into().ok())
            .map(u64::from_le_bytes);

        Ok(result)
    }

    fn get_last_instance_id(&self) -> Result<Option<[u8; 32]>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        let result = metadata
            .get(metadata_keys::LAST_INSTANCE_ID)
            .filter(|bytes| bytes.len() >= 32)
            .and_then(|bytes| bytes[..32].try_into().ok());

        Ok(result)
    }

    fn store_batch(&self, record: &BatchRecord) -> Result<()> {
        let batch_number = record.info.batch_number;

        // Store batch
        self.batches
            .write()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
            .insert(batch_number, record.clone());

        // Update last batch number
        self.metadata
            .write()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
            .insert(
                metadata_keys::LAST_BATCH_NUMBER.to_string(),
                batch_number.to_le_bytes().to_vec(),
            );

        Ok(())
    }

    fn get_batch(&self, batch_number: u64) -> Result<Option<BatchRecord>> {
        let batches = self
            .batches
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        Ok(batches.get(&batch_number).cloned())
    }

    fn get_batch_by_start_height(&self, start_height: u64) -> Result<Option<BatchRecord>> {
        let batches = self
            .batches
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        // Search for a batch with matching start_height
        // Note: This is O(n) - in production, use an index
        let start_block_height = u32::try_from(start_height).unwrap_or(u32::MAX);
        Ok(batches
            .values()
            .find(|r| u32::from(r.info.start_height) == start_block_height)
            .cloned())
    }

    fn get_batches_by_status(
        &self,
        status: SubmissionStatus,
    ) -> Result<Vec<BatchRecord>> {
        let batches = self
            .batches
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        Ok(batches
            .values()
            .filter(|r| r.status == status)
            .cloned()
            .collect())
    }

    fn update_batch(&self, record: &BatchRecord) -> Result<()> {
        // Store the updated batch
        self.store_batch(record)?;

        // If confirmed with instance_id, update last instance ID
        if let (SubmissionStatus::Confirmed, Some(instance_id)) =
            (record.status, &record.instance_id)
        {
            self.metadata
                .write()
                .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
                .insert(
                    metadata_keys::LAST_INSTANCE_ID.to_string(),
                    instance_id.to_vec(),
                );
        }

        Ok(())
    }

    fn get_submitted_height(&self) -> Result<Option<BlockHeight>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        let result = metadata
            .get(metadata_keys::SUBMITTED_HEIGHT)
            .filter(|bytes| bytes.len() >= 4)
            .and_then(|bytes| bytes[..4].try_into().ok())
            .map(|arr| BlockHeight::from(u32::from_le_bytes(arr)));

        Ok(result)
    }

    fn set_submitted_height(&self, height: BlockHeight) -> Result<()> {
        let value: u32 = height.into();
        self.metadata
            .write()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
            .insert(
                metadata_keys::SUBMITTED_HEIGHT.to_string(),
                value.to_le_bytes().to_vec(),
            );
        Ok(())
    }

    fn get_last_confirmed_block_hash(&self) -> Result<Option<[u8; 32]>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        let result = metadata
            .get(metadata_keys::LAST_CONFIRMED_BLOCK_HASH)
            .filter(|bytes| bytes.len() >= 32)
            .and_then(|bytes| bytes[..32].try_into().ok());

        Ok(result)
    }

    fn set_last_confirmed_block_hash(&self, hash: [u8; 32]) -> Result<()> {
        self.metadata
            .write()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
            .insert(
                metadata_keys::LAST_CONFIRMED_BLOCK_HASH.to_string(),
                hash.to_vec(),
            );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BatchCommitment,
        BatchInfo,
    };

    #[test]
    fn test_batch_storage_roundtrip() {
        let db = TondiSubmissionDb::new();

        let info = BatchInfo::new(
            1,
            BlockHeight::from(100u32),
            BlockHeight::from(105u32),
            6,
            1234567890,
        );

        let commitment = BatchCommitment::new([1u8; 32], [2u8; 32], [3u8; 32]);

        let record = BatchRecord::new_pending(info, commitment);

        db.store_batch(&record).unwrap();

        let retrieved = db.get_batch(1).unwrap().unwrap();
        assert_eq!(retrieved.info.batch_number, 1);
        assert_eq!(retrieved.status, SubmissionStatus::Pending);
    }

    #[test]
    fn test_last_batch_number() {
        let db = TondiSubmissionDb::new();

        assert!(db.get_last_batch_number().unwrap().is_none());

        let info = BatchInfo::new(
            42,
            BlockHeight::from(100u32),
            BlockHeight::from(105u32),
            6,
            1234567890,
        );
        let commitment = BatchCommitment::new([1u8; 32], [2u8; 32], [3u8; 32]);
        let record = BatchRecord::new_pending(info, commitment);

        db.store_batch(&record).unwrap();

        assert_eq!(db.get_last_batch_number().unwrap(), Some(42));
    }

    #[test]
    fn test_batches_by_status() {
        let db = TondiSubmissionDb::new();

        for i in 0..5 {
            let info = BatchInfo::new(
                i,
                BlockHeight::from((i * 10) as u32),
                BlockHeight::from((i * 10 + 5) as u32),
                6,
                1234567890,
            );
            let commitment = BatchCommitment::new([1u8; 32], [2u8; 32], [3u8; 32]);
            let mut record = BatchRecord::new_pending(info, commitment);

            if i % 2 == 0 {
                record.mark_submitted([i as u8; 32], None);
            }

            db.store_batch(&record).unwrap();
        }

        let submitted = db
            .get_batches_by_status(SubmissionStatus::Submitted)
            .unwrap();
        assert_eq!(submitted.len(), 3);

        let pending = db
            .get_batches_by_status(SubmissionStatus::Pending)
            .unwrap();
        assert_eq!(pending.len(), 2);
    }
}
