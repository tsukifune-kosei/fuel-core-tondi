//! Background service for Tondi block submission and L1 sync.
//!
//! This module implements the `RunnableService` trait for the Tondi adapter,
//! enabling it to run as a background task that:
//! 1. Processes blocks and submits batches to Tondi L1
//! 2. Polls the Tondi Indexer for confirmation status (Plan B approach)
//! 3. Handles reorg detection and batch resubmission
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    TondiIngotService                            │
//! │  ┌──────────────────────────┐  ┌─────────────────────────────┐  │
//! │  │  Submission Task         │  │  Sync Task                  │  │
//! │  │  - Queue blocks          │  │  - Poll indexer             │  │
//! │  │  - Build & submit batches│  │  - Track confirmations      │  │
//! │  │  - Store records         │  │  - Detect reorgs            │  │
//! │  └──────────────────────────┘  └─────────────────────────────┘  │
//! │              │                              ↑                    │
//! │              └──── submission_tx ───────────┘                    │
//! └─────────────────────────────────────────────────────────────────┘
//! ```

use crate::{
    adapter::{BatchSubmittedEvent, TondiIngotAdapter},
    config::Config,
    error::{
        Result,
        TondiAdapterError,
    },
    ports::{
        Signer,
        TondiIndexerClient,
        TondiRpcClient,
        TondiSubmissionDatabase,
    },
    sync::{IndexerSyncService, SyncEvent},
};
use async_trait::async_trait;
use fuel_core_services::{
    RunnableService,
    RunnableTask,
    ServiceRunner,
    StateWatcher,
    TaskNextAction,
};
use fuel_core_types::blockchain::SealedBlock;
use std::sync::Arc;
use tokio::sync::{
    mpsc,
    watch,
};

/// Submission and sync state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    /// Service is running and submitting batches.
    Running {
        /// Last submitted batch number.
        last_batch: u64,
        /// Number of batches pending L1 confirmation.
        pending_confirmations: usize,
    },
    /// Service is idle (no pending blocks).
    Idle,
    /// Service encountered an error.
    Error {
        /// Error message.
        message: String,
    },
    /// Service is syncing with L1.
    Syncing {
        /// Current L1 DAA score.
        l1_daa_score: u64,
    },
}

impl SyncState {
    /// Check if the service is running.
    pub fn is_running(&self) -> bool {
        matches!(self, SyncState::Running { .. } | SyncState::Syncing { .. })
    }
}

/// Type alias for the service (without indexer).
pub type Service<R, S, D> = ServiceRunner<NotInitializedTask<R, S, D>>;

/// Type alias for the service with sync (with indexer).
pub type ServiceWithSync<R, S, D, I> = ServiceRunner<NotInitializedTaskWithSync<R, S, D, I>>;

/// Shared state exposed by the service.
#[derive(Clone)]
pub struct SharedState {
    /// Channel to send blocks for submission.
    block_sender: mpsc::Sender<SealedBlock>,
    /// Current sync state.
    sync_state: watch::Receiver<SyncState>,
    /// Whether sync is enabled.
    sync_enabled: bool,
}

impl SharedState {
    /// Send a block for submission.
    pub async fn submit_block(&self, block: SealedBlock) -> Result<()> {
        self.block_sender
            .send(block)
            .await
            .map_err(|_| TondiAdapterError::Shutdown)
    }

    /// Get the current sync state.
    pub fn sync_state(&self) -> SyncState {
        self.sync_state.borrow().clone()
    }

    /// Wait for a state change.
    pub async fn wait_for_change(&mut self) -> Result<SyncState> {
        self.sync_state
            .changed()
            .await
            .map_err(|_| TondiAdapterError::Shutdown)?;
        Ok(self.sync_state.borrow().clone())
    }

    /// Check if sync is enabled.
    pub fn sync_enabled(&self) -> bool {
        self.sync_enabled
    }
}

