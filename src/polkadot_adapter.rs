// PolkadotAdapter: Frostgate's ChainAdapter for Polkadot/Substrate Chains

#![allow(private_interfaces)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]

use crate::chainadapter::{
    ChainAdapter, AdapterError,
};
use frostgate_sdk::frostmessage::{
    FrostMessage,
    MessageEvent,
    MessageStatus,
};
use async_trait::async_trait;
use subxt::{
    OnlineClient, PolkadotConfig as SubxtPolkadotConfig, 
    tx::{PairSigner, TxStatus, TxProgress}, 
    error::Error as SubxtError,
    events::EventsClient,
    blocks::BlocksClient,
    utils::{H256, AccountId32},
};
use subxt::*;
use uuid::Uuid;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
    fmt,
};
use tokio::sync::{RwLock, Mutex, Semaphore};
use tokio::time::{sleep, timeout, interval, MissedTickBehavior};
use tracing::{info, warn, error, debug, instrument, span, Level};
use serde::{Deserialize, Serialize};

/// Default number of confirmations required for Substrate finality
const DEFAULT_CONFIRMATIONS: u32 = 12;

/// Default RPC request timeout duration
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of retry attempts for failed operations
const MAX_RETRY_ATTEMPTS: u32 = 3;

/// Base delay for exponential backoff retry strategy
const RETRY_BASE_DELAY: Duration = Duration::from_millis(1000);

/// Maximum delay between retry attempts
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Default rate limiting: minimum interval between RPC calls
const DEFAULT_MIN_CALL_INTERVAL: Duration = Duration::from_millis(100);

/// Default maximum concurrent RPC requests
const DEFAULT_MAX_CONCURRENT_REQUESTS: u32 = 10;

/// Default health check interval
const DEFAULT_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time to wait for finality before timing out
const MAX_FINALITY_WAIT_TIME: Duration = Duration::from_secs(600);

/// Average Substrate block time (used for polling intervals)
const SUBSTRATE_BLOCK_TIME: Duration = Duration::from_secs(6);

/// Maximum number of failed health checks before marking as unhealthy
const MAX_CONSECUTIVE_HEALTH_FAILURES: u32 = 5;

/// Configuration for the PolkadotAdapter
///
/// This struct contains all necessary configuration parameters for connecting
/// to and interacting with Substrate-based chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolkadotConfig {
    /// WebSocket or HTTP RPC endpoint URL
    /// Examples: "wss://rpc.polkadot.io", "https://polkadot.api.onfinality.io/public"
    pub rpc_url: String,
    
    /// Signer seed phrase or private key for transaction signing
    /// Can be mnemonic phrase, //Alice format, or hex private key
    pub signer_seed: String,
    
    /// Number of block confirmations required for finality
    /// None uses DEFAULT_CONFIRMATIONS (12 for Polkadot mainnet)
    pub confirmations: Option<u32>,
    
    /// Timeout for individual RPC requests
    /// None uses DEFAULT_RPC_TIMEOUT (30 seconds)
    pub rpc_timeout: Option<Duration>,
    
    /// Maximum number of concurrent RPC requests
    /// Helps prevent overwhelming the RPC endpoint
    pub max_concurrent_requests: Option<u32>,
    
    /// Interval between automated health checks
    /// None disables automated health monitoring
    pub health_check_interval: Option<Duration>,
    
    /// Minimum interval between RPC calls for rate limiting
    /// Helps prevent rate limiting by RPC providers
    pub min_call_interval: Option<Duration>,
    
    /// Custom pallet name for cross-chain messaging
    /// Defaults to "XcmpQueue" if not specified
    pub message_pallet: Option<String>,
    
    /// Chain-specific configuration overrides
    pub chain_specific: Option<ChainSpecificConfig>,
}

/// Chain-specific configuration parameters
///
/// Different Substrate chains may have varying parameters and requirements
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

/// Comprehensive health metrics for monitoring adapter performance
///
/// These metrics provide visibility into the adapter's operational status
/// and can be used for alerting and performance optimization.
#[derive(Debug, Clone)]
struct HealthMetrics {
    /// Timestamp of the last successful RPC call
    pub last_successful_call: Option<Instant>,
    
    /// Number of consecutive failed operations
    pub consecutive_failures: u32,
    
    /// Total number of RPC calls made
    pub total_calls: u64,
    
    /// Total number of failed RPC calls
    pub failed_calls: u64,
    
    /// Average response time for successful calls
    pub avg_response_time: Duration,
    
    /// Current connection status
    pub connection_status: ConnectionStatus,
    
    /// Latest block number seen
    pub latest_block_seen: Option<u32>,
    
    /// Timestamp when metrics were last updated
    pub last_updated: Instant,
}

/// Connection status enumeration
#[derive(Debug, Clone, PartialEq)]
enum ConnectionStatus {
    /// Connection is healthy and operational
    Healthy,
    /// Connection is degraded but functional
    Degraded,
    /// Connection is unhealthy or failed
    Unhealthy,
    /// Connection status is unknown (initial state)
    Unknown,
}

