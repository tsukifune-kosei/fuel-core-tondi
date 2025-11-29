//! Configuration for the Tondi Ingot Adapter.

use std::time::Duration;

/// Configuration for Tondi L1 block submission.
#[derive(Debug, Clone)]
pub struct Config {
    /// Tondi RPC endpoint URL.
    pub rpc_url: url::Url,

    /// L1 submission interval (default: 12 seconds, aligned with Tondi block time).
    pub submission_interval: Duration,

    /// Maximum blocks per batch.
    pub max_batch_size: u32,

    /// Minimum blocks before submission (must meet this threshold).
    pub min_batch_size: u32,

    /// Force submission timeout (submit even if min_batch_size not reached).
    pub force_submission_timeout: Duration,

    /// Schema ID for FuelVM batch payloads.
    /// Default: blake3("fuelvm/batch/v1")
    pub schema_id: Option<String>,

    /// Enable metrics collection.
    pub metrics: bool,

    /// Sync configuration for L1 confirmation tracking.
    pub sync: SyncConfig,
}

/// Configuration for L1-L2 sync service (Plan B: Indexer polling).
///
/// This enables reorg-proof sync by polling the Tondi Indexer to:
/// 1. Track batch confirmation status
/// 2. Detect L1 reorgs and handle orphaned batches
/// 3. Schedule resubmission of failed/orphaned batches
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Indexer RPC endpoint URL.
    /// Default: http://localhost:18110 (Tondi Indexer default port)
    pub indexer_url: url::Url,

    /// Polling interval for checking batch status.
    /// Recommended: 2-6 seconds for reasonable latency.
    pub poll_interval: Duration,

    /// Timeout before considering a batch orphaned.
    /// If a batch is not confirmed within this time, check L1 status.
    pub orphan_timeout: Duration,

    /// Maximum resubmission attempts for failed batches.
    pub max_resubmit_attempts: u8,

    /// Number of confirmations required for finality.
    /// For Tondi, typically 10-30 confirmations (DAA score difference).
    pub finality_confirmations: u64,

    /// Enable sync service.
    pub enabled: bool,
}

impl SyncConfig {
    /// Default poll interval (3 seconds).
    pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(3);

    /// Default orphan timeout (45 seconds).
    pub const DEFAULT_ORPHAN_TIMEOUT: Duration = Duration::from_secs(45);

    /// Default max resubmit attempts (3).
    pub const DEFAULT_MAX_RESUBMIT_ATTEMPTS: u8 = 3;

    /// Default finality confirmations (10 DAA score difference).
    pub const DEFAULT_FINALITY_CONFIRMATIONS: u64 = 10;

    /// Create a new SyncConfig with the given indexer URL.
    pub fn new(indexer_url: url::Url) -> Self {
        Self {
            indexer_url,
            poll_interval: Self::DEFAULT_POLL_INTERVAL,
            orphan_timeout: Self::DEFAULT_ORPHAN_TIMEOUT,
            max_resubmit_attempts: Self::DEFAULT_MAX_RESUBMIT_ATTEMPTS,
            finality_confirmations: Self::DEFAULT_FINALITY_CONFIRMATIONS,
            enabled: true,
        }
    }

    /// Disable sync service.
    pub fn disabled() -> Self {
        Self {
            indexer_url: url::Url::parse("http://localhost:18110")
                .expect("default URL should be valid"),
            poll_interval: Self::DEFAULT_POLL_INTERVAL,
            orphan_timeout: Self::DEFAULT_ORPHAN_TIMEOUT,
            max_resubmit_attempts: Self::DEFAULT_MAX_RESUBMIT_ATTEMPTS,
            finality_confirmations: Self::DEFAULT_FINALITY_CONFIRMATIONS,
            enabled: false,
        }
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self::new(
            url::Url::parse("http://localhost:18110")
                .expect("default URL should be valid"),
        )
    }
}

impl Config {
    /// Default submission interval (12 seconds).
    pub const DEFAULT_SUBMISSION_INTERVAL: Duration = Duration::from_secs(12);