/// Not-initialized task that will be converted to a running task.
pub struct NotInitializedTask<R, S, D>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
{
    config: Config,
    adapter: Arc<TondiIngotAdapter<R, S, D>>,
    block_receiver: mpsc::Receiver<SealedBlock>,
    block_sender: mpsc::Sender<SealedBlock>,
    sync_state_tx: watch::Sender<SyncState>,
    sync_state_rx: watch::Receiver<SyncState>,
}

impl<R, S, D> NotInitializedTask<R, S, D>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
{
    /// Create a new not-initialized task.
    pub fn new(
        config: Config,
        rpc_client: R,
        signer: S,
        database: D,
    ) -> Result<Self> {
        let adapter = TondiIngotAdapter::new(
            config.clone(),
            rpc_client,
            signer,
            database,
        )?;

        let (block_sender, block_receiver) = mpsc::channel(100);
        let (sync_state_tx, sync_state_rx) = watch::channel(SyncState::Idle);

        Ok(Self {
            config,
            adapter: Arc::new(adapter),
            block_receiver,
            block_sender,
            sync_state_tx,
            sync_state_rx,
        })
    }
}

/// Not-initialized task with sync service (Plan B: Indexer polling).
pub struct NotInitializedTaskWithSync<R, S, D, I>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
    I: TondiIndexerClient,
{
    config: Config,
    adapter: Arc<TondiIngotAdapter<R, S, ArcDatabase<D>>>,
    sync_service: Arc<IndexerSyncService<I, D>>,
    #[allow(dead_code)]
    database: Arc<D>,
    block_receiver: mpsc::Receiver<SealedBlock>,
    block_sender: mpsc::Sender<SealedBlock>,
    submission_rx: mpsc::Receiver<BatchSubmittedEvent>,
    sync_state_tx: watch::Sender<SyncState>,
    sync_state_rx: watch::Receiver<SyncState>,
    sync_event_tx: mpsc::Sender<SyncEvent>,
    #[allow(dead_code)]
    sync_event_rx: mpsc::Receiver<SyncEvent>,
}

impl<R, S, D, I> NotInitializedTaskWithSync<R, S, D, I>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
    I: TondiIndexerClient + 'static,
{
    /// Create a new not-initialized task with sync service.
    ///
    /// Note: The database is wrapped in an Arc for sharing between
    /// the adapter and sync service.
    pub fn new_with_arc_database(
        config: Config,
        rpc_client: R,
        signer: S,
        database: Arc<D>,
        indexer: I,
    ) -> Result<Self> {
        // Create submission notification channel
        let (submission_tx, submission_rx) = mpsc::channel(100);

        // Create adapter - we need to create a new wrapper database
        // that forwards to the Arc<D>
        let adapter_db = ArcDatabase {
            inner: database.clone(),
        };
        let adapter = TondiIngotAdapter::new(
            config.clone(),
            rpc_client,
            signer,
            adapter_db,
        )?.with_submission_channel(submission_tx);

        // Create sync service
        let schema_id = adapter.schema_id_bytes();
        let sync_service = IndexerSyncService::new(
            config.sync.clone(),
            indexer,
            database.clone(),
            schema_id,
        );

        let (block_sender, block_receiver) = mpsc::channel(100);
        let (sync_state_tx, sync_state_rx) = watch::channel(SyncState::Idle);
        let (sync_event_tx, sync_event_rx) = mpsc::channel(100);

        Ok(Self {
            config,
            adapter: Arc::new(adapter),
            sync_service: Arc::new(sync_service),
            database,
            block_receiver,
            block_sender,
            submission_rx,
            sync_state_tx,
            sync_state_rx,
            sync_event_tx,
            sync_event_rx,
        })
    }
}

/// Wrapper to allow using Arc<D> as TondiSubmissionDatabase.
struct ArcDatabase<D>
where
    D: TondiSubmissionDatabase,
{
    inner: Arc<D>,
}

