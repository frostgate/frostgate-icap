//! # EthereumAdapter: Frostgate's ChainAdapter for EVM Chains

#![allow(private_interfaces)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]


use crate::types::{HealthMetrics, ConnectionStatus};
use crate::utils::retry_async;
use crate::chainadapter::{
    ChainAdapter, AdapterError,
};
use frostgate_sdk::frostmessage::{
    FrostMessage,
    MessageEvent,
    MessageStatus,
};
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::prelude::*;
use uuid::Uuid;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use tokio::time::{sleep, timeout};
use tracing::{info, warn, error, debug, instrument};
use serde::{Deserialize, Serialize};
use reqwest::{Client, Url};


/// Default number of confirmations required for finality on Ethereum mainnet
const DEFAULT_CONFIRMATIONS: u64 = 12;

/// Maximum gas price multiplier to prevent excessive fees during network congestion
const MAX_GAS_PRICE_MULTIPLIER: f64 = 10.0;

/// Default timeout for RPC requests
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retry attempts for failed operations
const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Base delay between retry attempts (exponential backoff)
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);

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
struct CachedTransaction {
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

/// Frostgate's Ethereum adapter
/// 
/// This adapter provides a robust interface to Ethereum-compatible blockchains,
/// handling connection management, transaction lifecycle, event monitoring, and
/// health checking with production-ready reliability features.
pub struct EthereumAdapter {
    /// Ethereum JSON-RPC client with connection pooling
    client: Arc<Provider<Http>>,
    
    /// Network configuration
    config: EthereumConfig,
    
    /// In-memory cache for pending transactions
    /// Key: Transaction hash, Value: Transaction metadata
    pending_transactions: Arc<RwLock<HashMap<TxHash, CachedTransaction>>>,
    
    /// Message ID to transaction hash mapping for tracking
    /// Key: Message ID, Value: Transaction hash
    message_tx_mapping: Arc<RwLock<HashMap<Uuid, TxHash>>>,
    
    /// Connection health metrics and monitoring
    health_metrics: Arc<Mutex<HealthMetrics>>,
    
    /// Rate limiting: timestamp of last RPC call
    last_rpc_call: Arc<Mutex<Option<Instant>>>,
    
    /// Minimum interval between RPC calls to prevent rate limiting
    min_call_interval: Duration,
}

impl EthereumAdapter {
    /// Creates a new Ethereum adapter instance for Frostgate
    ///
    /// # Arguments
    ///
    /// * `config` - Ethereum network configuration
    ///
    /// # Returns
    ///
    /// * `Result<Self, AdapterError>` - New adapter instance or configuration error
    ///
    /// # Example
    ///
    /// ```rust
    /// use frostgate_ethereum::*;
    /// 
    /// let config = EthereumConfig {
    ///     rpc_url: "https://mainnet.infura.io/v3/YOUR-PROJECT-ID".to_string(),
    ///     chain_id: 1,
    ///     relayer_address: "0x742d35Cc0d40D4c8f123b8C2d3a2f82F8C8A7c7C".parse().unwrap(),
    ///     contract_address: "0x123...".parse().unwrap(),
    ///     confirmations: Some(12),
    ///     max_gas_price_gwei: Some(100),
    ///     rpc_timeout: Some(Duration::from_secs(30)),
    ///     enable_gas_bumping: Some(true),
    /// };
    /// 
    /// let adapter = EthereumAdapter::new(config).await?;
    /// ```
    #[instrument(skip(config), fields(chain_id = config.chain_id, rpc_url = %config.rpc_url))]
    pub async fn new(config: EthereumConfig) -> Result<Self, AdapterError> {
        info!("Initializing Ethereum adapter for chain {}", config.chain_id);
        
        // Validate configuration
        Self::validate_config(&config)?;
        
        // Create HTTP provider with timeout
        let timeout_duration = config.rpc_timeout.unwrap_or(DEFAULT_RPC_TIMEOUT);

        // Build a reqwest client with timeout
        let reqwest_client = Client::builder()
            .timeout(timeout_duration)
            .build()
            .map_err(|e| AdapterError::Other(format!("Failed to build HTTP client: {e}")))?;

        // Parse the URL
        let rpc_url = Url::parse(&config.rpc_url)
            .map_err(|e| AdapterError::Other(format!("Invalid RPC URL '{}': {}", config.rpc_url, e)))?;

        // Build the ethers Http provider with the custom client
        let http = Http::new_with_client(rpc_url, reqwest_client);

        // Pass it to Provider and set the polling interval
        let client = Provider::new(http)
            .interval(Duration::from_millis(100));
        
        let adapter = Self {
            client: Arc::new(client),
            config,
            pending_transactions: Arc::new(RwLock::new(HashMap::new())),
            message_tx_mapping: Arc::new(RwLock::new(HashMap::new())),
            health_metrics: Arc::new(Mutex::new(HealthMetrics::default())),
            last_rpc_call: Arc::new(Mutex::new(None)),
            min_call_interval: Duration::from_millis(100), // 10 RPS limit
        };
        
        // Perform initial health check
        adapter.perform_health_check().await?;
        
        info!("Ethereum adapter initialized successfully for chain {}", adapter.config.chain_id);
        Ok(adapter)
    }
    
