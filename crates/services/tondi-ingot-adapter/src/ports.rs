//! Port traits for the Tondi Ingot Adapter.
//!
//! These traits define the interfaces that must be implemented for
//! the adapter to interact with the rest of the system.

use crate::{
    error::Result,
    types::{
        BatchRecord,
        SubmissionStatus,
    },
};
use async_trait::async_trait;
use fuel_core_types::{
    blockchain::SealedBlock,
    fuel_types::BlockHeight,
};

/// Port for accessing sealed blocks.
#[async_trait]
pub trait BlockProvider: Send + Sync {
    /// Get a sealed block by height.
    async fn get_sealed_block(
        &self,
        height: &BlockHeight,
    ) -> Result<Option<SealedBlock>>;

    /// Get the latest sealed block height.
    async fn latest_height(&self) -> Result<BlockHeight>;
}

/// Port for Tondi L1 RPC operations.
#[async_trait]
pub trait TondiRpcClient: Send + Sync {
    /// Submit an Ingot transaction to Tondi.
    ///
    /// Returns the transaction ID on success.
    async fn submit_transaction(&self, tx_bytes: Vec<u8>) -> Result<[u8; 32]>;

    /// Get the current Tondi block height.
    async fn get_block_height(&self) -> Result<u64>;

    /// Check if a transaction is confirmed.
    ///
    /// Returns the block height and instance ID if confirmed.
    async fn get_transaction_status(
        &self,
        tx_id: &[u8; 32],
    ) -> Result<Option<(u64, [u8; 32])>>;

    /// Get available UTXOs for the given address.
    async fn get_utxos(&self, address: &[u8]) -> Result<Vec<UtxoInfo>>;
}

/// Information about an available UTXO.
#[derive(Debug, Clone)]
pub struct UtxoInfo {
    /// Outpoint transaction ID.
    pub tx_id: [u8; 32],
    /// Outpoint index.
    pub index: u32,
    /// UTXO value in SAU.
    pub value: u64,
}

/// Port for signing transactions.
#[async_trait]
pub trait Signer: Send + Sync {
    /// Get the public key.
    fn public_key(&self) -> Vec<u8>;

    /// Sign a message.
    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>>;
}

/// Port for batch submission database operations.
pub trait TondiSubmissionDatabase: Send + Sync {
    /// Get the last submitted batch number.
    fn get_last_batch_number(&self) -> Result<Option<u64>>;

    /// Get the last confirmed instance ID (for parent_ref).
    fn get_last_instance_id(&self) -> Result<Option<[u8; 32]>>;

    /// Store a batch record.
    fn store_batch(&self, record: &BatchRecord) -> Result<()>;

    /// Get a batch record by number.
    fn get_batch(&self, batch_number: u64) -> Result<Option<BatchRecord>>;

    /// Get batches with a specific status.
    fn get_batches_by_status(&self, status: SubmissionStatus)
        -> Result<Vec<BatchRecord>>;

    /// Update a batch record.
    fn update_batch(&self, record: &BatchRecord) -> Result<()>;

    /// Get the highest block height that has been submitted.
    fn get_submitted_height(&self) -> Result<Option<BlockHeight>>;

    /// Set the highest block height that has been submitted.
    fn set_submitted_height(&self, height: BlockHeight) -> Result<()>;
}

/// Notifier for block production events.
#[async_trait]
pub trait BlockNotifier: Send + Sync {
    /// Notify that a new block has been produced.
    async fn notify_block(&self, block: SealedBlock) -> Result<()>;
}

/// Port for Tondi Indexer RPC operations.
///
/// This trait defines the interface for querying the Tondi Ingot Indexer
/// to track batch confirmation status and detect reorgs (Plan B approach).
#[async_trait]
pub trait TondiIndexerClient: Send + Sync {
    /// Get indexer statistics.
    async fn get_stats(&self) -> Result<IndexerStats>;

    /// Query transactions by schema ID.
    ///
    /// Returns Ingot transactions matching the FuelVM schema, optionally
    /// filtered by DAA score range.
    async fn query_transactions(
        &self,
        options: IndexerQueryOptions,
    ) -> Result<Vec<L1IngotRecord>>;

    /// Get a specific transaction by instance ID.
    async fn get_transaction(
        &self,
        instance_id: &[u8; 32],
    ) -> Result<Option<L1IngotRecord>>;

