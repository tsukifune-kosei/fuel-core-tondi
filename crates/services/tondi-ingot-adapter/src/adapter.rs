//! Core Tondi Ingot Adapter implementation.
//!
//! This module contains the main adapter logic for batching and submitting
//! FuelVM blocks to Tondi L1.
//!
//! ## Integration with Sync Service
//!
//! The adapter works with the `IndexerSyncService` for confirmation tracking:
//! 1. Adapter submits batches and notifies the sync service
//! 2. Sync service polls the indexer for confirmation status
//! 3. Sync service handles reorg detection and resubmission scheduling

use crate::{
    config::Config,
    error::{
        Result,
        TondiAdapterError,
    },
    payload::PayloadBuilder,
    ports::{
        Signer,
        TondiRpcClient,
        TondiSubmissionDatabase,
    },
    types::{
        BatchRecord,
        SubmissionStatus,
    },
};
use fuel_core_types::blockchain::SealedBlock;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tondi_consensus_core::tx::ingot::{
    create_ingot_output,
    IngotFlags,
    IngotOutput,
    Lock,
    SignatureType,
};
use tondi_hashes::Hash;

/// Event sent to the sync service when a batch is submitted.
#[derive(Debug, Clone)]
pub struct BatchSubmittedEvent {
    /// Batch number.
    pub batch_number: u64,
    /// Transaction ID on L1.
    pub tx_id: [u8; 32],
}

/// Tondi Ingot Adapter for submitting FuelVM blocks to L1.
pub struct TondiIngotAdapter<R, S, D>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
{
    config: Config,
    rpc_client: R,
    signer: S,
    database: D,
    /// Pending blocks waiting to be submitted.
    pending_blocks: Mutex<Vec<SealedBlock>>,
    /// Last submission time.
    last_submission: Mutex<Instant>,
    /// Current batch counter.
    batch_counter: Mutex<u64>,
    /// Last confirmed instance ID (for parent_ref).
    last_instance_id: Mutex<Option<[u8; 32]>>,
    /// Channel to notify sync service about submissions.
    submission_tx: Option<mpsc::Sender<BatchSubmittedEvent>>,
}

impl<R, S, D> TondiIngotAdapter<R, S, D>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
{
    /// Create a new TondiIngotAdapter.
    pub fn new(config: Config, rpc_client: R, signer: S, database: D) -> Result<Self> {
        // Recover state from database
        let batch_counter = database.get_last_batch_number()?.unwrap_or(0);
        let last_instance_id = database.get_last_instance_id()?;

        Ok(Self {
            config,
            rpc_client,
            signer,
            database,
            pending_blocks: Mutex::new(Vec::new()),
            last_submission: Mutex::new(Instant::now()),
            batch_counter: Mutex::new(batch_counter),
            last_instance_id: Mutex::new(last_instance_id),
            submission_tx: None,
        })
    }

    /// Set the submission notification channel.
    ///
    /// This channel is used to notify the sync service when batches are submitted.
    pub fn with_submission_channel(
        mut self,
        tx: mpsc::Sender<BatchSubmittedEvent>,
    ) -> Self {
        self.submission_tx = Some(tx);
        self
    }

    /// Get the schema ID bytes.
    pub fn schema_id_bytes(&self) -> [u8; 32] {
        self.config.schema_id_bytes()
    }

    /// Queue a block for submission.
    pub async fn queue_block(&self, block: SealedBlock) -> Result<()> {
        let mut pending = self.pending_blocks.lock().await;
        pending.push(block);

        // Check if we should submit immediately
        if self.should_submit_now(&pending).await {
            drop(pending);
            self.submit_batch().await?;
        }

        Ok(())
    }

    /// Check if we should submit a batch now.
    async fn should_submit_now(&self, pending: &[SealedBlock]) -> bool {
        // Check max batch size
        if pending.len() >= self.config.max_batch_size as usize {
            return true;
        }

        let last_submission = self.last_submission.lock().await;
        let elapsed = last_submission.elapsed();

        // Check force submission timeout
        if elapsed >= self.config.force_submission_timeout && !pending.is_empty() {
            return true;
        }

        // Check submission interval
        if elapsed >= self.config.submission_interval
            && pending.len() >= self.config.min_batch_size as usize
        {
            return true;
        }

        false
    }

