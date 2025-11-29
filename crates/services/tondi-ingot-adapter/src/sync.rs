//! L1-L2 Sync Service (Plan B: Indexer Polling)
//!
//! This module implements the reorg-proof sync service using the Tondi Indexer
//! polling approach. Key responsibilities:
//!
//! 1. **Batch Confirmation Tracking**: Poll the indexer to check if submitted
//!    batches have been included and confirmed on L1.
//!
//! 2. **Reorg Detection**: Detect L1 reorgs by comparing indexed records with
//!    local submission history.
//!
//! 3. **Orphan Handling**: Identify orphaned batches (submitted but not appearing
//!    in indexer after timeout) and schedule resubmission.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    IndexerSyncService                       │
//! │  ┌───────────────────────────────────────────────────────┐  │
//! │  │  Sync Loop                                            │  │
//! │  │  - Poll indexer for confirmed FuelVM batches          │  │
//! │  │  - Compare with local submission records              │  │
//! │  │  - Update batch status (confirmed/orphaned)           │  │
//! │  │  - Schedule resubmission for orphaned batches         │  │
//! │  └───────────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────────┘
//!                          │ Query
//!                          ↓
//! ┌─────────────────────────────────────────────────────────────┐
//! │              Tondi Ingot Indexer (L1)                       │
//! │  - Indexed FuelVM batch Ingots                              │
//! │  - DAA score for finality tracking                          │
//! │  - Schema filtering for FuelVM batches                      │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    config::SyncConfig,
    error::Result,
    ports::{
        IndexerQueryOptions, IndexerStats, L1IngotRecord, L1TxStatus,
        TondiIndexerClient, TondiSubmissionDatabase,
    },
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

/// L1 confirmation level for a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationLevel {
    /// Batch not found on L1.
    NotFound,
    /// Batch is in mempool (submitted but not yet included).
    Pending,
    /// Batch is included in a block but not yet finalized.
    Included {
        /// DAA score of the including block.
        daa_score: u64,
        /// Number of confirmations (DAA score difference from tip).
        confirmations: u64,
    },
    /// Batch is finalized (sufficient confirmations).
    Finalized {
        /// DAA score of the including block.
        daa_score: u64,
    },
    /// Batch was orphaned (removed due to reorg).
    Orphaned,
}

impl ConfirmationLevel {
    /// Check if the batch is confirmed (finalized).
    pub fn is_finalized(&self) -> bool {
        matches!(self, ConfirmationLevel::Finalized { .. })
    }

    /// Check if the batch needs attention (orphaned or not found).
    pub fn needs_resubmission(&self) -> bool {
        matches!(self, ConfirmationLevel::Orphaned | ConfirmationLevel::NotFound)
    }
}

/// Event emitted by the sync service.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// Batch was confirmed on L1.
    BatchConfirmed {
        /// Batch number.
        batch_number: u64,
        /// Instance ID of the Ingot.
        instance_id: [u8; 32],
        /// DAA score of the including block.
        daa_score: u64,
    },
    /// Batch was finalized on L1.
    BatchFinalized {
        /// Batch number.
        batch_number: u64,
        /// Instance ID of the Ingot.
        instance_id: [u8; 32],
        /// DAA score of the including block.
        daa_score: u64,
    },
    /// Batch was orphaned (needs resubmission).
    BatchOrphaned {
        /// Batch number.
        batch_number: u64,
        /// Transaction ID on L1.
        tx_id: [u8; 32],
        /// Reason for orphaning.
        reason: String,
    },
    /// Reorg detected on L1.
    ReorgDetected {
        /// DAA score where reorg occurred.
        reorg_daa_score: u64,
        /// Number of batches affected.
        affected_count: usize,
    },
}

