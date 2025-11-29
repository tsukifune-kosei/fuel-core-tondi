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

        if let Some(bytes) = metadata.get(metadata_keys::LAST_BATCH_NUMBER) {
            if bytes.len() >= 8 {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&bytes[..8]);
                return Ok(Some(u64::from_le_bytes(arr)));
            }
        }
        Ok(None)
    }

    fn get_last_instance_id(&self) -> Result<Option<[u8; 32]>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        if let Some(bytes) = metadata.get(metadata_keys::LAST_INSTANCE_ID) {
            if bytes.len() >= 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes[..32]);
                return Ok(Some(arr));
            }
        }
        Ok(None)
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

        // If confirmed, update last instance ID
        if record.status == SubmissionStatus::Confirmed {
            if let Some(instance_id) = record.instance_id {
                self.metadata
                    .write()
                    .map_err(|e| TondiAdapterError::Storage(e.to_string()))?
                    .insert(
                        metadata_keys::LAST_INSTANCE_ID.to_string(),
                        instance_id.to_vec(),
                    );
            }
        }

        Ok(())
    }

    fn get_submitted_height(&self) -> Result<Option<BlockHeight>> {
        let metadata = self
            .metadata
            .read()
            .map_err(|e| TondiAdapterError::Storage(e.to_string()))?;

        if let Some(bytes) = metadata.get(metadata_keys::SUBMITTED_HEIGHT) {
            if bytes.len() >= 4 {
                let mut arr = [0u8; 4];
                arr.copy_from_slice(&bytes[..4]);
                return Ok(Some(BlockHeight::from(u32::from_le_bytes(arr))));
            }
        }
        Ok(None)
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