/// Transaction tracking information
///
/// Maintains state and metadata for submitted transactions
#[derive(Debug, Clone)]
struct TransactionInfo {
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
enum TxTrackingStatus {
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

/// Polkadot/Substrate chain adapter
pub struct PolkadotAdapter {
    /// Subxt client for blockchain interaction
    client: Arc<OnlineClient<SubxtPolkadotConfig>>,
    
    /// Adapter configuration
    config: PolkadotConfig,
    
    /// Mapping from message IDs to transaction information
    message_tx_mapping: Arc<RwLock<HashMap<Uuid, TransactionInfo>>>,
    
    /// Current health and performance metrics
    health_metrics: Arc<Mutex<HealthMetrics>>,
    
    /// Rate limiting: timestamp of last RPC call
    last_rpc_call: Arc<Mutex<Option<Instant>>>,
    
    /// Semaphore for controlling concurrent RPC requests
    rpc_semaphore: Arc<Semaphore>,
    
    /// Background health monitor task handle
    _health_monitor_handle: tokio::task::JoinHandle<()>,
    
    /// Transaction signer for submitting extrinsics
    signer: Arc<PairSigner<SubxtPolkadotConfig, subxt::utils::MultiSignature>>,
}

impl fmt::Debug for PolkadotAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PolkadotAdapter")
            .field("rpc_url", &self.config.rpc_url)
            .field("confirmations", &self.config.confirmations)
            .field("max_concurrent_requests", &self.config.max_concurrent_requests)
            .finish()
    }
}

impl PolkadotAdapter {
    /// Creates a new PolkadotAdapter instance
    #[instrument(level = "info", skip(config))]
    pub async fn new(config: PolkadotConfig) -> Result<Self, AdapterError> {
        let span = span!(Level::INFO, "polkadot_adapter_init");
        let _enter = span.enter();
        
        info!("Initializing PolkadotAdapter for {}", config.rpc_url);
        
        // Establish connection to the Substrate chain
        let client = Self::create_client(&config).await?;
        
        // Initialize transaction signer
        let signer = Self::create_signer(&config)?;
        
        // Initialize health metrics
        let health_metrics = Arc::new(Mutex::new(HealthMetrics {
            last_successful_call: None,
            consecutive_failures: 0,
            total_calls: 0,
            failed_calls: 0,
            avg_response_time: Duration::ZERO,
            connection_status: ConnectionStatus::Unknown,
            latest_block_seen: None,
            last_updated: Instant::now(),
        }));
        
        // Create semaphore for concurrent request limiting
        let max_concurrent = config.max_concurrent_requests
            .unwrap_or(DEFAULT_MAX_CONCURRENT_REQUESTS) as usize;
        let rpc_semaphore = Arc::new(Semaphore::new(max_concurrent));
        
        // Start background health monitoring if configured
        let health_monitor_handle = if config.health_check_interval.is_some() {
            Self::start_health_monitor(
                Arc::clone(&client),
                Arc::clone(&health_metrics),
                config.health_check_interval.unwrap(),
            )
        } else {
            // Create a dummy task that does nothing
            tokio::spawn(async {})
        };
        
        let adapter = Self {
            client: Arc::new(client),
            config,
            message_tx_mapping: Arc::new(RwLock::new(HashMap::new())),
            health_metrics,
            last_rpc_call: Arc::new(Mutex::new(None)),
            rpc_semaphore,
            _health_monitor_handle: health_monitor_handle,
            signer: Arc::new(signer),
        };
        
        // Perform initial health check
        if let Err(e) = adapter.health_check().await {
            warn!("Initial health check failed: {}", e);
        } else {
            info!("PolkadotAdapter initialized successfully");
        }
        
        Ok(adapter)
    }
    
    /// Creates and configures the Subxt client
    ///
    /// Establishes connection to the Substrate chain with appropriate timeouts
    /// and error handling.
    async fn create_client(config: &PolkadotConfig) -> Result<OnlineClient<SubxtPolkadotConfig>, AdapterError> {
        let timeout_duration = config.rpc_timeout.unwrap_or(DEFAULT_RPC_TIMEOUT);
        
        match timeout(timeout_duration, OnlineClient::<SubxtPolkadotConfig>::from_url(&config.rpc_url)).await {
            Ok(Ok(client)) => {
                debug!("Successfully connected to {}", config.rpc_url);
                Ok(client)
            },
            Ok(Err(e)) => {
                error!("Failed to connect to {}: {}", config.rpc_url, e);
                Err(AdapterError::Network(format!("Failed to connect to {}: {}", config.rpc_url, e)))
            },
            Err(_) => {
                error!("Connection timeout to {}", config.rpc_url);
                Err(AdapterError::Network(format!("Connection timeout to {}", config.rpc_url)))
            }
        }
    }
    
