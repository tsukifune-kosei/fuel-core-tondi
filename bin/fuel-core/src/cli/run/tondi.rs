//! Tondi L1 integration CLI arguments.

use clap::Args;
use fuel_core::service::config::TondiConfig;
use std::time::Duration;

/// Tondi L1 integration arguments for Sovereign Rollup mode.
#[derive(Debug, Clone, Args)]
pub struct TondiArgs {
    /// Enable Tondi L1 block submission.
    /// When enabled, FuelVM operates as a Sovereign Rollup,
    /// submitting block batches to Tondi for data availability.
    #[clap(long = "enable-tondi", env)]
    pub enabled: bool,

    /// Tondi RPC endpoint URL.
    #[clap(
        long = "tondi-rpc-url",
        env,
        default_value = "http://localhost:16210"
    )]
    pub rpc_url: String,

    /// L1 submission interval (aligned with Tondi block time).
    /// Batches are submitted at this interval.
    #[clap(long = "tondi-submission-interval", env, default_value = "12s")]
    pub submission_interval: humantime::Duration,

    /// Maximum blocks per batch.
    #[clap(long = "tondi-max-batch-size", env, default_value = "10")]
    pub max_batch_size: u32,

    /// Minimum blocks before submission.
    #[clap(long = "tondi-min-batch-size", env, default_value = "1")]
    pub min_batch_size: u32,

    /// Force submission timeout (submit even if min_batch_size not reached).
    #[clap(long = "tondi-force-timeout", env, default_value = "30s")]
    pub force_submission_timeout: humantime::Duration,

    /// Schema ID for FuelVM batch payloads.
    /// Default: "fuelvm/batch/v1"
    #[clap(long = "tondi-schema-id", env)]
    pub schema_id: Option<String>,
}

impl Default for TondiArgs {
    fn default() -> Self {
        Self {
            enabled: false,
            rpc_url: "http://localhost:16210".to_string(),
            submission_interval: humantime::Duration::from(Duration::from_secs(12)),
            max_batch_size: 10,
            min_batch_size: 1,
            force_submission_timeout: humantime::Duration::from(Duration::from_secs(30)),
            schema_id: None,
        }
    }
}

impl TondiArgs {
    /// Convert CLI args to config.
    pub fn into_config(self) -> Option<TondiConfig> {
        if !self.enabled {
            return None;
        }

        let rpc_url = url::Url::parse(&self.rpc_url)
            .expect("Invalid Tondi RPC URL");

        Some(TondiConfig {
            rpc_url,
            submission_interval: self.submission_interval.into(),
            max_batch_size: self.max_batch_size,
            min_batch_size: self.min_batch_size,
            force_submission_timeout: self.force_submission_timeout.into(),
            schema_id: self.schema_id,
            metrics: false, // TODO: Add metrics flag
        })
    }
}