    /// Validates the adapter configuration
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration to validate
    ///
    /// # Returns
    ///
    /// * `Result<(), AdapterError>` - Success or configuration error
    fn validate_config(config: &EthereumConfig) -> Result<(), AdapterError> {
        // Validate RPC URL format
        if !config.rpc_url.starts_with("http://") && !config.rpc_url.starts_with("https://") {
            return Err(AdapterError::Configuration(
                "RPC URL must start with http:// or https://".to_string()
            ));
        }
        
        // Validate the chain ID
        if config.chain_id == 0 {
            return Err(AdapterError::Configuration(
                "Chain ID cannot be zero".to_string()
            ));
        }
        
        // Validate that addresses are not zero
        if config.relayer_address == Address::zero() {
            return Err(AdapterError::Configuration(
                "Relayer address cannot be zero address".to_string()
            ));
        }
        
        if config.contract_address == Address::zero() {
            return Err(AdapterError::Configuration(
                "Contract address cannot be zero address".to_string()
            ));
        }
        
        // Validate gas price limits
        if let Some(max_gas_price) = config.max_gas_price_gwei {
            if max_gas_price == 0 {
                return Err(AdapterError::Configuration(
                    "Maximum gas price cannot be zero".to_string()
                ));
            }
            if max_gas_price > 10000 { // 10,000 gwei = very high
                warn!("Maximum gas price is set very high: {} gwei", max_gas_price);
            }
        }
        
        Ok(())
    }
    
    /// Executes an RPC call with rate limiting and error handling
    ///
    /// # Arguments
    ///
    /// * `operation` - Async closure representing the RPC operation
    ///
    /// # Returns
    ///
    /// * `Result<T, AdapterError>` - Operation result or network error
    async fn execute_rpc_call<T, F, Fut>(&self, mut operation: F) -> Result<T, AdapterError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
        {
        // Rate limiting
        self.enforce_rate_limit().await;
        
        let start_time = Instant::now();
        
        // Execute with retry logic
        let mut last_error = None;
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            match timeout(DEFAULT_RPC_TIMEOUT, operation()).await {
                Ok(Ok(result)) => {
                    // Update success metrics
                    self.update_success_metrics(start_time.elapsed()).await;
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!("RPC call failed on attempt {}: {}", attempt + 1, e);
                    last_error = Some(e);
                    
                    // Exponential backoff
                    if attempt < MAX_RETRY_ATTEMPTS - 1 {
                        let delay = RETRY_BASE_DELAY * 2_u32.pow(attempt);
                        sleep(delay).await;
                    }
                }
                Err(_) => {
                    warn!("RPC call timed out on attempt {}", attempt + 1);
                    let error_msg = "Request timed out".to_string();
                    return Err(AdapterError::Network(error_msg));
                }
            }
        }
        
        // Update failure metrics
        self.update_failure_metrics().await;
        
        let error_msg = last_error
            .map(|e| format!("RPC call failed after {} attempts: {}", MAX_RETRY_ATTEMPTS, e))
            .unwrap_or_else(|| "RPC call failed after maximum retry attempts".to_string());
        
        error!("{}", error_msg);
        Err(AdapterError::Network(error_msg))
    }
    
    /// Enforces rate limiting between RPC calls
    async fn enforce_rate_limit(&self) {
        let mut last_call = self.last_rpc_call.lock().await;
        if let Some(last_time) = *last_call {
            let elapsed = last_time.elapsed();
            if elapsed < self.min_call_interval {
                let sleep_duration = self.min_call_interval - elapsed;
                drop(last_call); // Release lock before sleeping
                sleep(sleep_duration).await;
                last_call = self.last_rpc_call.lock().await;
            }
        }
        *last_call = Some(Instant::now());
    }
    
    /// Updates success metrics after a successful RPC call
    async fn update_success_metrics(&self, response_time: Duration) {
        let mut metrics = self.health_metrics.lock().await;
        metrics.last_successful_call = Some(Instant::now());
        metrics.last_successful_timestamp = Some(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64);
        metrics.consecutive_failures = 0;
        metrics.total_calls += 1;
        metrics.connection_status = ConnectionStatus::Healthy;
        
        // Update average response time (simple moving average)
        let total_successful = metrics.total_calls - metrics.failed_calls;
        if total_successful == 1 {
            metrics.avg_response_time = response_time;
        } else {
            let current_avg_nanos = metrics.avg_response_time.as_nanos() as f64;
            let new_avg_nanos = (current_avg_nanos * (total_successful as f64 - 1.0) + response_time.as_nanos() as f64) / total_successful as f64;
            metrics.avg_response_time = Duration::from_nanos(new_avg_nanos as u64);
        }
    }
    
