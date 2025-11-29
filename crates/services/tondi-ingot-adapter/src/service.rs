//! Background service for Tondi block submission.
//!
//! This module implements the `RunnableService` trait for the Tondi adapter,
//! enabling it to run as a background task that processes blocks and submits
//! batches to Tondi L1.

use crate::{
    adapter::TondiIngotAdapter,
    config::Config,
    error::{
        Result,
        TondiAdapterError,
    },
    ports::{
        Signer,
        TondiRpcClient,
        TondiSubmissionDatabase,
    },
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

/// Submission sync state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Adapter is running and submitting batches.
    Running {
        /// Last submitted batch number.
        last_batch: u64,
    },
    /// Adapter is idle (no pending blocks).
    Idle,
    /// Adapter encountered an error.
    Error,
}

/// Type alias for the service.
pub type Service<R, S, D> = ServiceRunner<NotInitializedTask<R, S, D>>;

/// Shared state exposed by the service.
#[derive(Clone)]
pub struct SharedState {
    /// Channel to send blocks for submission.
    block_sender: mpsc::Sender<SealedBlock>,
    /// Current sync state.
    sync_state: watch::Receiver<SyncState>,
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
        *self.sync_state.borrow()
    }

    /// Wait for a state change.
    pub async fn wait_for_change(&mut self) -> Result<SyncState> {
        self.sync_state
            .changed()
            .await
            .map_err(|_| TondiAdapterError::Shutdown)?;
        Ok(*self.sync_state.borrow())
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
                        self.update_state(SyncState::Error);
                        TaskNextAction::ErrorContinue(e.into())
                    } else {
                        TaskNextAction::Continue
                    }
                }

                // Periodic submission check
                _ = interval.tick() => {
                    // Check if we should submit
                    let pending_count = self.adapter.pending_block_count().await;
                    if pending_count >= self.config.min_batch_size as usize {
                        if let Err(e) = self.adapter.submit_batch().await {
                            tracing::error!(error = %e, "Failed to submit batch");
                            self.update_state(SyncState::Error);
                            // Continue trying
                        }
                    }

                    // Check pending submissions for confirmation
                    if let Err(e) = self.adapter.check_pending_submissions().await {
                        tracing::warn!(error = %e, "Failed to check pending submissions");
                    }

                    // Update state
                    let pending = self.adapter.pending_block_count().await;
                    if pending > 0 {
                        self.update_state(SyncState::Running { last_batch: 0 });
                    } else {
                        self.update_state(SyncState::Idle);
                    }

                    TaskNextAction::Continue
                }
            }
        }
    }

    fn shutdown(self) -> impl core::future::Future<Output = anyhow::Result<()>> + Send {
        async move {
            // Flush any remaining blocks
            self.adapter.flush().await.map_err(anyhow::Error::from)
        }
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

/// Create a new Tondi adapter service.
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

#[cfg(test)]
mod tests {
    use super::*;

    // Basic compilation test
    #[test]
    fn test_sync_state() {
        let state = SyncState::Idle;
        assert_eq!(state, SyncState::Idle);

        let state = SyncState::Running { last_batch: 5 };
        match state {
            SyncState::Running { last_batch } => assert_eq!(last_batch, 5),
            _ => panic!("Expected Running state"),
        }
    }
}

