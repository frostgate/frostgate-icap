use solana_sdk::{
    pubkey::Pubkey,
    commitment_config::CommitmentLevel,
};
use std::time::{SystemTime, Duration};
use uuid::Uuid;
use thiserror::Error;
use frostgate_sdk::message::MessageStatus;

/// Configuration parameters for the Solana adapter
#[derive(Debug, Clone)]
pub struct SolanaConfig {
    /// RPC endpoint URL for Solana node communication
    pub rpc_url: String,
    
    /// Public key of the relayer account used for transaction submission
    pub relayer_pubkey: Pubkey,
    
    /// Commitment level for transaction confirmation (default: Confirmed)
    pub commitment: CommitmentLevel,
    
    /// Maximum time to wait for finality confirmation in seconds (default: 60)
    pub finality_timeout_secs: u64,
    
    /// Polling interval for finality checks in milliseconds (default: 1000)
    pub finality_poll_interval_ms: u64,
    
    /// Maximum number of retry attempts for RPC calls (default: 3)
    pub max_retries: u32,
    
    /// Base delay between retry attempts in milliseconds (default: 1000)
    pub retry_delay_ms: u64,
    
    /// Enable exponential backoff for retries (default: true)
    pub exponential_backoff: bool,
}

impl Default for SolanaConfig {
    fn default() -> Self {
        Self {
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            relayer_pubkey: Pubkey::default(),
            commitment: CommitmentLevel::Confirmed,
            finality_timeout_secs: 60,
            finality_poll_interval_ms: 1000,
            max_retries: 3,
            retry_delay_ms: 1000,
            exponential_backoff: true,
        }
    }
}

/// Builder pattern for SolanaConfig
#[derive(Default)]
pub struct SolanaConfigBuilder {
    config: SolanaConfig,
}

impl SolanaConfigBuilder {
    pub fn rpc_url<S: Into<String>>(mut self, url: S) -> Self {
        self.config.rpc_url = url.into();
        self
    }
    
    pub fn relayer_pubkey(mut self, pubkey: Pubkey) -> Self {
        self.config.relayer_pubkey = pubkey;
        self
    }
    
    pub fn commitment(mut self, commitment: CommitmentLevel) -> Self {
        self.config.commitment = commitment;
        self
    }
    
    pub fn finality_timeout_secs(mut self, timeout: u64) -> Self {
        self.config.finality_timeout_secs = timeout;
        self
    }
    
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.config.max_retries = retries;
        self
    }
    
    pub fn build(self) -> Result<SolanaConfig, SolanaAdapterError> {
        // Validate configuration
        if self.config.rpc_url.is_empty() {
            return Err(SolanaAdapterError::InvalidConfiguration(
                "RPC URL cannot be empty".to_string()
            ));
        }
        
        if self.config.finality_timeout_secs == 0 {
            return Err(SolanaAdapterError::InvalidConfiguration(
                "Finality timeout must be greater than 0".to_string()
            ));
        }
        
        Ok(self.config)
    }
}

impl SolanaConfig {
    /// Create a new configuration builder
    pub fn builder() -> SolanaConfigBuilder {
        SolanaConfigBuilder::default()
    }

    /// Creates a configuration for Solana mainnet
    pub fn mainnet(rpc_url: String, relayer_pubkey: Pubkey) -> Self {
        Self {
            rpc_url,
            relayer_pubkey,
            commitment: CommitmentLevel::Confirmed,
            finality_timeout_secs: 60,
            finality_poll_interval_ms: 1000,
            max_retries: 3,
            retry_delay_ms: 1000,
            exponential_backoff: true,
        }
    }

    /// Creates a configuration for Solana devnet
    pub fn devnet(rpc_url: String, relayer_pubkey: Pubkey) -> Self {
        Self {
            rpc_url,
            relayer_pubkey,
            commitment: CommitmentLevel::Confirmed,
            finality_timeout_secs: 30,
            finality_poll_interval_ms: 500,
            max_retries: 5,
            retry_delay_ms: 500,
            exponential_backoff: true,
        }
    }

    /// Creates a configuration for local development/testing
    pub fn localnet(rpc_url: String, relayer_pubkey: Pubkey) -> Self {
        Self {
            rpc_url,
            relayer_pubkey,
            commitment: CommitmentLevel::Processed,
            finality_timeout_secs: 10,
            finality_poll_interval_ms: 100,
            max_retries: 2,
            retry_delay_ms: 200,
            exponential_backoff: false,
        }
    }
}

/// Specialized error types for Solana adapter operations
#[derive(Error, Debug)]
pub enum SolanaAdapterError {
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("RPC client error: {0}")]
    RpcClient(#[from] solana_client::client_error::ClientError),
    
    #[error("Transaction error: {0}")]
    Transaction(String),
    
    #[error("Timeout error: {0}")]
    Timeout(String),
    
    #[error("Serialization error: {0}")]
    Serialization(String),
    
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
    
    #[error("Message not found: {0}")]
    MessageNotFound(String),
    
    #[error("Verification failed: {0}")]
    VerificationFailed(String),
}

/// Internal message tracking structure
#[derive(Debug, Clone)]
pub struct TrackedMessage {
    /// Unique identifier for the message
    pub id: Uuid,
    
    /// Transaction signature on Solana
    pub signature: Option<String>,
    
    /// Current status of the message
    pub status: MessageStatus,
    
    /// Timestamp when message was submitted
    pub submitted_at: SystemTime,
    
    /// Block slot where transaction was included (if any)
    pub block_slot: Option<u64>,
}

/// Constants for Solana adapter
pub mod constants {
    use super::*;

    /// Default RPC timeout duration
    pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

    /// Default number of confirmations for finality
    pub const DEFAULT_CONFIRMATIONS: u8 = 32;

    /// Maximum time to wait for transaction confirmation
    pub const MAX_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(90);

    /// Default interval between status checks
    pub const DEFAULT_STATUS_CHECK_INTERVAL: Duration = Duration::from_secs(1);

    /// Maximum cache age for tracked messages
    pub const MAX_CACHE_AGE_HOURS: u64 = 24;

    /// Default commitment for transaction submission
    pub const DEFAULT_COMMITMENT: CommitmentLevel = CommitmentLevel::Confirmed;

    /// Maximum number of concurrent RPC requests
    pub const MAX_CONCURRENT_REQUESTS: usize = 10;

    /// Default rate limiting: minimum interval between RPC calls
    pub const MIN_CALL_INTERVAL: Duration = Duration::from_millis(50);

    /// Maximum number of retries for failed RPC calls
    pub const MAX_RPC_RETRIES: u32 = 3;

    /// Base delay between retry attempts
    pub const BASE_RETRY_DELAY: Duration = Duration::from_millis(1000);

    /// Maximum delay between retry attempts
    pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

    /// Default health check interval
    pub const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

    /// Maximum number of failed health checks before marking as unhealthy
    pub const MAX_CONSECUTIVE_HEALTH_FAILURES: u32 = 5;
}