impl<D> TondiSubmissionDatabase for ArcDatabase<D>
where
    D: TondiSubmissionDatabase,
{
    fn get_last_batch_number(&self) -> Result<Option<u64>> {
        self.inner.get_last_batch_number()
    }

    fn get_last_instance_id(&self) -> Result<Option<[u8; 32]>> {
        self.inner.get_last_instance_id()
    }

    fn store_batch(&self, record: &crate::types::BatchRecord) -> Result<()> {
        self.inner.store_batch(record)
    }

    fn get_batch(&self, batch_number: u64) -> Result<Option<crate::types::BatchRecord>> {
        self.inner.get_batch(batch_number)
    }

    fn get_batch_by_start_height(&self, start_height: u64) -> Result<Option<crate::types::BatchRecord>> {
        self.inner.get_batch_by_start_height(start_height)
    }

    fn get_batches_by_status(
        &self,
        status: crate::types::SubmissionStatus,
    ) -> Result<Vec<crate::types::BatchRecord>> {
        self.inner.get_batches_by_status(status)
    }

    fn update_batch(&self, record: &crate::types::BatchRecord) -> Result<()> {
        self.inner.update_batch(record)
    }

    fn get_submitted_height(&self) -> Result<Option<fuel_core_types::fuel_types::BlockHeight>> {
        self.inner.get_submitted_height()
    }

    fn set_submitted_height(&self, height: fuel_core_types::fuel_types::BlockHeight) -> Result<()> {
        self.inner.set_submitted_height(height)
    }

    fn get_last_confirmed_block_hash(&self) -> Result<Option<[u8; 32]>> {
        self.inner.get_last_confirmed_block_hash()
    }

    fn set_last_confirmed_block_hash(&self, hash: [u8; 32]) -> Result<()> {
        self.inner.set_last_confirmed_block_hash(hash)
    }
}

/// Running task for the Tondi adapter service.
pub struct Task<R, S, D>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
{
    config: Config,
    adapter: Arc<TondiIngotAdapter<R, S, D>>,
    block_receiver: mpsc::Receiver<SealedBlock>,
    sync_state_tx: watch::Sender<SyncState>,
    shutdown: StateWatcher,
}

/// Running task with sync service (Plan B: Indexer polling).
pub struct TaskWithSync<R, S, D, I>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
    I: TondiIndexerClient,
{
    config: Config,
    adapter: Arc<TondiIngotAdapter<R, S, ArcDatabase<D>>>,
    sync_service: Arc<IndexerSyncService<I, D>>,
    block_receiver: mpsc::Receiver<SealedBlock>,
    submission_rx: mpsc::Receiver<BatchSubmittedEvent>,
    sync_state_tx: watch::Sender<SyncState>,
    sync_event_tx: mpsc::Sender<SyncEvent>,
    shutdown: StateWatcher,
}

#[async_trait]
impl<R, S, D> RunnableService for NotInitializedTask<R, S, D>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
{
    const NAME: &'static str = "TondiIngotAdapter";

    type SharedData = SharedState;
    type Task = Task<R, S, D>;
    type TaskParams = ();

    fn shared_data(&self) -> Self::SharedData {
        SharedState {
            block_sender: self.block_sender.clone(),
            sync_state: self.sync_state_rx.clone(),
            sync_enabled: false,
        }
    }

    async fn into_task(
        self,
        watcher: &StateWatcher,
        _params: Self::TaskParams,
    ) -> anyhow::Result<Self::Task> {
        let task = Task {
            config: self.config,
            adapter: self.adapter,
            block_receiver: self.block_receiver,
            sync_state_tx: self.sync_state_tx,
            shutdown: watcher.clone(),
        };
        Ok(task)
    }
}

#[async_trait]
impl<R, S, D, I> RunnableService for NotInitializedTaskWithSync<R, S, D, I>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
    I: TondiIndexerClient + 'static,
{
    const NAME: &'static str = "TondiIngotAdapterWithSync";

    type SharedData = SharedState;
    type Task = TaskWithSync<R, S, D, I>;
    type TaskParams = ();

    fn shared_data(&self) -> Self::SharedData {
        SharedState {
            block_sender: self.block_sender.clone(),
            sync_state: self.sync_state_rx.clone(),
            sync_enabled: self.config.sync.enabled,
        }
    }

    async fn into_task(
        self,
        watcher: &StateWatcher,
        _params: Self::TaskParams,
    ) -> anyhow::Result<Self::Task> {
        let task = TaskWithSync {
            config: self.config,
            adapter: self.adapter,
            sync_service: self.sync_service,
            block_receiver: self.block_receiver,
            submission_rx: self.submission_rx,
            sync_state_tx: self.sync_state_tx,
            sync_event_tx: self.sync_event_tx,
            shutdown: watcher.clone(),
        };
        Ok(task)
    }
}

