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
        }
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
}