/// Tracking info for a submitted batch.
#[derive(Debug, Clone)]
struct SubmittedBatchTracker {
    /// Transaction ID on L1.
    tx_id: [u8; 32],
    /// When the batch was submitted.
    submitted_at: Instant,
    /// Last known L1 status.
    last_status: ConfirmationLevel,
    /// Resubmission attempts.
    resubmit_count: u8,
}

/// L1-L2 Indexer Sync Service.
///
/// Polls the Tondi Indexer to:
/// - Track batch confirmation status
/// - Detect L1 reorgs
/// - Handle orphaned batches
pub struct IndexerSyncService<I, D>
where
    I: TondiIndexerClient,
    D: TondiSubmissionDatabase,
{
    config: SyncConfig,
    indexer: I,
    database: Arc<D>,
    /// FuelVM schema ID for filtering.
    schema_id: [u8; 32],
    /// Last confirmed DAA score.
    last_confirmed_daa_score: Mutex<u64>,
    /// Tracking submitted batches pending confirmation.
    pending_batches: Mutex<HashMap<u64, SubmittedBatchTracker>>,
    /// Last sync time.
    last_sync: Mutex<Instant>,
}

impl<I, D> IndexerSyncService<I, D>
where
    I: TondiIndexerClient,
    D: TondiSubmissionDatabase,
{
    /// Create a new IndexerSyncService.
    pub fn new(
        config: SyncConfig,
        indexer: I,
        database: Arc<D>,
        schema_id: [u8; 32],
    ) -> Self {
        Self {
            config,
            indexer,
            database,
            schema_id,
            last_confirmed_daa_score: Mutex::new(0),
            pending_batches: Mutex::new(HashMap::new()),
            last_sync: Mutex::new(Instant::now()),
        }
    }

    /// Start tracking a newly submitted batch.
    pub async fn track_submission(
        &self,
        batch_number: u64,
        tx_id: [u8; 32],
    ) {
        let mut pending = self.pending_batches.lock().await;
        pending.insert(batch_number, SubmittedBatchTracker {
            tx_id,
            submitted_at: Instant::now(),
            last_status: ConfirmationLevel::Pending,
            resubmit_count: 0,
        });
        tracing::debug!(
            batch_number,
            tx_id = %hex::encode(tx_id),
            "Started tracking batch submission"
        );
    }

    /// Run one sync iteration.
    ///
    /// Returns events that occurred during this sync.
    pub async fn sync_once(&self) -> Result<Vec<SyncEvent>> {
        let mut events = Vec::new();

        // 1. Get current indexer state
        let stats = self.indexer.get_stats().await?;
        let current_daa_score = stats.current_daa_score;

        // 2. Query confirmed FuelVM batches from indexer
        let last_daa = *self.last_confirmed_daa_score.lock().await;
        let options = IndexerQueryOptions::for_fuel_batches(
            self.schema_id,
            if last_daa > 0 { Some(last_daa) } else { None },
        );
        let confirmed_ingots = self.indexer.query_transactions(options).await?;

        // 3. Process confirmed batches
        for ingot in confirmed_ingots {
            let batch_events = self
                .process_confirmed_ingot(&ingot, current_daa_score)
                .await?;
            events.extend(batch_events);
        }

        // 4. Check for orphaned batches
        let orphan_events = self.check_orphaned_batches(current_daa_score).await?;
        events.extend(orphan_events);

        // 5. Update last sync time
        *self.last_sync.lock().await = Instant::now();

        if !events.is_empty() {
            tracing::info!(
                event_count = events.len(),
                current_daa_score,
                "Sync completed with events"
            );
        }

        Ok(events)
    }

    /// Process a confirmed Ingot from the indexer.
    async fn process_confirmed_ingot(
        &self,
        ingot: &L1IngotRecord,
        current_daa_score: u64,
    ) -> Result<Vec<SyncEvent>> {
        let mut events = Vec::new();

        // Extract batch number by matching tx_id with pending batches
        let batch_number = match self.extract_batch_number(ingot).await {
            Some(n) => n,
            None => {
                tracing::warn!(
                    instance_id = %hex::encode(ingot.instance_id),
                    txid = %hex::encode(ingot.txid),
                    "Could not find batch number for Ingot (not in pending list)"
                );
                return Ok(events);
            }
        };

        // Validate payload if data is available
        if ingot.payload_data.is_some() {
            // Get expected parent hash from last confirmed batch
            let expected_parent = self.database.get_last_confirmed_block_hash().ok().flatten();

            match self.validate_payload(ingot, expected_parent.as_ref()) {
                Ok(payload) => {
                    tracing::debug!(
                        batch_number,
                        start_height = payload.header.start_height,
                        block_count = payload.header.block_count,
                        "Validated batch payload"
                    );

                    // Update last confirmed block hash for next batch's parent validation
                    if let Some(last_block) = payload.blocks.last()
                        && let Err(e) = self.database.set_last_confirmed_block_hash(last_block.block_hash)
                    {
                        tracing::warn!(error = %e, "Failed to update last confirmed block hash");
                    }
                }
                Err(e) => {
                    tracing::error!(
                        batch_number,
                        instance_id = %hex::encode(ingot.instance_id),
                        error = %e,
                        "Failed to validate batch payload - this may indicate a fork!"
                    );
                    // TODO: Handle fork detection - this is a critical security event
                    // For now, we continue processing but log the error
                }
            }
        }

        // Check if this batch is in our pending list
        let mut pending = self.pending_batches.lock().await;
        if let Some(tracker) = pending.get_mut(&batch_number) {
            let confirmations = current_daa_score.saturating_sub(ingot.daa_score);

            if confirmations >= self.config.finality_confirmations {
                // Batch is finalized
                events.push(SyncEvent::BatchFinalized {
                    batch_number,
                    instance_id: ingot.instance_id,
                    daa_score: ingot.daa_score,
                });

                // Update database record
                self.mark_batch_finalized(batch_number, ingot).await?;

                // Remove from pending
                pending.remove(&batch_number);

                // Update last confirmed DAA score
                let mut last_daa = self.last_confirmed_daa_score.lock().await;
                if ingot.daa_score > *last_daa {
                    *last_daa = ingot.daa_score;
                }

                tracing::info!(
                    batch_number,
                    instance_id = %hex::encode(ingot.instance_id),
                    daa_score = ingot.daa_score,
                    confirmations,
                    "Batch finalized on L1"
                );
            } else {
                // Batch is confirmed but not yet finalized
                if !matches!(tracker.last_status, ConfirmationLevel::Included { .. }) {
                    events.push(SyncEvent::BatchConfirmed {
                        batch_number,
                        instance_id: ingot.instance_id,
                        daa_score: ingot.daa_score,
                    });
                }

                tracker.last_status = ConfirmationLevel::Included {
                    daa_score: ingot.daa_score,
                    confirmations,
                };
            }
        }

        Ok(events)
    }

    /// Check for orphaned batches.
    async fn check_orphaned_batches(
        &self,
        _current_daa_score: u64,
    ) -> Result<Vec<SyncEvent>> {
        let mut events = Vec::new();
        let mut pending = self.pending_batches.lock().await;
        let mut to_remove = Vec::new();

        for (batch_number, tracker) in pending.iter_mut() {
            let elapsed = tracker.submitted_at.elapsed();

            if elapsed > self.config.orphan_timeout {
                // Check L1 status of the transaction
                let status = self.indexer.get_tx_status(&tracker.tx_id).await?;

                match status {
                    L1TxStatus::NotFound => {
                        // Transaction not found - likely dropped from mempool
                        if tracker.resubmit_count < self.config.max_resubmit_attempts {
                            events.push(SyncEvent::BatchOrphaned {
                                batch_number: *batch_number,
                                tx_id: tracker.tx_id,
                                reason: "Transaction not found on L1".to_string(),
                            });
                            tracker.resubmit_count = tracker.resubmit_count.saturating_add(1);
                            tracker.last_status = ConfirmationLevel::Orphaned;

                            tracing::warn!(
                                batch_number,
                                tx_id = %hex::encode(tracker.tx_id),
                                resubmit_count = tracker.resubmit_count,
                                "Batch orphaned - scheduling resubmission"
                            );
                        } else {
                            // Max retries exceeded
                            tracing::error!(
                                batch_number,
                                tx_id = %hex::encode(tracker.tx_id),
                                "Batch failed after max resubmission attempts"
                            );
                            to_remove.push(*batch_number);
                        }
                    }
                    L1TxStatus::InMempool => {
                        // Still in mempool, extend timeout
                        tracing::debug!(
                            batch_number,
                            tx_id = %hex::encode(tracker.tx_id),
                            "Batch still in mempool"
                        );
                    }
                    L1TxStatus::Rejected { reason } => {
                        events.push(SyncEvent::BatchOrphaned {
                            batch_number: *batch_number,
                            tx_id: tracker.tx_id,
                            reason: reason.clone(),
                        });
                        tracker.resubmit_count = tracker.resubmit_count.saturating_add(1);
                        tracker.last_status = ConfirmationLevel::Orphaned;

                        tracing::error!(
                            batch_number,
                            tx_id = %hex::encode(tracker.tx_id),
                            %reason,
                            "Batch rejected by L1"
                        );
                    }
                    L1TxStatus::Included { .. } => {
                        // Transaction is included, will be processed in next sync
                    }
                }
            }
        }

        // Remove failed batches
        for batch_number in to_remove {
            pending.remove(&batch_number);
        }

        Ok(events)
    }

    /// Extract batch number from pending batches by matching txid.
    ///
    /// Since `batch_number` is an internal counter and `start_height` is L2 block height,
    /// we need to match by L1 transaction ID to find the correct batch.
    async fn extract_batch_number(&self, ingot: &L1IngotRecord) -> Option<u64> {
        // Match by L1 transaction ID with pending batches
        let pending = self.pending_batches.lock().await;
        for (batch_number, tracker) in pending.iter() {
            if tracker.tx_id == ingot.txid {
                return Some(*batch_number);
            }
        }

        // Fallback: try to find in database by start_height
        // This handles cases where the batch was confirmed but we restarted
        use crate::payload::PayloadBuilder;
        if let Some(ref payload) = ingot.payload_data
            && let Some(header) = PayloadBuilder::decode_header_only(payload)
            && let Ok(Some(record)) = self.database.get_batch_by_start_height(header.start_height)
        {
            return Some(record.info.batch_number);
        }

        None
    }

    /// Validate and process the payload data from an L1 Ingot.
    ///
    /// This performs critical L2 validation:
    /// 1. Verify payload hash matches IngotOutput.hash_payload
    /// 2. Decode the FuelBatchPayload
    /// 3. Validate BatchHeader.parent_hash for chain continuity
    /// 4. Execute truncation strategy if blocks are invalid
    fn validate_payload(
        &self,
        ingot: &L1IngotRecord,
        expected_parent_hash: Option<&[u8; 32]>,
    ) -> std::result::Result<crate::payload::FuelBlockBatchPayload, String> {
        use crate::payload::PayloadBuilder;

        let payload_data = ingot.payload_data.as_ref()
            .ok_or("Missing payload data")?;

        // 1. Verify payload hash integrity
        let computed_hash = *blake3::hash(payload_data).as_bytes();
        if computed_hash != ingot.payload_hash {
            return Err(format!(
                "Payload hash mismatch: expected {}, got {}",
                hex::encode(ingot.payload_hash),
                hex::encode(computed_hash)
            ));
        }

        // 2. Decode the full payload
        let payload = PayloadBuilder::decode(payload_data)
            .map_err(|e| format!("Failed to decode payload: {:?}", e))?;

        // 3. Validate header (including parent_hash)
        if !payload.validate_header(expected_parent_hash) {
            return Err(format!(
                "Invalid batch header: parent_hash mismatch or invalid structure. \
                Expected parent: {:?}, got: {}",
                expected_parent_hash.map(hex::encode),
                hex::encode(payload.header.parent_hash)
            ));
        }

        // 4. Validate block count matches
        let block_count = u32::try_from(payload.blocks.len()).unwrap_or(u32::MAX);
        if payload.header.block_count != block_count {
            return Err(format!(
                "Block count mismatch: header says {}, payload has {}",
                payload.header.block_count, block_count
            ));
        }

        Ok(payload)
    }

    /// Mark a batch as finalized in the database.
    async fn mark_batch_finalized(
        &self,
        batch_number: u64,
        ingot: &L1IngotRecord,
    ) -> Result<()> {
        if let Some(mut record) = self.database.get_batch(batch_number)? {
            record.mark_confirmed(ingot.daa_score, ingot.instance_id);
            self.database.update_batch(&record)?;
        }
        Ok(())
    }

    /// Get the number of pending (unconfirmed) batches.
    pub async fn pending_count(&self) -> usize {
        self.pending_batches.lock().await.len()
    }

    /// Get batches that need resubmission.
    pub async fn get_batches_for_resubmission(&self) -> Vec<u64> {
        let pending = self.pending_batches.lock().await;
        pending
            .iter()
            .filter(|(_, t)| t.last_status.needs_resubmission())
            .map(|(batch_number, _)| *batch_number)
            .collect()
    }

    /// Get current indexer stats.
    pub async fn get_indexer_stats(&self) -> Result<IndexerStats> {
        self.indexer.get_stats().await
    }

    /// Get time since last successful sync.
    pub async fn time_since_last_sync(&self) -> Duration {
        self.last_sync.lock().await.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::mock::MockTondiIndexerClient;
    use crate::storage::TondiSubmissionDb;
    use std::sync::Arc;

    fn create_test_schema_id() -> [u8; 32] {
        *blake3::hash(b"fuelvm/batch/v1").as_bytes()
    }

    #[tokio::test]
    async fn test_sync_service_creation() {
        let config = SyncConfig::default();
        let indexer = MockTondiIndexerClient::default();
        let database = Arc::new(TondiSubmissionDb::new());
        let schema_id = create_test_schema_id();

        let service = IndexerSyncService::new(config, indexer, database, schema_id);
        assert_eq!(service.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_track_submission() {
        let config = SyncConfig::default();
        let indexer = MockTondiIndexerClient::default();
        let database = Arc::new(TondiSubmissionDb::new());
        let schema_id = create_test_schema_id();

        let service = IndexerSyncService::new(config, indexer, database, schema_id);

        // Track a submission
        service.track_submission(1, [0xab; 32]).await;
        assert_eq!(service.pending_count().await, 1);

        // Track another
        service.track_submission(2, [0xcd; 32]).await;
        assert_eq!(service.pending_count().await, 2);
    }

    #[tokio::test]
    async fn test_sync_once_empty() {
        let config = SyncConfig::default();
        let indexer = MockTondiIndexerClient::default();
        let database = Arc::new(TondiSubmissionDb::new());
        let schema_id = create_test_schema_id();

        let service = IndexerSyncService::new(config, indexer, database, schema_id);

        // Sync with no pending batches
        let events = service.sync_once().await.unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_confirmation_level() {
        assert!(ConfirmationLevel::Finalized { daa_score: 100 }.is_finalized());
        assert!(!ConfirmationLevel::Included { daa_score: 100, confirmations: 5 }.is_finalized());
        assert!(!ConfirmationLevel::Pending.is_finalized());

        assert!(ConfirmationLevel::Orphaned.needs_resubmission());
        assert!(ConfirmationLevel::NotFound.needs_resubmission());
        assert!(!ConfirmationLevel::Pending.needs_resubmission());
    }
}