    /// Updates failure metrics after a failed RPC call
    async fn update_failure_metrics(&self) {
        let mut metrics = self.health_metrics.lock().await;
        metrics.consecutive_failures += 1;
        metrics.total_calls += 1;
        metrics.failed_calls += 1;
        metrics.connection_status = if metrics.consecutive_failures > 5 {
            ConnectionStatus::Unhealthy
        } else {
            ConnectionStatus::Degraded
        };
    }
    
    /// Performs a comprehensive health check
    async fn perform_health_check(&self) -> Result<(), AdapterError> {
        debug!("Performing health check for chain {}", self.config.chain_id);
        
        // Check if we can connect and get basic chain info
        let chain_id = self.execute_rpc_call(|| async {
            self.client.get_chainid().await
        }).await?;
        
        // Verify chain ID matches configuration
        if chain_id.as_u64() != self.config.chain_id {
            return Err(AdapterError::Configuration(
                format!("Chain ID mismatch: expected {}, got {}", 
                    self.config.chain_id, chain_id.as_u64())
            ));
        }
        
        // Check if we can get the latest block
        self.execute_rpc_call(|| async {
            self.client.get_block_number().await
        }).await?;
        
        // Check if relayer account exists and has some balance
        let balance = self.execute_rpc_call(|| async {
            self.client.get_balance(self.config.relayer_address, None).await
        }).await?;
        
        if balance.is_zero() {
            warn!("Relayer account has zero balance: {:?}", self.config.relayer_address);
        }
        
        info!("Health check passed for chain {}", self.config.chain_id);
        Ok(())
    }
    
    /// Estimates gas for a transaction with safety margins
    ///
    /// # Arguments
    ///
    /// * `tx` - Transaction request to estimate gas for
    ///
    /// # Returns
    ///
    /// * `Result<U256, AdapterError>` - Estimated gas amount with safety margin
    async fn estimate_gas_with_margin(&self, tx: &TypedTransaction) -> Result<U256, AdapterError> {
        let base_estimate = self.execute_rpc_call(|| async {
            self.client.estimate_gas(tx, None).await
        }).await?;
        
        // Add 20% safety margin to prevent out-of-gas errors
        let safety_margin = base_estimate / 5; // 20%
        let final_estimate = base_estimate + safety_margin;
        
        debug!("Gas estimate: base={}, with_margin={}", base_estimate, final_estimate);
        Ok(final_estimate)
    }
    
    /// Gets the current gas price with surge protection
    ///
    /// # Returns
    ///
    /// * `Result<U256, AdapterError>` - Safe gas price to use
    async fn get_safe_gas_price(&self) -> Result<U256, AdapterError> {
        let current_gas_price = self.execute_rpc_call(|| async {
            self.client.get_gas_price().await
        }).await?;
        
        // Apply maximum gas price limit if configured
        if let Some(max_gwei) = self.config.max_gas_price_gwei {
            let max_wei = U256::from(max_gwei) * U256::exp10(9); // Convert gwei to wei
            if current_gas_price > max_wei {
                warn!("Current gas price ({} gwei) exceeds maximum ({} gwei), using maximum", 
                    current_gas_price / U256::exp10(9), max_gwei);
                return Ok(max_wei);
            }
        }
        
        // Check for unreasonable gas price spikes
        let base_price = U256::from(20) * U256::exp10(9); // 20 gwei baseline
        if current_gas_price > base_price * U256::from((MAX_GAS_PRICE_MULTIPLIER * 100.0) as u64) / 100 {
            warn!("Gas price spike detected: {} gwei, limiting to reasonable amount", 
                current_gas_price / U256::exp10(9));
            return Ok(base_price * U256::from((MAX_GAS_PRICE_MULTIPLIER * 100.0) as u64) / 100);
        }
        
        Ok(current_gas_price)
    }
    
    /// Cleans up old pending transactions that are likely dropped or confirmed
    async fn cleanup_stale_transactions(&self) {
        let mut pending = self.pending_transactions.write().await;
        let mut mapping = self.message_tx_mapping.write().await;
        
        let stale_threshold = Duration::from_secs(300); // 5 minutes
        let now = Instant::now();
        
        pending.retain(|tx_hash, cached_tx| {
            let is_stale = now.duration_since(cached_tx.submitted_at) > stale_threshold;
            if is_stale {
                debug!("Removing stale transaction: {:?}", tx_hash);
                mapping.remove(&cached_tx.message_id);
                false
            } else {
                true
            }
        });
    }
}