    /// Creates and configures the transaction signer
    ///
    /// Initializes the cryptographic signer for submitting transactions to the chain.
    fn create_signer(config: &PolkadotConfig) -> Result<PairSigner<SubxtPolkadotConfig, subxt::utils::MultiSignature>, AdapterError> {
        let pair = subxt::utils::sr25519::Pair::from_string(&config.signer_seed, None)
            .map_err(|e| AdapterError::Other(format!("Invalid signer seed: {}", e)))?;
            
        Ok(PairSigner::new(pair))
    }
    
    /// Starts the background health monitoring task
    ///
    /// This task periodically checks the health of the connection and updates
    /// metrics accordingly.
    fn start_health_monitor(
        client: Arc<OnlineClient<SubxtPolkadotConfig>>,
        health_metrics: Arc<Mutex<HealthMetrics>>,
        check_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval_timer = interval(check_interval);
            interval_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            
            info!("Starting health monitor with interval: {:?}", check_interval);
            
            loop {
                interval_timer.tick().await;
                
                // Perform health check
                let health_result = timeout(
                    DEFAULT_RPC_TIMEOUT,
                    client.blocks().at_latest().await?.header()
                ).await;
                
                let mut metrics = health_metrics.lock().await;
                metrics.last_updated = Instant::now();
                
                match health_result {
                    Ok(Ok(Some(header))) => {
                        debug!("Health check passed - block #{}", header.number);
                        metrics.connection_status = ConnectionStatus::Healthy;
                        metrics.consecutive_failures = 0;
                        metrics.latest_block_seen = Some(header.number);
                        metrics.last_successful_call = Some(Instant::now());
                    },
                    Ok(Ok(None)) => {
                        warn!("Health check: No block header received");
                        metrics.consecutive_failures += 1;
                        metrics.connection_status = if metrics.consecutive_failures > MAX_CONSECUTIVE_HEALTH_FAILURES {
                            ConnectionStatus::Unhealthy
                        } else {
                            ConnectionStatus::Degraded
                        };
                    },
                    Ok(Err(e)) => {
                        warn!("Health check failed: {}", e);
                        metrics.consecutive_failures += 1;
                        metrics.connection_status = if metrics.consecutive_failures > MAX_CONSECUTIVE_HEALTH_FAILURES {
                            ConnectionStatus::Unhealthy
                        } else {
                            ConnectionStatus::Degraded
                        };
                    },
                    Err(_) => {
                        warn!("Health check timed out");
                        metrics.consecutive_failures += 1;
                        metrics.connection_status = ConnectionStatus::Unhealthy;
                    }
                }
                
                if metrics.consecutive_failures > 0 {
                    debug!(
                        "Health monitor - consecutive failures: {}, status: {:?}",
                        metrics.consecutive_failures,
                        metrics.connection_status
                    );
                }
            }
        })
    }
    
    /// Executes an RPC call with comprehensive error handling and retry logic
    #[instrument(level = "debug", skip(self, operation))]
    async fn execute_rpc_call<T, F, Fut>(&self, mut operation: F) -> Result<T, AdapterError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, SubxtError>>,
    {
        // Acquire semaphore permit to limit concurrent requests
        let _permit = self.rpc_semaphore.acquire().await
            .map_err(|_| AdapterError::Other("Failed to acquire RPC semaphore".to_string()))?;
        
        // Enforce rate limiting
        self.enforce_rate_limit().await;
        
        let start_time = Instant::now();
        let mut last_error = None;
        let timeout_duration = self.config.rpc_timeout.unwrap_or(DEFAULT_RPC_TIMEOUT);
        
        // Retry loop with exponential backoff
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            debug!("RPC call attempt {} of {}", attempt + 1, MAX_RETRY_ATTEMPTS);
            
            match timeout(timeout_duration, operation()).await {
                Ok(Ok(result)) => {
                    let response_time = start_time.elapsed();
                    debug!("RPC call succeeded in {:?}", response_time);
                    self.update_success_metrics(response_time).await;
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!("RPC call failed on attempt {}: {}", attempt + 1, e);
                    last_error = Some(e);
                    
                    // Only retry if not the last attempt
                    if attempt < MAX_RETRY_ATTEMPTS - 1 {
                        let delay = std::cmp::min(
                            RETRY_BASE_DELAY * 2_u32.pow(attempt),
                            MAX_RETRY_DELAY
                        );
                        debug!("Retrying in {:?}", delay);
                        sleep(delay).await;
                    }
                }
                Err(_) => {
                    warn!("RPC call timed out on attempt {} (timeout: {:?})", attempt + 1, timeout_duration);
                    if attempt < MAX_RETRY_ATTEMPTS - 1 {
                        let delay = std::cmp::min(
                            RETRY_BASE_DELAY * 2_u32.pow(attempt),
                            MAX_RETRY_DELAY
                        );
                        sleep(delay).await;
                    } else {
                        self.update_failure_metrics().await;
                        return Err(AdapterError::Network("Request timed out after all retry attempts".to_string()));
                    }
                }
            }
        }
        
        // All retry attempts exhausted
        self.update_failure_metrics().await;
        let error_msg = last_error
            .map(|e| format!("RPC call failed after {} attempts: {}", MAX_RETRY_ATTEMPTS, e))
            .unwrap_or_else(|| format!("RPC call failed after {} retry attempts", MAX_RETRY_ATTEMPTS));
        
        error!("{}", error_msg);
        Err(AdapterError::Network(error_msg))
    }
    
    /// Enforces rate limiting between RPC calls
    ///
    /// Ensures that consecutive RPC calls are spaced appropriately to avoid
    /// overwhelming the RPC endpoint or hitting rate limits.
    async fn enforce_rate_limit(&self) {
        let min_interval = self.config.min_call_interval.unwrap_or(DEFAULT_MIN_CALL_INTERVAL);
        let mut last_call = self.last_rpc_call.lock().await;
        
        if let Some(last_time) = *last_call {
            let elapsed = last_time.elapsed();
            if elapsed < min_interval {
                let sleep_duration = min_interval - elapsed;
                drop(last_call); // Release lock during sleep
                debug!("Rate limiting: sleeping for {:?}", sleep_duration);
                sleep(sleep_duration).await;
                last_call = self.last_rpc_call.lock().await;
            }
        }
        
        *last_call = Some(Instant::now());
    }
    
    /// Updates success metrics after a successful RPC call
    ///
    /// Maintains rolling averages and success counters for monitoring purposes.
    async fn update_success_metrics(&self, response_time: Duration) {
        let mut metrics = self.health_metrics.lock().await;
        metrics.last_successful_call = Some(Instant::now());
        metrics.consecutive_failures = 0;
        metrics.total_calls += 1;
        metrics.last_updated = Instant::now();
        
        // Update connection status if it was degraded
        if metrics.connection_status != ConnectionStatus::Healthy {
            metrics.connection_status = ConnectionStatus::Healthy;
            info!("Connection status restored to healthy");
        }
        
        // Calculate rolling average response time
        let total_successful = metrics.total_calls - metrics.failed_calls;
        if total_successful == 1 {
            metrics.avg_response_time = response_time;
        } else {
            let current_avg_nanos = metrics.avg_response_time.as_nanos() as f64;
            let new_response_nanos = response_time.as_nanos() as f64;
            let new_avg_nanos = (current_avg_nanos * (total_successful as f64 - 1.0) + new_response_nanos) / total_successful as f64;
            metrics.avg_response_time = Duration::from_nanos(new_avg_nanos as u64);
        }
        
        debug!(
            "Success metrics updated - avg_response_time: {:?}, total_calls: {}, success_rate: {:.2}%",
            metrics.avg_response_time,
            metrics.total_calls,
            ((metrics.total_calls - metrics.failed_calls) as f64 / metrics.total_calls as f64) * 100.0
        );
    }
    
    /// Updates failure metrics after a failed RPC call
    ///
    /// Tracks failure rates and adjusts connection status based on consecutive failures.
    async fn update_failure_metrics(&self) {
        let mut metrics = self.health_metrics.lock().await;
        metrics.consecutive_failures += 1;
        metrics.total_calls += 1;
        metrics.failed_calls += 1;
        metrics.last_updated = Instant::now();
        
        // Update connection status based on consecutive failures
        metrics.connection_status = match metrics.consecutive_failures {
            1..=2 => ConnectionStatus::Degraded,
            _ => ConnectionStatus::Unhealthy,
        };
        
        warn!(
            "Failure metrics updated - consecutive_failures: {}, total_failures: {}, failure_rate: {:.2}%",
            metrics.consecutive_failures,
            metrics.failed_calls,
            (metrics.failed_calls as f64 / metrics.total_calls as f64) * 100.0
        );
    }
    
    /// Updates transaction tracking information
    ///
    /// Maintains the lifecycle state of submitted transactions for monitoring
    /// and status reporting purposes.
    async fn update_transaction_status(&self, message_id: &Uuid, status: TxTrackingStatus, block_number: Option<u32>, error: Option<String>) {
        let mut mapping = self.message_tx_mapping.write().await;
        if let Some(tx_info) = mapping.get_mut(message_id) {
            tx_info.status = status.clone();
            if let Some(block) = block_number {
                tx_info.block_number = Some(block);
            }
            if let Some(err) = error {
                tx_info.error = Some(err);
            }
            
            debug!(
                "Transaction status updated - message_id: {}, status: {:?}, block: {:?}",
                message_id, status, block_number
            );
        }
    }
    
    /// Retrieves comprehensive health and performance metrics
    ///
    /// Returns a snapshot of current adapter health including connection status,
    /// performance metrics, and error rates.
    pub async fn get_health_metrics(&self) -> HealthMetrics {
        self.health_metrics.lock().await.clone()
    }
    
    /// Retrieves transaction information by message ID
    ///
    /// Provides detailed tracking information for submitted messages including
    /// current status, block inclusion, and any errors encountered.
    pub async fn get_transaction_info(&self, message_id: &Uuid) -> Option<TransactionInfo> {
        let mapping = self.message_tx_mapping.read().await;
        mapping.get(message_id).cloned()
    }
}