    /// Default maximum batch size (10 blocks).
    pub const DEFAULT_MAX_BATCH_SIZE: u32 = 10;

    /// Default minimum batch size (1 block).
    pub const DEFAULT_MIN_BATCH_SIZE: u32 = 1;

    /// Default force submission timeout (30 seconds).
    pub const DEFAULT_FORCE_TIMEOUT: Duration = Duration::from_secs(30);

    /// Default schema identifier.
    pub const DEFAULT_SCHEMA: &'static str = "fuelvm/batch/v1";

    /// Maximum recommended payload size (84.5 KiB as per Ingot spec).
    pub const MAX_RECOMMENDED_PAYLOAD_SIZE: usize = 84 * 1024 + 512;

    /// Default Ingot output value in SAU (Satoshi-like units).
    /// This is the minimum value for an Ingot output on Tondi.
    /// 100,000 SAU = 0.001 TDI (assuming 8 decimal places).
    pub const DEFAULT_INGOT_OUTPUT_VALUE: u64 = 100_000;

    /// Create a new config with the given RPC URL.
    pub fn new(rpc_url: url::Url) -> Self {
        Self {
            rpc_url,
            submission_interval: Self::DEFAULT_SUBMISSION_INTERVAL,
            max_batch_size: Self::DEFAULT_MAX_BATCH_SIZE,
            min_batch_size: Self::DEFAULT_MIN_BATCH_SIZE,
            force_submission_timeout: Self::DEFAULT_FORCE_TIMEOUT,
            schema_id: None,
            metrics: false,
            sync: SyncConfig::default(),
        }
    }

    /// Create a new config with custom sync configuration.
    pub fn with_sync(mut self, sync: SyncConfig) -> Self {
        self.sync = sync;
        self
    }

    /// Get the schema ID bytes (blake3 hash of schema string).
    pub fn schema_id_bytes(&self) -> [u8; 32] {
        let schema = self.schema_id.as_deref().unwrap_or(Self::DEFAULT_SCHEMA);
        *blake3::hash(schema.as_bytes()).as_bytes()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            rpc_url: url::Url::parse("http://localhost:16210")
                .expect("default URL should be valid"),
            submission_interval: Self::DEFAULT_SUBMISSION_INTERVAL,
            max_batch_size: Self::DEFAULT_MAX_BATCH_SIZE,
            min_batch_size: Self::DEFAULT_MIN_BATCH_SIZE,
            force_submission_timeout: Self::DEFAULT_FORCE_TIMEOUT,
            schema_id: None,
            metrics: false,
            sync: SyncConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.submission_interval, Duration::from_secs(12));
        assert_eq!(config.max_batch_size, 10);
        assert_eq!(config.min_batch_size, 1);
        assert!(config.sync.enabled);
    }

    #[test]
    fn test_schema_id_bytes() {
        let config = Config::default();
        let expected = *blake3::hash(b"fuelvm/batch/v1").as_bytes();
        assert_eq!(config.schema_id_bytes(), expected);
    }

    #[test]
    fn test_custom_schema_id() {
        let mut config = Config::default();
        config.schema_id = Some("custom/schema/v2".to_string());
        let expected = *blake3::hash(b"custom/schema/v2").as_bytes();
        assert_eq!(config.schema_id_bytes(), expected);
    }

    #[test]
    fn test_sync_config_default() {
        let sync = SyncConfig::default();
        assert_eq!(sync.poll_interval, Duration::from_secs(3));
        assert_eq!(sync.orphan_timeout, Duration::from_secs(45));
        assert_eq!(sync.max_resubmit_attempts, 3);
        assert_eq!(sync.finality_confirmations, 10);
        assert!(sync.enabled);
    }

    #[test]
    fn test_sync_config_disabled() {
        let sync = SyncConfig::disabled();
        assert!(!sync.enabled);
    }

    #[test]
    fn test_config_with_sync() {
        let config = Config::default();
        let sync = SyncConfig {
            poll_interval: Duration::from_secs(5),
            ..SyncConfig::default()
        };
        let config = config.with_sync(sync);
        assert_eq!(config.sync.poll_interval, Duration::from_secs(5));
    }
}