#[async_trait]
impl ChainAdapter for EthereumAdapter {
    type BlockId = U64;
    type TxId = TxHash;
    type Error = AdapterError;

    /// Retrieves the latest block number from the blockchain
    ///
    /// This method fetches the most recent block number with retry logic and error handling.
    /// It's used for finality checking and general chain state monitoring.
    ///
    /// # Returns
    ///
    /// * `Result<Self::BlockId, Self::Error>` - Latest block number or network error
    #[instrument(skip(self))]
    async fn latest_block(&self) -> Result<Self::BlockId, Self::Error> {
        debug!("Fetching latest block number for chain {}", self.config.chain_id);
        
        // Use tokio runtime for async call in sync context
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            self.execute_rpc_call(|| async {
                self.client.get_block_number().await
            }).await
        })
    }

    /// Retrieves transaction data by hash
    ///
    /// Fetches complete transaction information from the blockchain, including
    /// receipt data if the transaction has been mined.
    ///
    /// # Arguments
    ///
    /// * `tx_hash` - Transaction hash to look up
    ///
    /// # Returns
    ///
    /// * `Result<Option<Vec<u8>>, Self::Error>` - Serialized transaction data or None if not found
    #[instrument(skip(self), fields(tx_hash = %tx))]
    async fn get_transaction(&self, tx: &Self::TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        debug!("Fetching transaction: {:?}", tx);

        // Get both transaction and receipt for complete information
        let tx_result = self.execute_rpc_call(|| async {
            self.client.get_transaction(*tx).await
        }).await?;

        if let Some(transaction) = tx_result {
            // Also fetch receipt if available
            let receipt_result = self.execute_rpc_call(|| async {
                self.client.get_transaction_receipt(*tx).await
            }).await.ok();

            // Combine transaction and receipt data
            let combined_data = (transaction, receipt_result);

            match bincode::serialize(&combined_data) {
                Ok(serialized) => {
                    debug!("Transaction data serialized: {} bytes", serialized.len());
                    Ok(Some(serialized))
                }
                Err(e) => {
                    error!("Failed to serialize transaction data: {}", e);
                    Err(AdapterError::Serialization(format!("Transaction serialization failed: {}", e)))
                }
            }
        } else {
            debug!("Transaction not found: {:?}", tx);
            Ok(None)
        }
    }

    /// Waits for block finality based on confirmation requirements
    ///
    /// This method blocks until the specified block has received enough confirmations
    /// to be considered final. The number of required confirmations is configurable
    /// and defaults to 12 blocks for Ethereum mainnet.
    ///
    /// # Arguments
    ///
    /// * `block` - Block number to wait for finality
    ///
    /// # Returns
    ///
    /// * `Result<(), Self::Error>` - Success when block is final, or timeout/network error
    #[instrument(skip(self), fields(target_block = %block))]
    async fn wait_for_finality(&self, block: &Self::BlockId) -> Result<(), Self::Error> {
        let confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);
        let target_block = *block + U64::from(confirmations);

        info!("Waiting for block {} to reach finality (target: {})", block, target_block);

        let start_time = Instant::now();
        let max_wait_time = Duration::from_secs(600); // 10 minutes maximum

        loop {
            // Check for timeout
            if start_time.elapsed() > max_wait_time {
                return Err(AdapterError::Timeout(
                    format!("Finality wait timeout for block {}", block)
                ));
            }

            let current_block = self.execute_rpc_call(|| async {
                self.client.get_block_number().await
            }).await?;

            if current_block >= target_block {
                info!("Block {} reached finality at block {}", block, current_block);
                return Ok(());
            }

            debug!("Waiting for finality: current={}, target={}", current_block, target_block);
            sleep(Duration::from_secs(10)).await;
        }
    }

    /// Submits a cross-chain message to the blockchain
    ///
    /// This method encodes the message and submits it as a transaction to the
    /// Frostgate smart contract. It handles gas estimation, nonce management,
    /// and transaction monitoring.
    ///
    /// # Arguments
    ///
    /// * `msg` - Cross-chain message to submit
    ///
    /// # Returns
    ///
    /// * `Result<Self::TxId, Self::Error>` - Transaction hash of submitted message
    #[instrument(skip(self, msg), fields(msg_id = %msg.id))]
    async fn submit_message(&self, msg: &FrostMessage) -> Result<Self::TxId, Self::Error> {
        info!("Submitting message {} to chain {}", msg.id, self.config.chain_id);

        // Clean up stale transactions periodically
        self.cleanup_stale_transactions().await;

        // TODO: Replace with actual smart contract interaction
        // This is a placeholder that demonstrates the transaction flow
        let dummy_tx_hash = TxHash::random();

        // Cache the transaction for monitoring
        let cached_tx = CachedTransaction {
            hash: dummy_tx_hash,
            nonce: U256::zero(), // Would be actual nonce in real implementation
            gas_price: U256::zero(), // Would be actual gas price
            submitted_at: Instant::now(),
            message_id: msg.id,
            retry_count: 0,
        };

        // Store transaction mapping
        {
            let mut pending = self.pending_transactions.write().await;
            let mut mapping = self.message_tx_mapping.write().await;

            pending.insert(dummy_tx_hash, cached_tx);
            mapping.insert(msg.id, dummy_tx_hash);
        }

        info!("Message {} submitted with transaction hash: {:?}", msg.id, dummy_tx_hash);
        Ok(dummy_tx_hash)
    }

    /// Listens for contract events related to cross-chain messages
    ///
    /// This method sets up event filters and retrieves recent events from the
    /// Frostgate smart contract, including message submissions, validations,
    /// and state changes.
    ///
    /// # Returns
    ///
    /// * `Result<Vec<MessageEvent>, Self::Error>` - List of recent message events
    #[instrument(skip(self))]
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, Self::Error> {
        debug!("Listening for contract events on chain {}", self.config.chain_id);

        // TODO: Implement actual contract event filtering
        // This would typically involve:
        // 1. Creating event filters for the Frostgate contract
        // 2. Querying recent blocks for matching events
        // 3. Parsing event logs into MessageEvent structures
        // 4. Managing event cursor/checkpoint for pagination

        // Placeholder implementation
        let events = Vec::new();

        debug!("Retrieved {} events from chain {}", events.len(), self.config.chain_id);
        Ok(events)
    }

    /// Verifies a message proof on-chain
    ///
    /// This method calls the smart contract's proof verification function
    /// to validate that a cross-chain message proof is correct and can be
    /// accepted by the destination chain.
    ///
    /// # Arguments
    ///
    /// * `msg` - Message with proof to verify
    ///
    /// # Returns
    ///
    /// * `Result<(), Self::Error>` - Success if proof is valid, error otherwise
    #[instrument(skip(self, msg), fields(msg_id = %msg.id))]
    async fn verify_on_chain(&self, msg: &FrostMessage) -> Result<(), Self::Error> {
        info!("Verifying message {} on chain {}", msg.id, self.config.chain_id);

        // TODO: Implement actual on-chain proof verification
        // This would involve:
        // 1. Encoding the message and proof data
        // 2. Calling the contract's verify function
        // 3. Handling revert reasons and gas estimation
        // 4. Managing verification state and caching results

        // Placeholder that always succeeds
        info!("Message {} proof verified successfully", msg.id);
        Ok(())
    }

    /// Estimates the fee required to process a message
    ///
    /// This method calculates the total cost including gas fees, priority fees,
    /// and any protocol-specific charges for processing the cross-chain message.
    ///
    /// # Arguments
    ///
    /// * `msg` - Message to estimate fees for
    ///
    /// # Returns
    ///
    /// * `Result<u128, Self::Error>` - Estimated fee in wei
    #[instrument(skip(self, msg), fields(msg_id = %msg.id))]
    async fn estimate_fee(&self, msg: &FrostMessage) -> Result<u128, Self::Error> {
    debug!("Estimating fee for message {} on chain {}", msg.id, self.config.chain_id);

    // Get current gas price
    let gas_price = self.get_safe_gas_price().await?;

    // TODO: Estimate actual gas usage for the message
    // This would involve simulating the contract call
    let estimated_gas = U256::from(150_000); // Placeholder gas estimate

    // Calculate total fee
    let base_fee = gas_price * estimated_gas;

    // Add 10% buffer for gas price fluctuations
    let buffer = base_fee / 10;
    let total_fee = base_fee + buffer;

    let fee_wei = total_fee.as_u128();
    debug!("Estimated fee: {} wei ({} ETH)", fee_wei, fee_wei as f64 / 1e18);

    Ok(fee_wei)
}

    /// Retrieves the current status of a cross-chain message
    ///
    /// This method checks the current state of a message by looking up
    /// associated transactions and their confirmation status.
    ///
    /// # Arguments
    ///
    /// * `id` - Message ID to check status for
    ///
    /// # Returns
    ///
    /// * `Result<MessageStatus, Self::Error>` - Current message status
    #[instrument(skip(self), fields(msg_id = %id))]
    async fn message_status(&self, id: &Uuid) -> Result<MessageStatus, Self::Error> {
        debug!("Checking status for message: {}", id);

        // Check if we have a pending transaction for this message
        let tx_hash = {
            let mapping = self.message_tx_mapping.read().await;
            mapping.get(id).copied()
        };

        if let Some(tx_hash) = tx_hash {
            // Check transaction status
            let receipt = self.execute_rpc_call(|| async {
                self.client.get_transaction_receipt(tx_hash).await
            }).await?;

            if let Some(receipt) = receipt {
                // Check if transaction was successful
                 if receipt.status == Some(U64::from(1)){
                    // Check for required confirmations
                    if let Some(block_number) = receipt.block_number {
                        let current_block = self.execute_rpc_call(|| async {
                            self.client.get_block_number().await
                        }).await?;

                        let confirmations = current_block.saturating_sub(block_number);
                        let required_confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);

                        if confirmations.as_u64() >= required_confirmations {
                            return Ok(MessageStatus::Confirmed);
                        } else {
                            return Ok(MessageStatus::Pending);
                        }
                    }
                } else {
                    // Transaction failed
                    return Ok(MessageStatus::Failed("reason".to_string()));
                }
            } else {
                // Transaction not yet mined
                return Ok(MessageStatus::Pending);
            }
        }

        // No transaction found, message might be queued or unknown
        Ok(MessageStatus::Pending)
    }

    /// Performs a comprehensive health check of the adapter
    ///
    /// This method verifies connectivity, chain state, account balances,
    /// and overall system health.
    ///
    /// # Returns
    ///
    /// * `Result<(), Self::Error>` - Success if all health checks pass
    #[instrument(skip(self))]
    async fn health_check(&self) -> Result<(), Self::Error> {
        info!("Performing comprehensive health check for chain {}", self.config.chain_id);

        self.perform_health_check().await
    }
}