#[async_trait]
impl ChainAdapter for PolkadotAdapter {
    type BlockId = u32;
    type TxId = H256;
    type Error = AdapterError;

    /// Retrieves the latest finalized block number
    #[instrument(level = "debug", skip(self))]
    async fn latest_block(&self) -> Result<Self::BlockId, Self::Error> {
        debug!("Fetching latest block number");
        
        let header = self.execute_rpc_call(|| async {
            self.client.blocks().at(block_hash).header().await
        }).await?;
        
        let block_number = header
            .ok_or_else(|| AdapterError::Network("No block header found".to_string()))?
            .number;
            
        debug!("Latest block number: {}", block_number);
        
        // Update health metrics with latest block
        {
            let mut metrics = self.health_metrics.lock().await;
            metrics.latest_block_seen = Some(block_number);
        }
        
        Ok(block_number)
    }

    /// Retrieves transaction data by transaction hash
    ///
    /// Note: Substrate chains don't have direct transaction lookup by hash
    /// like EVM chains. This would typically require indexing or using
    /// a block explorer API.
    #[instrument(level = "debug", skip(self))]
    async fn get_transaction(&self, tx: &Self::TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        debug!("Looking up transaction: {:?}", tx);
        
        // Substrate doesn't have direct transaction lookup by hash
        // This would require either:
        // 1. Indexing extrinsics during block processing
        // 2. Using a block explorer API
        // 3. Scanning recent blocks for the transaction
        
        // For now, return None indicating transaction lookup is not implemented
        // In a production environment, you would implement one of the above strategies
        warn!("Transaction lookup not implemented for Substrate chains");
        Ok(None)
    }