    /// Submit the current batch of blocks.
    pub async fn submit_batch(&self) -> Result<()> {
        // Take pending blocks
        let blocks = {
            let mut pending = self.pending_blocks.lock().await;
            if pending.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *pending)
        };

        // Get current batch number and increment
        let batch_number = {
            let mut counter = self.batch_counter.lock().await;
            let num = *counter;
            *counter = counter.saturating_add(1);
            num
        };

        // Get parent reference
        let parent_ref = *self.last_instance_id.lock().await;

        // Build payload
        let mut builder = PayloadBuilder::new(self.config.clone(), batch_number);
        builder = builder.add_blocks(blocks);
        if let Some(parent) = parent_ref {
            builder = builder.with_parent_ref(parent);
        }
        let payload = builder.build()?;

        // Encode payload as TLV
        let payload_bytes = PayloadBuilder::encode_tlv(&payload)?;
        let payload_hash = Hash::from_bytes(*blake3::hash(&payload_bytes).as_bytes());

        // Build Ingot output
        let schema_id = Hash::from_bytes(self.config.schema_id_bytes());
        let ingot_output = IngotOutput::new(
            schema_id,
            payload_hash,
            IngotFlags::new(),
            Lock::PubKey {
                pubkey: self.signer.public_key(),
                sig_type: SignatureType::CopperootSchnorr,
            },
        );

        // Build and submit transaction
        let tx_bytes = self
            .build_ingot_transaction(ingot_output, payload_bytes, parent_ref.is_none())
            .await?;

        let tx_id = self.rpc_client.submit_transaction(tx_bytes).await?;

        // Create and store batch record
        let mut record = BatchRecord::new_pending(payload.batch_info.clone(), payload.commitment);
        record.mark_submitted(tx_id, parent_ref);
        self.database.store_batch(&record)?;

        // Notify sync service about the submission
        if let Some(ref tx) = self.submission_tx {
            let event = BatchSubmittedEvent {
                batch_number,
                tx_id,
            };
            // Non-blocking send - if channel is full, we'll track via database anyway
            let _ = tx.try_send(event);
        }

        // Update submission time
        *self.last_submission.lock().await = Instant::now();

        tracing::info!(
            batch_number = record.info.batch_number,
            start_height = %record.info.start_height,
            end_height = %record.info.end_height,
            block_count = record.info.block_count,
            tx_id = %hex::encode(tx_id),
            "Submitted batch to Tondi L1"
        );