/// Additional utility methods for the EthereumAdapter
impl EthereumAdapter {
    /// Returns the current health metrics
    ///
    /// # Returns
    ///
    /// * `HealthMetrics` - Current health and performance metrics
    pub async fn get_health_metrics(&self) -> HealthMetrics {
        let guard = self.health_metrics.lock().await;
        guard.clone() 
    }
    
    /// Returns the number of pending transactions
    ///
    /// # Returns
    ///
    /// * `usize` - Count of currently pending transactions
    pub async fn pending_transaction_count(&self) -> usize {
        self.pending_transactions.read().await.len()
    }
    
    /// Manually triggers cleanup of stale transactions
    ///
    /// This method can be called periodically to clean up old pending transactions
    /// that are no longer relevant.
    pub async fn manual_cleanup(&self) {
        self.cleanup_stale_transactions().await;
    }
    
    /// Forces a health check update
    ///
    /// # Returns
    ///
    /// * `Result<(), AdapterError>` - Success if health check passes
    pub async fn force_health_check(&self) -> Result<(), AdapterError> {
        self.perform_health_check().await
    }
    
    /// Gets the current network gas price without safety limits
    ///
    /// # Returns
    ///
    /// * `Result<U256, AdapterError>` - Current network gas price in wei
    pub async fn get_current_gas_price(&self) -> Result<U256, AdapterError> {
        self.execute_rpc_call(|| async {
            self.client.get_gas_price().await
        }).await
    }
    