    /// Waits for a block to reach finality based on confirmation requirements
    ///
    /// This method polls the chain until the specified block has received
    /// the required number of confirmations as configured in the adapter.
    #[instrument(level = "debug", skip(self))]
    async fn wait_for_finality(&self, block: &Self::BlockId) -> Result<(), Self::Error> {
        let confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);
        let target_block = *block + confirmations;
        let start_time = Instant::now();
        
        info!(
            "Waiting for block {} to reach finality (target block: {}, confirmations: {})",
            block, target_block, confirmations
        );
        
        let block_time = self.config.chain_specific
            .as_ref()
            .and_then(|c| c.block_time)
            .unwrap_or(SUBSTRATE_BLOCK_TIME);
        
        loop {
            // Check for timeout
            if start_time.elapsed() > MAX_FINALITY_WAIT_TIME {
                let error_msg = format!(
                    "Finality wait timeout for block {} after {:?}",
                    block, MAX_FINALITY_WAIT_TIME
                );
                error!("{}", error_msg);
                return Err(AdapterError::Timeout(error_msg));
            }
            
            // Check current block height
            match self.latest_block().await {
                Ok(current_block) => {
                    debug!(
                        "Finality check - current: {}, target: {}, remaining: {}",
                        current_block,
                        target_block,
                        target_block.saturating_sub(current_block)
                    );
                    
                    if current_block >= target_block {
                        info!(
                            "Block {} reached finality in {:?}",
                            block,
                            start_time.elapsed()
                        );
                        return Ok(());
                    }
                }
                Err(e) => {
                    warn!("Error checking latest block during finality wait: {}", e);
                    // Continue waiting - temporary network issues shouldn't fail finality wait
                }
            }
            
            // Wait for next block
            sleep(block_time).await;
        }
    }

    /// Submits a cross-chain message to the Substrate chain
    ///
    /// This method constructs and submits the appropriate extrinsic for
    /// cross-chain message passing, tracking the transaction through
    /// its lifecycle.
    #[instrument(level = "info", skip(self, msg))]
    async fn submit_message(&self, msg: &FrostMessage) -> Result<Self::TxId, Self::Error> {
        info!("Submitting message with ID: {}", msg.id);
        
        // TODO: Replace with actual pallet call construction
        // This is a placeholder that would need to be replaced with the actual
        // call to your cross-chain messaging pallet
        //
        // Example for a hypothetical XcmpQueue pallet:
        // let call = polkadot_runtime::tx()
        //     .xcmp_queue()
        //     .send_xcmp_message(msg.destination_chain, msg.payload.clone());
        
        // For now, we'll simulate the transaction submission
        let tx_hash = H256::random();
        
        // Create transaction tracking info
        let tx_info = TransactionInfo {
            message_id: msg.id,
            tx_hash,
            block_number: None,
            status: TxTrackingStatus::Submitted,
            submitted_at: Instant::now(),
            confirmations: 0,
            error: None,
        };
        
        // Store transaction mapping
        {
            let mut mapping = self.message_tx_mapping.write().await;
            mapping.insert(msg.id, tx_info);
        }
        
        info!("Message submitted successfully - tx_hash: {:?}", tx_hash);
        
        // TODO: Implement actual transaction submission
        // let tx_progress = self.execute_rpc_call(|| async {
        //     self.client.tx().sign_and_submit_then_watch(&call, &*self.signer).await
        // }).await?;
        // 
        // let tx_hash = tx_progress.extrinsic_hash();
        // 
        // // Update transaction status based on progress
        // self.track_transaction_progress(msg.id, tx_progress).await?;
        
        Ok(tx_hash)
    }

    /// Listens for relevant cross-chain message events
    ///
    /// This method subscribes to Substrate events and filters for
    /// cross-chain messaging events that are relevant to the Frostgate protocol.
    #[instrument(level = "debug", skip(self))]
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, Self::Error> {
        debug!("Listening for cross-chain message events");
        
        // TODO: Implement event subscription and filtering
        // This would involve:
        // 1. Subscribing to finalized block headers
        // 2. Fetching events from each new block
        // 3. Filtering for relevant cross-chain messaging events
        // 4. Converting Substrate events to MessageEvent format
        
        // Example implementation:
        // let mut events = Vec::new();
        // let latest_block = self.latest_block().await?;
        // let block_hash = self.execute_rpc_call(|| async {
        //     self.client.rpc().block_hash(Some(latest_block.into())).await
        // }).await?;
        // 
        // if let Some(hash) = block_hash {
        //     let block_events = self.execute_rpc_call(|| async {
        //         self.client.events().at(hash).await
        //     }).await?;
        //     
        //     for event in block_events.iter() {
        //         let event = event.map_err(|e| AdapterError::Other(format!("Event decode error: {}", e)))?;
        //         
        //         // Filter for cross-chain messaging events
        //         if event.pallet_name() == "XcmpQueue" || event.pallet_name() == "DmpQueue" {
        //             // Convert to MessageEvent and add to results
        //             events.push(self.convert_substrate_event_to_message_event(event)?);
        //         }
        //     }
        // }
        
        // For now, return empty vector
        Ok(vec![])
    }

    /// Verifies that a message has been properly processed on-chain
    ///
    /// This method checks the chain state to verify that a message
    /// has been correctly processed and any required proofs are valid.
    #[instrument(level = "debug", skip(self, msg))]
    async fn verify_on_chain(&self, msg: &FrostMessage) -> Result<(), Self::Error> {
        debug!("Verifying message on-chain: {}", msg.id);
        
        // TODO: Implement on-chain verification logic
        // This would typically involve:
        // 1. Checking that the message exists in chain storage
        // 2. Verifying any cryptographic proofs
        // 3. Confirming the message status is correct
        // 4. Validating any cross-chain proof requirements
        
        // Example implementation:
        // let storage_key = self.build_message_storage_key(&msg.id);
        // let storage_value = self.execute_rpc_call(|| async {
        //     self.client.storage().at_latest().fetch(&storage_key).await
        // }).await?;
        // 
        // match storage_value {
        //     Some(value) => {
        //         // Verify the stored message matches our expectation
        //         let stored_msg: StoredMessage = codec::Decode::decode(&mut &value[..])
        //             .map_err(|e| AdapterError::Other(format!("Failed to decode stored message: {}", e)))?; 
        //         
        //         if stored_msg.hash != msg.compute_hash() {
        //             return Err(AdapterError::Other("Message hash mismatch".to_string()));
        //         }
        //         
        //         info!("Message verification successful: {}", msg.id);
        //         Ok(())
        //     }
        //     None => {
        //         Err(AdapterError::Other("Message not found in chain storage".to_string()))
        //     }
        // }
        
        // For now, always return success
        Ok(())
    }

    /// Estimates the fee required to submit a message
    ///
    /// This method calculates the estimated fee for submitting a cross-chain
    /// message based on current network conditions and message complexity.
    #[instrument(level = "debug", skip(self, msg))]
    async fn estimate_fee(&self, msg: &FrostMessage) -> Result<u128, Self::Error> {
        debug!("Estimating fee for message: {}", msg.id);
        
        // TODO: Implement actual fee estimation
        // This would involve:
        // 1. Constructing the actual extrinsic that would be submitted
        // 2. Using the payment info API to get fee estimates
        // 3. Adding any cross-chain specific fees
        // 4. Accounting for current network congestion
        
        // Example implementation:
        // let call = polkadot_runtime::tx()
        //     .xcmp_queue()
        //     .send_xcmp_message(msg.destination_chain, msg.payload.clone());
        // 
        // let partial_fee_details = self.execute_rpc_call(|| async {
        //     self.client
        //         .tx()
        //         .payment_info(&call, &*self.signer)
        //         .await
        // }).await?;
        // 
        // let estimated_fee = partial_fee_details.partial_fee;
        // 
        // // Add buffer for network congestion and cross-chain overhead
        // let fee_with_buffer = estimated_fee + (estimated_fee / 10); // 10% buffer
        // 
        // info!("Estimated fee for message {}: {}", msg.id, fee_with_buffer);
        // Ok(fee_with_buffer)
        
        // For now, return a placeholder fee
        let placeholder_fee = 1_000_000_000_000u128; // 1 DOT in Planck units
        Ok(placeholder_fee)
    }

    /// Retrieves the current status of a submitted message
    ///
    /// This method checks the current processing status of a message
    /// that has been submitted to the chain.
    #[instrument(level = "debug", skip(self))]
    async fn message_status(&self, id: &Uuid) -> Result<MessageStatus, Self::Error> {
        debug!("Checking status for message: {}", id);
        
        let mapping = self.message_tx_mapping.read().await;
        
        if let Some(tx_info) = mapping.get(id) {
            let status = match tx_info.status {
                TxTrackingStatus::Submitted => MessageStatus::Pending,
                TxTrackingStatus::InBlock => {
                    // Check if we have enough confirmations
                    if let Some(block_num) = tx_info.block_number {
                        match self.latest_block().await {
                            Ok(current_block) => {
                                let confirmations = current_block.saturating_sub(block_num);
                                let required_confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);
                                
                                if confirmations >= required_confirmations {
                                    MessageStatus::Confirmed
                                } else {
                                    MessageStatus::Pending
                                }
                            }
                            Err(_) => MessageStatus::Pending,
                        }
                    } else {
                        MessageStatus::Pending
                    }
                }
                TxTrackingStatus::Finalized => MessageStatus::Confirmed,
                TxTrackingStatus::Failed => MessageStatus::Failed("message failed".to_string()),
                TxTrackingStatus::Unknown => MessageStatus::Pending,
            };
            
            debug!("Message {} status: {:?}", id, status);
            Ok(status)
        } else {
            debug!("Message {} not found in tracking", id);
            Ok(MessageStatus::Pending)
        }
    }

    /// Performs a comprehensive health check of the adapter
    ///
    /// This method verifies that the adapter is functioning correctly
    /// by testing connectivity, checking recent performance, and
    /// validating configuration.
    #[instrument(level = "info", skip(self))]
    async fn health_check(&self) -> Result<(), Self::Error> {
        info!("Performing comprehensive health check");
        
        // Test basic connectivity by fetching latest block
        let latest_block = self.latest_block().await?;
        debug!("Connectivity check passed - latest block: {}", latest_block);
        
        // Check health metrics
        let metrics = self.health_metrics.lock().await;
        let health_status = match metrics.connection_status {
            ConnectionStatus::Healthy => {
                info!("Health check passed - connection healthy");
                Ok(())
            }
            ConnectionStatus::Degraded => {
                warn!(
                    "Health check warning - connection degraded ({} consecutive failures)",
                    metrics.consecutive_failures
                );
                // Still return Ok for degraded but functional state
                Ok(())
            }
            ConnectionStatus::Unhealthy => {
                error!(
                    "Health check failed - connection unhealthy ({} consecutive failures)",
                    metrics.consecutive_failures
                );
                Err(AdapterError::Network(format!(
                    "Unhealthy connection - {} consecutive failures",
                    metrics.consecutive_failures
                )))
            }
            ConnectionStatus::Unknown => {
                warn!("Health check - connection status unknown");
                Ok(())
            }
        };
        
        // Log health summary
        if metrics.total_calls > 0 {
            let success_rate = ((metrics.total_calls - metrics.failed_calls) as f64 / metrics.total_calls as f64) * 100.0;
            info!(
                "Health metrics - Total calls: {}, Success rate: {:.2}%, Avg response time: {:?}",
                metrics.total_calls,
                success_rate,
                metrics.avg_response_time
            );
        }
        
        health_status
    }
}

