// Chain specific types, logs, recipeints, etc. Handling the chain's specific data structure

use std::time::Duration;
use ethers::types::Address;
use serde::{Deserialize, Serialize};
use std::time::Instant;
use ethers::types::TxHash;
use uuid::Uuid;
use ethers::types::U256;

/// Default number of confirmations required for finality on Ethereum mainnet
pub const DEFAULT_CONFIRMATIONS: u64 = 12;

/// Maximum gas price multiplier to prevent excessive fees during network congestion
pub const MAX_GAS_PRICE_MULTIPLIER: f64 = 10.0;

/// Default timeout for RPC requests
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retry attempts for failed operations
pub const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Base delay between retry attempts (exponential backoff)
pub const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);

/// Configuration parameters for the Ethereum adapter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthereumConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Chain ID for the target EVM network
    pub chain_id: u64,
    /// Address of the relayer account
    pub relayer_address: Address,
    /// Address of the Frostgate smart contract
    pub contract_address: Address,
    /// Number of block confirmations required for finality
    pub confirmations: Option<u64>,
    /// Maximum gas price in gwei (None for automatic)
    pub max_gas_price_gwei: Option<u64>,
    /// RPC request timeout
    pub rpc_timeout: Option<Duration>,
    /// Enable/disable transaction resubmission on low gas
    pub enable_gas_bumping: Option<bool>,
}

/// Cached transaction information for tracking and resubmission
#[derive(Debug, Clone)]
pub struct CachedTransaction {
    /// Original transaction hash
    pub hash: TxHash,
    /// Transaction nonce
    pub nonce: U256,
    /// Gas price used
    pub gas_price: U256,
    /// Timestamp when transaction was submitted
    pub submitted_at: Instant,
    /// Associated message ID
    pub message_id: Uuid,
    /// Current retry count
    pub retry_count: u32,
}

/// Builder pattern for EthereumConfig
pub struct EthereumConfigBuilder {
    config: EthereumConfig,
}

impl EthereumConfigBuilder {
    /// Creates a new configuration builder
    pub fn new(rpc_url: String, chain_id: u64, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            config: EthereumConfig {
                rpc_url,
                chain_id,
                relayer_address,
                contract_address,
                confirmations: None,
                max_gas_price_gwei: None,
                rpc_timeout: None,
                enable_gas_bumping: None,
            }
        }
    }
    
    pub fn confirmations(mut self, confirmations: u64) -> Self {
        self.config.confirmations = Some(confirmations);
        self
    }
    
    pub fn max_gas_price_gwei(mut self, max_gas_price: u64) -> Self {
        self.config.max_gas_price_gwei = Some(max_gas_price);
        self
    }
    
    pub fn rpc_timeout(mut self, timeout: Duration) -> Self {
        self.config.rpc_timeout = Some(timeout);
        self
    }
    
    pub fn enable_gas_bumping(mut self, enable: bool) -> Self {
        self.config.enable_gas_bumping = Some(enable);
        self
    }
    
    pub fn build(self) -> EthereumConfig {
        self.config
    }
}

impl EthereumConfig {
    pub fn mainnet(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 1,
            relayer_address,
            contract_address,
            confirmations: Some(12),
            max_gas_price_gwei: Some(100),
            rpc_timeout: Some(Duration::from_secs(30)),
            enable_gas_bumping: Some(true),
        }
    }
    
    pub fn polygon(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 137,
            relayer_address,
            contract_address,
            confirmations: Some(20),
            max_gas_price_gwei: Some(1000),
            rpc_timeout: Some(Duration::from_secs(15)),
            enable_gas_bumping: Some(true),
        }
    }
    
    pub fn arbitrum(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 42161,
            relayer_address,
            contract_address,
            confirmations: Some(1),
            max_gas_price_gwei: Some(10),
            rpc_timeout: Some(Duration::from_secs(10)),
            enable_gas_bumping: Some(false),
        }
    }
    
    pub fn development(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 31337,
            relayer_address,
            contract_address,
            confirmations: Some(1),
            max_gas_price_gwei: Some(20),
            rpc_timeout: Some(Duration::from_secs(5)),
            enable_gas_bumping: Some(false),
        }
    }
}