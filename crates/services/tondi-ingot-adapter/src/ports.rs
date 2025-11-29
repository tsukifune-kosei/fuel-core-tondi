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
}