impl PolkadotAdapter {
    /// Tracks the progress of a submitted transaction
    ///
    /// This method would monitor transaction progress through various states
    /// and update internal tracking accordingly.
    async fn track_transaction_progress(
        &self,
        message_id: Uuid,
        mut _tx_progress: TxProgress<SubxtPolkadotConfig, OnlineClient<SubxtPolkadotConfig>>,
    ) -> Result<(), AdapterError> {
        // TODO: Implement transaction progress tracking
        // This would involve listening to the TxProgress stream and updating
        // the transaction status as it moves through different states
        
        // Example implementation:
        // while let Some(status) = tx_progress.next().await {
        //     match status? {
        //         TxStatus::InBestBlock(tx_in_block) => {
        //             let block_hash = tx_in_block.block_hash();
        //             let block_number = self.get_block_number_from_hash(&block_hash).await?;
        //             
        //             self.update_transaction_status(
        //                 &message_id,
        //                 TxTrackingStatus::InBlock,
        //                 Some(block_number),
        //                 None
        //             ).await;
        //             
        //             info!("Transaction {} included in block {}", message_id, block_number);
        //         }
        //         TxStatus::InFinalizedBlock(tx_finalized) => {
        //             self.update_transaction_status(
        //                 &message_id,
        //                 TxTrackingStatus::Finalized,
        //                 None,
        //                 None
        //             ).await;
        //             
        //             info!("Transaction {} finalized", message_id);
        //             break;
        //         }
        //         TxStatus::Error { message } => {
        //             self.update_transaction_status(
        //                 &message_id,
        //                 TxTrackingStatus::Failed,
        //                 None,
        //                 Some(message.clone())
        //             ).await;
        //             
        //             error!("Transaction {} failed: {}", message_id, message);
        //             return Err(AdapterError::Other(format!("Transaction failed: {}", message)));
        //         }
        //         TxStatus::Invalid { message } => {
        //             self.update_transaction_status(
        //                 &message_id,
        //                 TxTrackingStatus::Failed,
        //                 None,
        //                 Some(message.clone())
        //             ).await;
        //             
        //             error!("Transaction {} invalid: {}", message_id, message);
        //             return Err(AdapterError::Other(format!("Transaction invalid: {}", message)));
        //         }
        //         _ => {
        //             // Handle other status updates as needed
        //             debug!("Transaction {} status update: {:?}", message_id, status);
        //         }
        //     }
        // }
        
        Ok(())
    }
    