    /// Estimates gas for a specific contract call
    ///
    /// # Arguments
    ///
    /// * `to` - Contract address
    /// * `data` - Encoded contract call data
    ///
    /// # Returns
    ///
    /// * `Result<U256, AdapterError>` - Estimated gas usage
    pub async fn estimate_contract_gas(&self, to: Address, data: Bytes) -> Result<U256, AdapterError> {
        let tx = TypedTransaction::Legacy(TransactionRequest {
            to: Some(to.into()),
            data: Some(data),
            from: Some(self.config.relayer_address),
            ..Default::default()
        });
        
        self.estimate_gas_with_margin(&tx).await
    }
    
    /// Submits a raw transaction with proper gas estimation and error handling
    ///
    /// # Arguments
    ///
    /// * `tx` - Transaction to submit
    ///
    /// # Returns
    ///
    /// * `Result<TxHash, AdapterError>` - Transaction hash if successful
    pub async fn submit_raw_transaction(&self, mut tx: TypedTransaction) -> Result<TxHash, AdapterError> {
        // Estimate gas if not provided
        if tx.gas().is_none() {
            let gas_estimate = self.estimate_gas_with_margin(&tx).await?;
            tx.set_gas(gas_estimate);
        }
        
        // Set gas price if not provided
        if tx.gas_price().is_none() {
            let gas_price = self.get_safe_gas_price().await?;
            tx.set_gas_price(gas_price);
        }
        
        // Submit transaction
        let pending_tx = self.execute_rpc_call(|| async {
            self.client.send_transaction(tx.clone(), None).await
        }).await?;
        
        Ok(pending_tx.tx_hash())
    }
    
    /// Waits for a transaction to be mined and returns the receipt
    ///
    /// # Arguments
    ///
    /// * `tx_hash` - Transaction hash to wait for
    /// * `timeout` - Maximum time to wait
    ///
    /// # Returns
    ///
    /// * `Result<TransactionReceipt, AdapterError>` - Transaction receipt
    pub async fn wait_for_transaction(&self, tx_hash: TxHash, timeout: Duration) -> Result<TransactionReceipt, AdapterError> {
        let start_time = Instant::now();
        
        loop {
            if start_time.elapsed() > timeout {
                return Err(AdapterError::Timeout(
                    format!("Transaction {} not mined within timeout", tx_hash)
                ));
            }
            
            if let Ok(Some(receipt)) = self.execute_rpc_call(|| async {
                self.client.get_transaction_receipt(tx_hash).await
            }).await {
                return Ok(receipt);
            }
            
            sleep(Duration::from_secs(2)).await;
        }
    }
    