        Ok(())
    }

    /// Get batches that need resubmission.
    pub fn get_failed_batches(&self) -> Result<Vec<BatchRecord>> {
        self.database.get_batches_by_status(SubmissionStatus::Failed)
    }

    /// Mark a batch as needing resubmission.
    ///
    /// Note: Actual resubmission requires the original block data, which is not
    /// stored in the current implementation. The sync service should detect
    /// orphaned batches and trigger block producer to requeue those blocks.
    ///
    /// For now, this validates the batch exists and is in a resubmittable state.
    pub async fn mark_for_resubmission(&self, batch_number: u64) -> Result<()> {
        let record = self.database.get_batch(batch_number)?
            .ok_or_else(|| TondiAdapterError::Storage(
                format!("Batch {} not found", batch_number)
            ))?;

        if record.status != SubmissionStatus::Failed
            && record.status != SubmissionStatus::Submitted
        {
            return Err(TondiAdapterError::InvalidBlockData(format!(
                "Batch {} is in {:?} status, cannot resubmit",
                batch_number, record.status
            )));
        }

        tracing::warn!(
            batch_number,
            start_height = %record.info.start_height,
            end_height = %record.info.end_height,
            "Batch marked for resubmission - blocks need to be requeued from block producer"
        );

        // TODO: Implement block requeuing from block producer
        // The block producer should maintain a record of blocks that have been
        // submitted but not yet confirmed, and requeue them on orphan detection.

        Ok(())
    }

    /// Build an Ingot transaction.
    ///
    /// # Arguments
    /// * `ingot_output` - The Ingot output to include in the transaction
    /// * `payload` - The payload data to include in the witness
    /// * `_is_genesis` - Whether this is the genesis batch (unused currently)
    async fn build_ingot_transaction(
        &self,
        ingot_output: IngotOutput,
        payload: Vec<u8>,
        _is_genesis: bool,
    ) -> Result<Vec<u8>> {
        use tondi_consensus_core::subnets::SUBNETWORK_ID_NATIVE;
        use tondi_consensus_core::tx::ingot::{
            create_ingot_signature_script,
            IngotWitness,
        };
        use tondi_consensus_core::tx::{
            Transaction,
            TransactionInput,
            TransactionOutpoint,
        };

        // Get UTXOs for funding
        let utxos = self.rpc_client.get_utxos(&self.signer.public_key()).await?;
        if utxos.is_empty() {
            return Err(TondiAdapterError::Submission(
                "No UTXOs available for funding".to_string(),
            ));
        }

        // Use first UTXO for funding
        let funding_utxo = &utxos[0];

        // Create Ingot output with configurable value
        let ingot_value = Config::DEFAULT_INGOT_OUTPUT_VALUE;
        let ingot_tx_output = create_ingot_output(ingot_value, ingot_output.clone())
            .map_err(|e| TondiAdapterError::PayloadBuild(e.to_string()))?;

        // Create funding input (unsigned)
        let funding_input = TransactionInput {
            previous_outpoint: TransactionOutpoint::new(
                Hash::from_bytes(funding_utxo.tx_id),
                funding_utxo.index,
            ),
            signature_script: vec![], // Will be signed
            sequence: 0,
            sig_op_count: 1,
        };

        // Build unsigned transaction for sighash computation
        let unsigned_tx = Transaction::new(
            1, // version
            vec![funding_input.clone()],
            vec![ingot_tx_output.clone()],
            0, // locktime
            SUBNETWORK_ID_NATIVE,
            0, // gas
            vec![],
        );

        // Compute sighash and sign
        // TODO: Use proper sighash computation from tondi_consensus_core::tx::sign
        // For now, we hash the serialized transaction as a placeholder
        let tx_bytes = borsh::to_vec(&unsigned_tx)
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))?;
        let sighash = blake3::hash(&tx_bytes);

        let signature = self.signer.sign(sighash.as_bytes()).await?;

        // Create witness with payload and signature
        let witness_with_sig = IngotWitness {
            payload: Some(payload),
            auth_sigs: vec![signature],
            script_reveal: None,
            mast_proof: None,
        };

        // Create signature script from witness
        let witness_script = create_ingot_signature_script(&witness_with_sig)
            .map_err(|e| TondiAdapterError::Signing(e.to_string()))?;

        // Create signed input
        let signed_input = TransactionInput {
            previous_outpoint: TransactionOutpoint::new(
                Hash::from_bytes(funding_utxo.tx_id),
                funding_utxo.index,
            ),
            signature_script: witness_script,
            sequence: 0,
            sig_op_count: 1,
        };

        // Build final signed transaction with the original ingot_output
        let signed_tx = Transaction::new(
            1, // version
            vec![signed_input],
            vec![ingot_tx_output], // Use the original output, not a new one
            0, // locktime
            SUBNETWORK_ID_NATIVE,
            0, // gas
            vec![],
        );

        borsh::to_vec(&signed_tx)
            .map_err(|e| TondiAdapterError::Serialization(e.to_string()))
    }

    /// Check for pending batches and update their status.
    pub async fn check_pending_submissions(&self) -> Result<()> {
        let pending = self
            .database
            .get_batches_by_status(SubmissionStatus::Submitted)?;

        for mut record in pending {
            if let Some(tx_id) = record.tondi_tx_id {
                match self.rpc_client.get_transaction_status(&tx_id).await {
                    Ok(Some((block_height, instance_id))) => {
                        record.mark_confirmed(block_height, instance_id);
                        self.database.update_batch(&record)?;

                        // Update last instance ID
                        *self.last_instance_id.lock().await = Some(instance_id);

                        tracing::info!(
                            batch_number = record.info.batch_number,
                            tondi_block = block_height,
                            instance_id = %hex::encode(instance_id),
                            "Batch confirmed on Tondi L1"
                        );
                    }
                    Ok(None) => {
                        // Still pending, continue
                    }
                    Err(e) => {
                        tracing::warn!(
                            batch_number = record.info.batch_number,
                            error = %e,
                            "Failed to check batch status"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Get the number of pending blocks.
    pub async fn pending_block_count(&self) -> usize {
        self.pending_blocks.lock().await.len()
    }

    /// Force submit any pending blocks.
    pub async fn flush(&self) -> Result<()> {
        let has_pending = !self.pending_blocks.lock().await.is_empty();
        if has_pending {
            self.submit_batch().await?;
        }
        Ok(())
    }
}