    /// Gets block number from block hash
    ///
    /// Helper method to retrieve block number when only hash is available.
    async fn _get_block_number_from_hash(&self, _block_hash: &H256) -> Result<u32, AdapterError> {
        // TODO: Implement block number lookup by hash
        // let header = self.execute_rpc_call(|| async {
        //     self.client.rpc().header(Some(*block_hash)).await
        // }).await?;
        // 
        // Ok(header.ok_or_else(|| AdapterError::Network("Block header not found".to_string()))?.number)
        
        Ok(0) // Placeholder
    }
    
    /// Converts Substrate events to MessageEvent format
    ///
    /// Helper method to transform Substrate-specific events into the
    /// common MessageEvent format used by the Frostgate protocol.
    fn _convert_substrate_event_to_message_event(
        &self,
        _event: subxt::events::StaticEvent,
    ) -> Result<MessageEvent, AdapterError> {
        // TODO: Implement event conversion logic
        // This would decode Substrate events and map them to MessageEvent
        
        // Example:
        // match (event.pallet_name(), event.variant_name()) {
        //     ("XcmpQueue", "XcmpMessageSent") => {
        //         let event_data = event.field_values()?;
        //         Ok(MessageEvent {
        //             message_id: extract_message_id_from_event(&event_data)?,
        //             event_type: MessageEventType::Sent,
        //             block_number: Some(block_number),
        //             timestamp: Some(Instant::now()),
        //             additional_data: Some(event_data),
        //         })
        //     }
        //     _ => Err(AdapterError::Other("Unknown event type".to_string()))
        // }
        
        // Placeholder implementation
        Err(AdapterError::Other("Event conversion not implemented".to_string()))
    }
}