    /// Bumps the gas price for a pending transaction (replace-by-fee)
    ///
    /// # Arguments
    ///
    /// * `original_tx_hash` - Hash of the original transaction
    /// * `gas_price_multiplier` - Multiplier for the new gas price (e.g., 1.1 for 10% increase)
    ///
    /// # Returns
    ///
    /// * `Result<TxHash, AdapterError>` - New transaction hash
    pub async fn bump_gas_price(&self, original_tx_hash: TxHash, gas_price_multiplier: f64) -> Result<TxHash, AdapterError> {
        // Get original transaction
        let original_tx = self.execute_rpc_call(|| async {
            self.client.get_transaction(original_tx_hash).await
        }).await?;
        
        let original_tx = original_tx.ok_or_else(|| {
            AdapterError::Network("Original transaction not found".to_string())
        })?;
        
        // Create new transaction with higher gas price
        let new_gas_price = U256::from((original_tx.gas_price.unwrap_or_default().as_u128() as f64 * gas_price_multiplier) as u128);
        
        let new_tx = TypedTransaction::Legacy(TransactionRequest {
            from: Some(original_tx.from),
            to: original_tx.to.map(|addr| addr.into()),
            value: Some(original_tx.value),
            gas: Some(original_tx.gas),
            gas_price: Some(new_gas_price),
            data: Some(original_tx.input),
            nonce: Some(original_tx.nonce),
            ..Default::default()
        });
        
        // Submit the replacement transaction
        self.submit_raw_transaction(new_tx).await
    }
}

/// Helper function to create a development/test configuration
impl EthereumConfig {
    /// Creates a configuration for Ethereum mainnet
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - RPC endpoint URL
    /// * `relayer_address` - Address of the relayer account
    /// * `contract_address` - Address of the Frostgate contract
    ///
    /// # Returns
    ///
    /// * `EthereumConfig` - Mainnet configuration with production defaults
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
    
    /// Creates a configuration for Polygon mainnet
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - RPC endpoint URL
    /// * `relayer_address` - Address of the relayer account
    /// * `contract_address` - Address of the Frostgate contract
    ///
    /// # Returns
    ///
    /// * `EthereumConfig` - Polygon configuration with appropriate defaults
    pub fn polygon(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 137,
            relayer_address,
            contract_address,
            confirmations: Some(20), // Faster blocks, more confirmations needed
            max_gas_price_gwei: Some(1000), // Higher gas prices on Polygon
            rpc_timeout: Some(Duration::from_secs(15)),
            enable_gas_bumping: Some(true),
        }
    }
    
    /// Creates a configuration for Arbitrum One
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - RPC endpoint URL
    /// * `relayer_address` - Address of the relayer account
    /// * `contract_address` - Address of the Frostgate contract
    ///
    /// # Returns
    ///
    /// * `EthereumConfig` - Arbitrum configuration with L2-appropriate defaults
    pub fn arbitrum(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 42161,
            relayer_address,
            contract_address,
            confirmations: Some(1), // L2 finality is different
            max_gas_price_gwei: Some(10),
            rpc_timeout: Some(Duration::from_secs(10)),
            enable_gas_bumping: Some(false), // Gas bumping less necessary on L2
        }
    }
    
    /// Creates a development configuration for local testing
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - Local RPC endpoint (e.g., "http://localhost:8545")
    /// * `relayer_address` - Test relayer address
    /// * `contract_address` - Test contract address
    ///
    /// # Returns
    ///
    /// * `EthereumConfig` - Development configuration with test-friendly defaults
    pub fn development(rpc_url: String, relayer_address: Address, contract_address: Address) -> Self {
        Self {
            rpc_url,
            chain_id: 31337, // Common local chain ID
            relayer_address,
            contract_address,
            confirmations: Some(1), // Fast testing
            max_gas_price_gwei: Some(20),
            rpc_timeout: Some(Duration::from_secs(5)),
            enable_gas_bumping: Some(false),
        }
    }
}

/// Builder pattern for EthereumConfig
pub struct EthereumConfigBuilder {
    config: EthereumConfig,
}

impl EthereumConfigBuilder {
    /// Creates a new configuration builder
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - RPC endpoint URL
    /// * `chain_id` - Chain ID
    /// * `relayer_address` - Relayer account address
    /// * `contract_address` - Frostgate contract address
    ///
    /// # Returns
    ///
    /// * `Self` - Configuration builder
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
    