impl<R, S, D> RunnableTask for Task<R, S, D>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
{
    fn run(
        &mut self,
        _watcher: &mut StateWatcher,
    ) -> impl core::future::Future<Output = TaskNextAction> + Send {
        let submission_interval = self.config.submission_interval;

        async move {
            let mut interval = tokio::time::interval(submission_interval);

            tokio::select! {
                biased;

                // Check for shutdown
                _ = self.shutdown.while_started() => {
                    // Flush pending blocks before shutdown
                    if let Err(e) = self.adapter.flush().await {
                        tracing::error!(error = %e, "Failed to flush pending blocks on shutdown");
                    }
                    TaskNextAction::Stop
                }

                // Receive new blocks
                Some(block) = self.block_receiver.recv() => {
                    if let Err(e) = self.adapter.queue_block(block).await {
                        tracing::error!(error = %e, "Failed to queue block");
                        self.update_state(SyncState::Error { message: e.to_string() });
                        TaskNextAction::ErrorContinue(e.into())
                    } else {
                        TaskNextAction::Continue
                    }
                }

                // Periodic submission check
                _ = interval.tick() => {
                    // Check if we should submit
                    let pending_count = self.adapter.pending_block_count().await;
                    let min_batch = self.config.min_batch_size as usize;
                    if pending_count >= min_batch && self.adapter.submit_batch().await.is_err() {
                        tracing::error!("Failed to submit batch");
                        self.update_state(SyncState::Error { message: "Batch submission failed".to_string() });
                        // Continue trying
                    }

                    // Check pending submissions for confirmation
                    if let Err(e) = self.adapter.check_pending_submissions().await {
                        tracing::warn!(error = %e, "Failed to check pending submissions");
                    }

                    // Update state
                    let pending = self.adapter.pending_block_count().await;
                    if pending > 0 {
                        self.update_state(SyncState::Running { last_batch: 0, pending_confirmations: 0 });
                    } else {
                        self.update_state(SyncState::Idle);
                    }

                    TaskNextAction::Continue
                }
            }
        }
    }

    async fn shutdown(self) -> anyhow::Result<()> {
        // Flush any remaining blocks
        self.adapter.flush().await.map_err(anyhow::Error::from)
    }
}