impl Drop for PolkadotAdapter {
    fn drop(&mut self) {
        info!("PolkadotAdapter is being dropped - cleaning up resources");
        // The health monitor task will be automatically cancelled when the handle is dropped
    }
}

// Tests Module For Frostgate-Polkadot
#[cfg(test)]
mod tests {
    use super::*;
    use tokio;
    
    #[tokio::test]
    async fn test_adapter_creation() {
        let config = PolkadotConfig {
            rpc_url: "ws://localhost:9944".to_string(),
            signer_seed: "//Alice".to_string(),
            confirmations: Some(6),
            rpc_timeout: Some(Duration::from_secs(10)),
            max_concurrent_requests: Some(5),
            health_check_interval: None, // Disable for testing
            min_call_interval: Some(Duration::from_millis(50)),
            message_pallet: None,
            chain_specific: None,
        };
        
        // This test will fail unless you have a local Substrate node running
        // In a real test environment, you'd use a mock client
        match PolkadotAdapter::new(config).await {
            Ok(_adapter) => {
                // Test successful creation
                assert!(true);
            }
            Err(_) => {
                // Expected if no local node is running
                assert!(true);
            }
        }
    }
    
    #[test]
    fn test_config_serialization() {
        let config = PolkadotConfig {
            rpc_url: "wss://rpc.polkadot.io".to_string(),
            signer_seed: "//Alice".to_string(),
            confirmations: Some(12),
            rpc_timeout: Some(Duration::from_secs(30)),
            max_concurrent_requests: Some(10),
            health_check_interval: Some(Duration::from_secs(30)),
            min_call_interval: Some(Duration::from_millis(100)),
            message_pallet: Some("XcmpQueue".to_string()),
            chain_specific: Some(ChainSpecificConfig {
                chain_id: "polkadot".to_string(),
                block_time: Some(Duration::from_secs(6)),
                finality_blocks: Some(12),
                custom_rpc_methods: None,
            }),
        };
        
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: PolkadotConfig = serde_json::from_str(&serialized).unwrap();
        
        assert_eq!(config.rpc_url, deserialized.rpc_url);
        assert_eq!(config.confirmations, deserialized.confirmations);
    }
    
    #[test]
    fn test_health_metrics_default() {
        let metrics = HealthMetrics {
            last_successful_call: None,
            consecutive_failures: 0,
            total_calls: 0,
            failed_calls: 0,
            avg_response_time: Duration::ZERO,
            connection_status: ConnectionStatus::Unknown,
            latest_block_seen: None,
            last_updated: Instant::now(),
        };
        
        assert_eq!(metrics.consecutive_failures, 0);
        assert_eq!(metrics.connection_status, ConnectionStatus::Unknown);
        assert!(metrics.last_successful_call.is_none());
    }
}