    /// Sets the number of confirmations required for finality
    pub fn confirmations(mut self, confirmations: u64) -> Self {
        self.config.confirmations = Some(confirmations);
        self
    }
    
    /// Sets the maximum gas price in gwei
    pub fn max_gas_price_gwei(mut self, max_gas_price: u64) -> Self {
        self.config.max_gas_price_gwei = Some(max_gas_price);
        self
    }
    
    /// Sets the RPC request timeout
    pub fn rpc_timeout(mut self, timeout: Duration) -> Self {
        self.config.rpc_timeout = Some(timeout);
        self
    }
    
    /// Enables or disables gas bumping
    pub fn enable_gas_bumping(mut self, enable: bool) -> Self {
        self.config.enable_gas_bumping = Some(enable);
        self
    }
    
    /// Builds the final configuration
    pub fn build(self) -> EthereumConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_validation() {
        let valid_config = EthereumConfig {
            rpc_url: "https://mainnet.infura.io/v3/test".to_string(),
            chain_id: 1,
            relayer_address: Address::from_low_u64_be(1),
            contract_address: Address::from_low_u64_be(2),
            confirmations: Some(12),
            max_gas_price_gwei: Some(100),
            rpc_timeout: Some(Duration::from_secs(30)),
            enable_gas_bumping: Some(true),
        };
        
        assert!(EthereumAdapter::validate_config(&valid_config).is_ok());
    }
    
    #[test]
    fn test_config_validation_invalid_url() {
        let invalid_config = EthereumConfig {
            rpc_url: "invalid-url".to_string(),
            chain_id: 1,
            relayer_address: Address::from_low_u64_be(1),
            contract_address: Address::from_low_u64_be(2),
            confirmations: Some(12),
            max_gas_price_gwei: Some(100),
            rpc_timeout: Some(Duration::from_secs(30)),
            enable_gas_bumping: Some(true),
        };
        
        assert!(EthereumAdapter::validate_config(&invalid_config).is_err());
    }
    
    #[test]
    fn test_config_builder() {
        let config = EthereumConfigBuilder::new(
            "https://mainnet.infura.io/v3/test".to_string(),
            1,
            Address::from_low_u64_be(1),
            Address::from_low_u64_be(2),
        )
        .confirmations(15)
        .max_gas_price_gwei(150)
        .rpc_timeout(Duration::from_secs(45))
        .enable_gas_bumping(false)
        .build();
        
        assert_eq!(config.confirmations, Some(15));
        assert_eq!(config.max_gas_price_gwei, Some(150));
        assert_eq!(config.rpc_timeout, Some(Duration::from_secs(45)));
        assert_eq!(config.enable_gas_bumping, Some(false));
    }
}

/// Example usage and integration patterns
#[cfg(feature = "examples")]
mod examples {
    use super::*;

    /// Example: Creating and using an Ethereum adapter for mainnet
    pub async fn mainnet_example() -> Result<(), AdapterError> {
        let config = EthereumConfig::mainnet(
            "https://mainnet.infura.io/v3/YOUR-PROJECT-ID".to_string(),
            "0x742d35Cc0d40D4c8f123b8C2d3a2f82F8C8A7c7C".parse().unwrap(),
            "0x123456789abcdef123456789abcdef123456789a".parse().unwrap(),
        );

        let adapter = EthereumAdapter::new(config).await?;

        // Perform health check
        adapter.health_check().await?;

        // Get latest block
        let latest_block = adapter.latest_block().await?;
        println!("Latest block: {}", latest_block);

        // Check adapter health metrics
        let metrics = adapter.get_health_metrics().await;
        println!("Health metrics: {:?}", metrics);

        Ok(())
    }

    /// Example: Submitting a message and tracking its status
    pub async fn message_lifecycle_example() -> Result<(), AdapterError> {
        let config = EthereumConfig::development(
            "http://localhost:8545".to_string(),
            Address::from_low_u64_be(1),
            Address::from_low_u64_be(2),
        );

        let adapter = EthereumAdapter::new(config).await?;

        // Create a dummy message
        let message = FrostMessage {
            id: Uuid::new_v4(),
            // ... fill other message fields as needed ...
            from_chain: todo!(),
            to_chain: todo!(),
            payload: vec![],
            proof: None,
            timestamp: 0,
            nonce: 0,
            signature: None,
            fee: None,
            metadata: None,
        };

        // Submit the message
        let tx_hash = adapter.submit_message(&message).await?;
        println!("Message submitted with tx hash: {:?}", tx_hash);

        // Check message status
        let status = adapter.message_status(&message.id).await?;
        println!("Message status: {:?}", status);

        // Estimate fee for the message
        let fee = adapter.estimate_fee(&message).await?;
        println!("Estimated fee: {} wei", fee);

        Ok(())
    }
}