impl<R, S, D, I> RunnableTask for TaskWithSync<R, S, D, I>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
    I: TondiIndexerClient + 'static,
{
    fn run(
        &mut self,
        _watcher: &mut StateWatcher,
    ) -> impl core::future::Future<Output = TaskNextAction> + Send {
        let submission_interval = self.config.submission_interval;
        let sync_poll_interval = self.config.sync.poll_interval;

        async move {
            let mut submission_interval = tokio::time::interval(submission_interval);
            let mut sync_interval = tokio::time::interval(sync_poll_interval);

            tokio::select! {
                biased;

                // Check for shutdown
                _ = self.shutdown.while_started() => {
                    // Flush pending blocks before shutdown
                    if let Err(e) = self.adapter.flush().await {
                        tracing::error!(error = %e, "Failed to flush pending blocks on shutdown");
                    }
                    TaskNextAction::Stop
                }

                // Receive new blocks
                Some(block) = self.block_receiver.recv() => {
                    if let Err(e) = self.adapter.queue_block(block).await {
                        tracing::error!(error = %e, "Failed to queue block");
                        self.update_state(SyncState::Error { message: e.to_string() });
                        TaskNextAction::ErrorContinue(e.into())
                    } else {
                        TaskNextAction::Continue
                    }
                }

                // Receive submission notifications
                Some(event) = self.submission_rx.recv() => {
                    // Track submission in sync service
                    self.sync_service.track_submission(event.batch_number, event.tx_id).await;
                    TaskNextAction::Continue
                }

                // Periodic submission check
                _ = submission_interval.tick() => {
                    // Check if we should submit
                    let pending_count = self.adapter.pending_block_count().await;
                    let min_batch = self.config.min_batch_size as usize;
                    if pending_count >= min_batch && self.adapter.submit_batch().await.is_err() {
                        tracing::error!("Failed to submit batch");
                        self.update_state(SyncState::Error { message: "Batch submission failed".to_string() });
                    }
                    TaskNextAction::Continue
                }

                // Periodic sync check (Plan B: Indexer polling)
                _ = sync_interval.tick() => {
                    if self.config.sync.enabled {
                        match self.sync_service.sync_once().await {
                            Ok(events) => {
                                // Forward sync events
                                for event in events {
                                    let _ = self.sync_event_tx.try_send(event);
                                }

                                // Update state
                                let pending = self.sync_service.pending_count().await;
                                if pending > 0 {
                                    self.update_state(SyncState::Running {
                                        last_batch: 0,
                                        pending_confirmations: pending,
                                    });
                                } else {
                                    self.update_state(SyncState::Idle);
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "Failed to sync with indexer");
                            }
                        }
                    }
                    TaskNextAction::Continue
                }
            }
        }
    }

    async fn shutdown(self) -> anyhow::Result<()> {
        // Flush any remaining blocks
        self.adapter.flush().await.map_err(anyhow::Error::from)
    }
}

impl<R, S, D> Task<R, S, D>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
{
    fn update_state(&self, state: SyncState) {
        let _ = self.sync_state_tx.send(state);
    }
}

impl<R, S, D, I> TaskWithSync<R, S, D, I>
where
    R: TondiRpcClient,
    S: Signer,
    D: TondiSubmissionDatabase,
    I: TondiIndexerClient,
{
    fn update_state(&self, state: SyncState) {
        let _ = self.sync_state_tx.send(state);
    }
}

/// Create a new Tondi adapter service (without sync).
pub fn new_service<R, S, D>(
    config: Config,
    rpc_client: R,
    signer: S,
    database: D,
) -> anyhow::Result<Service<R, S, D>>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
{
    let task = NotInitializedTask::new(config, rpc_client, signer, database)?;
    Ok(ServiceRunner::new(task))
}

/// Create a new Tondi adapter service with L1 sync (Plan B: Indexer polling).
///
/// This service includes:
/// - Block submission to Tondi L1
/// - Confirmation tracking via Indexer polling
/// - Reorg detection and handling
/// - Automatic resubmission of orphaned batches
///
/// Note: The database is wrapped in an Arc for sharing between the adapter
/// and sync service.
pub fn new_service_with_sync<R, S, D, I>(
    config: Config,
    rpc_client: R,
    signer: S,
    database: Arc<D>,
    indexer: I,
) -> anyhow::Result<ServiceWithSync<R, S, D, I>>
where
    R: TondiRpcClient + 'static,
    S: Signer + 'static,
    D: TondiSubmissionDatabase + 'static,
    I: TondiIndexerClient + 'static,
{
    let task = NotInitializedTaskWithSync::new_with_arc_database(
        config,
        rpc_client,
        signer,
        database,
        indexer,
    )?;
    Ok(ServiceRunner::new(task))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Basic compilation test
    #[test]
    fn test_sync_state() {
        let state = SyncState::Idle;
        assert_eq!(state, SyncState::Idle);
        assert!(!state.is_running());

        let state = SyncState::Running {
            last_batch: 5,
            pending_confirmations: 2,
        };
        match state {
            SyncState::Running { last_batch, pending_confirmations } => {
                assert_eq!(last_batch, 5);
                assert_eq!(pending_confirmations, 2);
            }
            _ => panic!("Expected Running state"),
        }
        assert!(state.is_running());

        let state = SyncState::Syncing { l1_daa_score: 100 };
        assert!(state.is_running());

        let state = SyncState::Error { message: "test".to_string() };
        assert!(!state.is_running());
    }
}