    /// Get transaction status by transaction ID.
    ///
    /// Returns the L1 inclusion status of a submitted transaction.
    async fn get_tx_status(&self, tx_id: &[u8; 32]) -> Result<L1TxStatus>;
}

/// Query options for the Tondi Indexer.
#[derive(Debug, Clone, Default)]
pub struct IndexerQueryOptions {
    /// Pagination offset.
    pub offset: usize,
    /// Maximum results to return.
    pub limit: usize,
    /// Filter by schema ID.
    pub schema_id: Option<[u8; 32]>,
    /// Minimum DAA score (bluescore) filter.
    pub min_daa_score: Option<u64>,
    /// Maximum DAA score filter.
    pub max_daa_score: Option<u64>,
    /// Include spent transactions.
    pub include_spent: bool,
    /// Sort order.
    pub sort_desc: bool,
}

impl IndexerQueryOptions {
    /// Create options for querying FuelVM batches.
    pub fn for_fuel_batches(schema_id: [u8; 32], from_daa_score: Option<u64>) -> Self {
        Self {
            offset: 0,
            limit: 100,
            schema_id: Some(schema_id),
            min_daa_score: from_daa_score,
            max_daa_score: None,
            include_spent: false,
            sort_desc: false, // Oldest first
        }
    }
}

/// Record of an Ingot transaction indexed on L1.
#[derive(Debug, Clone)]
pub struct L1IngotRecord {
    /// Transaction ID on Tondi L1.
    pub txid: [u8; 32],
    /// Block ID where the transaction is included.
    pub block_id: [u8; 32],
    /// DAA score (bluescore) of the block.
    pub daa_score: u64,
    /// Transaction index in block.
    pub tx_index: u32,
    /// Output index.
    pub output_index: u32,
    /// Schema ID.
    pub schema_id: [u8; 32],
    /// Payload hash.
    pub payload_hash: [u8; 32],
    /// Raw payload data (optional, may be large).
    pub payload_data: Option<Vec<u8>>,
    /// Instance ID (unique identifier for oUTXO).
    pub instance_id: [u8; 32],
    /// Whether this output has been spent.
    pub is_spent: bool,
    /// Transaction that spent this output (if spent).
    pub spent_txid: Option<[u8; 32]>,
    /// DAA score when spent (if spent).
    pub spent_daa_score: Option<u64>,
    /// Creation timestamp.
    pub created_at: u64,
}

/// Transaction status on L1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum L1TxStatus {
    /// Transaction not found on L1.
    NotFound,
    /// Transaction is in the mempool.
    InMempool,
    /// Transaction is included in a block.
    Included {
        /// Block ID.
        block_id: [u8; 32],
        /// DAA score of the block.
        daa_score: u64,
        /// Instance ID of the created Ingot.
        instance_id: [u8; 32],
    },
    /// Transaction was rejected.
    Rejected {
        /// Rejection reason.
        reason: String,
    },
}

impl L1TxStatus {
    /// Check if the transaction is confirmed.
    pub fn is_included(&self) -> bool {
        matches!(self, L1TxStatus::Included { .. })
    }
}

/// Indexer statistics.
#[derive(Debug, Clone)]
pub struct IndexerStats {
    /// Total indexed transactions.
    pub total_transactions: u64,
    /// Active (unspent) transactions.
    pub active_transactions: u64,
    /// Current indexed DAA score.
    pub current_daa_score: u64,
    /// Latest indexed block ID.
    pub latest_block_id: [u8; 32],
    /// Last update timestamp.
    pub last_update: u64,
}

#[cfg(test)]
pub mod mock {
    //! Mock implementations for testing.

    use super::*;
    use std::sync::{
        Arc,
        Mutex,
    };

    /// Mock block provider for testing.
    #[derive(Default)]
    pub struct MockBlockProvider {
        /// Stored blocks.
        pub blocks: Arc<Mutex<Vec<SealedBlock>>>,
    }

    #[async_trait]
    impl BlockProvider for MockBlockProvider {
        async fn get_sealed_block(
            &self,
            height: &BlockHeight,
        ) -> Result<Option<SealedBlock>> {
            let blocks = self.blocks.lock().unwrap();
            Ok(blocks
                .iter()
                .find(|b| b.entity.header().consensus().height == *height)
                .cloned())
        }

