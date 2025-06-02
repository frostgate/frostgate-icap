#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]


use std::time::Duration;
use std::collections::HashMap;
use subxt::utils::{H256, AccountId32};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use uuid::Uuid;

/// Default number of confirmations required for Substrate finality
pub const DEFAULT_CONFIRMATIONS: u32 = 12;

/// Default RPC request timeout duration
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of retry attempts for failed operations
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Base delay for exponential backoff retry strategy
pub const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);

/// Maximum delay between retry attempts
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Default rate limiting: minimum interval between RPC calls
pub const DEFAULT_MIN_CALL_INTERVAL: Duration = Duration::from_millis(100);

/// Default maximum concurrent RPC requests
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: u32 = 10;

/// Default health check interval
pub const DEFAULT_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time to wait for finality before timing out
pub const MAX_FINALITY_WAIT_TIME: Duration = Duration::from_secs(600);

/// Average Substrate block time (used for polling intervals)
pub const SUBSTRATE_BLOCK_TIME: Duration = Duration::from_secs(6);

/// Maximum number of failed health checks before marking as unhealthy
pub const MAX_CONSECUTIVE_HEALTH_FAILURES: u32 = 5;

/// Configuration for the PolkadotAdapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolkadotConfig {
    /// WebSocket or HTTP RPC endpoint URL
    pub rpc_url: String,
    
    /// Signer seed phrase or private key for transaction signing
    pub signer_seed: String,
    
    /// Number of block confirmations required for finality
    pub confirmations: Option<u32>,
    
    /// Timeout for individual RPC requests
    pub rpc_timeout: Option<Duration>,
    
    /// Maximum number of concurrent RPC requests
    pub max_concurrent_requests: Option<u32>,
    
    /// Interval between automated health checks
    pub health_check_interval: Option<Duration>,
    
    /// Minimum interval between RPC calls for rate limiting
    pub min_call_interval: Option<Duration>,
    
    /// Custom pallet name for cross-chain messaging
    pub message_pallet: Option<String>,
    
    /// Chain-specific configuration overrides
    pub chain_specific: Option<ChainSpecificConfig>,
}

/// Chain-specific configuration parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSpecificConfig {
    /// Chain identifier (e.g., "polkadot", "kusama", "acala")
    pub chain_id: String,
    
    /// Expected average block time for this chain
    pub block_time: Option<Duration>,
    
    /// Chain-specific finality requirements
    pub finality_blocks: Option<u32>,
    
    /// Custom RPC methods or endpoints
    pub custom_rpc_methods: Option<HashMap<String, String>>,
}

/// Transaction tracking information
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    /// Original message ID
    pub message_id: Uuid,
    
    /// Substrate transaction hash
    pub tx_hash: H256,
    
    /// Block number where transaction was included
    pub block_number: Option<u32>,
    
    /// Transaction status
    pub status: TxTrackingStatus,
    
    /// Timestamp when transaction was submitted
    pub submitted_at: Instant,
    
    /// Number of confirmation blocks seen
    pub confirmations: u32,
    
    /// Any error encountered during processing
    pub error: Option<String>,
}

/// Internal transaction tracking status
#[derive(Debug, Clone, PartialEq)]
pub enum TxTrackingStatus {
    /// Transaction has been submitted to mempool
    Submitted,
    /// Transaction is included in a block
    InBlock,
    /// Transaction is finalized
    Finalized,
    /// Transaction failed or was rejected
    Failed,
    /// Transaction status is unknown
    Unknown,
}

impl PolkadotConfig {
    /// Creates a configuration for Polkadot mainnet
    pub fn mainnet(rpc_url: String, signer_seed: String) -> Self {
        Self {
            rpc_url,
            signer_seed,
            confirmations: Some(DEFAULT_CONFIRMATIONS),
            rpc_timeout: Some(DEFAULT_RPC_TIMEOUT),
            max_concurrent_requests: Some(DEFAULT_MAX_CONCURRENT_REQUESTS),
            health_check_interval: Some(DEFAULT_HEALTH_CHECK_INTERVAL),
            min_call_interval: Some(DEFAULT_MIN_CALL_INTERVAL),
            message_pallet: Some("XcmpQueue".to_string()),
            chain_specific: Some(ChainSpecificConfig {
                chain_id: "polkadot".to_string(),
                block_time: Some(SUBSTRATE_BLOCK_TIME),
                finality_blocks: Some(DEFAULT_CONFIRMATIONS),
                custom_rpc_methods: None,
            }),
        }
    }

    /// Creates a configuration for Kusama network
    pub fn kusama(rpc_url: String, signer_seed: String) -> Self {
        Self {
            rpc_url,
            signer_seed,
            confirmations: Some(6),
            rpc_timeout: Some(Duration::from_secs(20)),
            max_concurrent_requests: Some(DEFAULT_MAX_CONCURRENT_REQUESTS),
            health_check_interval: Some(DEFAULT_HEALTH_CHECK_INTERVAL),
            min_call_interval: Some(DEFAULT_MIN_CALL_INTERVAL),
            message_pallet: Some("XcmpQueue".to_string()),
            chain_specific: Some(ChainSpecificConfig {
                chain_id: "kusama".to_string(),
                block_time: Some(Duration::from_secs(6)),
                finality_blocks: Some(6),
                custom_rpc_methods: None,
            }),
        }
    }

    /// Creates a configuration for development/testing
    pub fn development(rpc_url: String, signer_seed: String) -> Self {
        Self {
            rpc_url,
            signer_seed,
            confirmations: Some(1),
            rpc_timeout: Some(Duration::from_secs(10)),
            max_concurrent_requests: Some(5),
            health_check_interval: None,
            min_call_interval: Some(Duration::from_millis(50)),
            message_pallet: None,
            chain_specific: Some(ChainSpecificConfig {
                chain_id: "dev".to_string(),
                block_time: Some(Duration::from_secs(3)),
                finality_blocks: Some(1),
                custom_rpc_methods: None,
            }),
        }
    }
}