        async fn latest_height(&self) -> Result<BlockHeight> {
            let blocks = self.blocks.lock().unwrap();
            Ok(blocks
                .last()
                .map(|b| b.entity.header().consensus().height)
                .unwrap_or_default())
        }
    }

    /// Mock Tondi RPC client for testing.
    #[derive(Default)]
    pub struct MockTondiRpcClient {
        /// Submitted transactions.
        pub submitted_txs: Arc<Mutex<Vec<Vec<u8>>>>,
        /// Current block height.
        pub block_height: Arc<Mutex<u64>>,
    }

    #[async_trait]
    impl TondiRpcClient for MockTondiRpcClient {
        async fn submit_transaction(&self, tx_bytes: Vec<u8>) -> Result<[u8; 32]> {
            let tx_id = *blake3::hash(&tx_bytes).as_bytes();
            self.submitted_txs.lock().unwrap().push(tx_bytes);
            Ok(tx_id)
        }

        async fn get_block_height(&self) -> Result<u64> {
            Ok(*self.block_height.lock().unwrap())
        }

        async fn get_transaction_status(
            &self,
            tx_id: &[u8; 32],
        ) -> Result<Option<(u64, [u8; 32])>> {
            let height = *self.block_height.lock().unwrap();
            // Simulate confirmation after 1 block
            if height > 0 {
                Ok(Some((height, *tx_id)))
            } else {
                Ok(None)
            }
        }

        async fn get_utxos(&self, _address: &[u8]) -> Result<Vec<UtxoInfo>> {
            Ok(vec![UtxoInfo {
                tx_id: [0u8; 32],
                index: 0,
                value: 1_000_000_000, // 10 TDI
            }])
        }
    }

    /// Mock Tondi Indexer client for testing.
    #[derive(Default)]
    pub struct MockTondiIndexerClient {
        /// Indexed transactions.
        pub indexed_txs: Arc<Mutex<Vec<L1IngotRecord>>>,
        /// Current DAA score.
        pub current_daa_score: Arc<Mutex<u64>>,
        /// Transaction statuses (tx_id -> status).
        pub tx_statuses: Arc<Mutex<std::collections::HashMap<[u8; 32], L1TxStatus>>>,
    }

    #[async_trait]
    impl TondiIndexerClient for MockTondiIndexerClient {
        async fn get_stats(&self) -> Result<IndexerStats> {
            let txs = self.indexed_txs.lock().unwrap();
            let daa_score = *self.current_daa_score.lock().unwrap();
            Ok(IndexerStats {
                total_transactions: txs.len() as u64,
                active_transactions: txs.iter().filter(|t| !t.is_spent).count() as u64,
                current_daa_score: daa_score,
                latest_block_id: [0u8; 32],
                last_update: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            })
        }

        async fn query_transactions(
            &self,
            options: IndexerQueryOptions,
        ) -> Result<Vec<L1IngotRecord>> {
            let txs = self.indexed_txs.lock().unwrap();
            let filtered: Vec<L1IngotRecord> = txs
                .iter()
                .filter(|tx| {
                    // Schema filter
                    if let Some(schema_id) = &options.schema_id {
                        if tx.schema_id != *schema_id {
                            return false;
                        }
                    }
                    // DAA score range filter
                    if let Some(min) = options.min_daa_score {
                        if tx.daa_score < min {
                            return false;
                        }
                    }
                    if let Some(max) = options.max_daa_score {
                        if tx.daa_score > max {
                            return false;
                        }
                    }
                    // Spent filter
                    if !options.include_spent && tx.is_spent {
                        return false;
                    }
                    true
                })
                .skip(options.offset)
                .take(options.limit)
                .cloned()
                .collect();
            Ok(filtered)
        }

        async fn get_transaction(
            &self,
            instance_id: &[u8; 32],
        ) -> Result<Option<L1IngotRecord>> {
            let txs = self.indexed_txs.lock().unwrap();
            Ok(txs.iter().find(|tx| &tx.instance_id == instance_id).cloned())
        }

        async fn get_tx_status(&self, tx_id: &[u8; 32]) -> Result<L1TxStatus> {
            let statuses = self.tx_statuses.lock().unwrap();
            Ok(statuses.get(tx_id).cloned().unwrap_or(L1TxStatus::NotFound))
        }
    }
}

