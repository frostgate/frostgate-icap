// SolanaAdapter: Frostgate ChainAdapter for Solana Chains

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_mut)]


use crate::chainadapter::{
    ChainAdapter, AdapterError,
};
use frostgate_sdk::frostmessage::{
    FrostMessage,
    MessageEvent,
    MessageStatus,
};
use solana_client::{
    rpc_client::RpcClient,
    rpc_config::{RpcTransactionConfig, RpcSendTransactionConfig},
    rpc_request::RpcRequest,
    client_error::{ClientError, ClientErrorKind},
};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Signature,
    commitment_config::{CommitmentConfig, CommitmentLevel},
    transaction::Transaction,
};
use solana_transaction_status::{
    UiTransactionEncoding, 
    TransactionStatus,
    EncodedTransaction,
};
use uuid::Uuid;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    thread,
};
use tokio::sync::RwLock; 
use tokio::time::sleep;
use log::{debug, info, warn, error, trace};
use thiserror::Error;

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

impl SolanaConfig {
    /// Create a new configuration builder
    pub fn builder() -> SolanaConfigBuilder {
        SolanaConfigBuilder::default()
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

/// Specialized error types for Solana adapter operations
#[derive(Error, Debug)]
pub enum SolanaAdapterError {
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("RPC client error: {0}")]
    RpcClient(#[from] ClientError),
    
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
struct TrackedMessage {
    /// Unique identifier for the message
    id: Uuid,
    
    /// Transaction signature on Solana
    signature: Option<String>,
    
    /// Current status of the message
    status: MessageStatus,
    
    /// Timestamp when message was submitted
    submitted_at: SystemTime,
    
    /// Block slot where transaction was included (if any)
    block_slot: Option<u64>,
}

/// Production-grade Solana adapter implementation
pub struct SolanaAdapter {
    /// RPC client for Solana blockchain interaction
    client: RpcClient,
    
    /// Configuration parameters
    config: SolanaConfig,
    
    /// In-memory cache for tracking message states
    /// In production, consider using a persistent store (Redis, Database)
    message_cache: Arc<RwLock<HashMap<Uuid, TrackedMessage>>>,
    
    /// Metrics and monitoring (placeholder for production metrics)
    start_time: Instant,
    
    /// Connection health status
    last_health_check: Arc<RwLock<Option<Instant>>>,
}

impl SolanaAdapter {
    /// Create a new SolanaAdapter instance
    pub fn new(config: SolanaConfig) -> Result<Self, SolanaAdapterError> {
        info!("Initializing SolanaAdapter with RPC URL: {}", config.rpc_url);
        
        // Create RPC client with appropriate commitment level
        let commitment_config = CommitmentConfig {
            commitment: config.commitment,
        };
        
        let client = RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            commitment_config,
        );
        
        let adapter = Self {
            client,
            config,
            message_cache: Arc::new(RwLock::new(HashMap::new())),
            start_time: Instant::now(),
            last_health_check: Arc::new(RwLock::new(None)),
        };
    
        
        info!("SolanaAdapter initialized successfully");
        Ok(adapter)
    }
    
    /// Execute an operation with retry logic
    async fn with_retry<T, Fut, F>(&self, mut operation: F, operation_name: &str) -> Result<T, SolanaAdapterError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ClientError>>,
    {
        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        debug!("{} succeeded on attempt {}", operation_name, attempt + 1);
                    }
                    return Ok(result);
                }
                Err(error) => {
                    last_error = Some(error);

                    if attempt < self.config.max_retries {
                        let delay = if self.config.exponential_backoff {
                            self.config.retry_delay_ms * (2_u64.pow(attempt))
                        } else {
                            self.config.retry_delay_ms
                        };
                        warn!(
                            "{} failed on attempt {} (will retry in {} ms): {:?}",
                            operation_name, attempt + 1, delay, last_error
                        );
                        sleep(Duration::from_millis(delay)).await;
                    }
                }
            }
        }

        let error = last_error.unwrap();
        error!("{} failed after {} attempts: {:?}", operation_name, self.config.max_retries + 1, error);
        Err(SolanaAdapterError::RpcClient(error))
    }
    
    /// Clean up old entries from the message cache
    /// In production, this should be called periodically or triggered by cache size
    async fn cleanup_message_cache(&self) {
        const MAX_CACHE_AGE_HOURS: u64 = 24;
        let cutoff = SystemTime::now() - Duration::from_secs(MAX_CACHE_AGE_HOURS * 3600);

        let mut cache = self.message_cache.write().await;
        let initial_size = cache.len();
        cache.retain(|_, msg| msg.submitted_at > cutoff);
        let removed = initial_size - cache.len();
        if removed > 0 {
            debug!("Cleaned up {} old message cache entries", removed);
        }
    }

    
    /// Parse transaction signature from string
    fn parse_signature(sig_str: &str) -> Result<Signature, SolanaAdapterError> {
        sig_str.parse::<Signature>()
            .map_err(|e| SolanaAdapterError::Transaction(
                format!("Invalid signature format: {}", e)
            ))
    }
    
    /// Update message status in cache
    async fn update_message_status(&self, id: &Uuid, status: MessageStatus, signature: Option<String>) {
        let mut cache = self.message_cache.write().await;
        if let Some(msg) = cache.get_mut(id) {
            msg.status = status;
            if let Some(sig) = signature {
                msg.signature = Some(sig);
            }
            trace!("Updated message {} status to {:?}", id, msg.status);
        }
    }
}

#[async_trait]
impl ChainAdapter for SolanaAdapter {
    type BlockId = u64;
    type TxId = String;
    type Error = AdapterError;

    /// Get the latest finalized block slot
    async fn latest_block(&self) -> Result<Self::BlockId, Self::Error> {
        debug!("Fetching latest block slot");
        let slot = self.with_retry(
            || async { self.client.get_slot() },
            "get_latest_block"
        ).await.map_err(|e| {
            error!("Failed to fetch latest block: {:?}", e);
            AdapterError::Network(format!("Block fetch error: {}", e))
        })?;
        trace!("Latest block slot: {}", slot);
        Ok(slot)
    }

    /// Retrieve transaction data by signature
    async fn get_transaction(&self, tx_id: &Self::TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        debug!("Fetching transaction: {}", tx_id);
        let signature = Self::parse_signature(tx_id)
            .map_err(|e| AdapterError::InvalidInput(e.to_string()))?;
        let config = RpcTransactionConfig {
            encoding: Some(UiTransactionEncoding::Binary),
            commitment: Some(CommitmentConfig {
                commitment: self.config.commitment,
            }),
            max_supported_transaction_version: Some(0),
        };
        let result = self.with_retry(
            || async { 
                self.client.get_transaction_with_config(&signature, config.clone()) 
            },
            "get_transaction"
        ).await;
        
        match result {
            Ok(transaction) => {
                // Serialize the transaction for return
                match bincode::serialize(&transaction) {
                    Ok(serialized) => {
                        trace!("Successfully retrieved and serialized transaction: {}", tx_id);
                        Ok(Some(serialized))
                    }
                    Err(e) => {
                        error!("Failed to serialize transaction {}: {}", tx_id, e);
                        Err(AdapterError::Serialization(format!("Transaction serialization failed: {}", e)))
                    }
                }
            }
            Err(SolanaAdapterError::RpcClient(ClientError { 
                kind: ClientErrorKind::RpcError(rpc_error), .. 
            })) => {
                if let solana_client::rpc_request::RpcError::RpcResponseError { code, .. } = rpc_error {
                    if code == -32603 {
                        debug!("Transaction not found: {}", tx_id);
                        return Ok(None);
                    } else {
                        debug!("Unhandled Solana RPC error code {} for tx {}", code, tx_id);
                        // Falls through to the generic error below
                    }
                }
                // If not handled above, treat as a network error
                error!("Failed to fetch transaction {}: {:?}", tx_id, rpc_error);
                Err(AdapterError::Network(format!("Transaction fetch error: {:?}", rpc_error)))
            }
            Err(e) => {
                error!("Failed to fetch transaction {}: {:?}", tx_id, e);
                Err(AdapterError::Network(format!("Transaction fetch error: {}", e)))
            }
        }
    }

    /// Wait for a block to reach finality
    async fn wait_for_finality(&self, block: &Self::BlockId) -> Result<(), Self::Error> {
        info!("Waiting for finality of block slot: {}", block);

        let start_time = Instant::now();
        let timeout = Duration::from_secs(self.config.finality_timeout_secs);
        let poll_interval = Duration::from_millis(self.config.finality_poll_interval_ms);

        loop {
            if start_time.elapsed() > timeout {
                error!("Finality timeout for block {}", block);
                return Err(AdapterError::Timeout(format!(
                    "Finality timeout after {} seconds for block {}",
                    self.config.finality_timeout_secs, block
                )));
            }

            match self.latest_block().await {
                Ok(current_slot) => {
                    if current_slot >= *block {
                        info!("Block {} reached finality (current: {})", block, current_slot);
                        return Ok(());
                    }
                    trace!("Current slot {} < target slot {}, continuing to wait", current_slot, block);
                }
                Err(e) => {
                    warn!("Error checking finality for block {}: {:?}", block, e);
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Submit a FrostMessage to the Solana blockchain
    /// 
    /// # Note
    /// This is a stub implementation. In production, you would:
    /// 1. Serialize the message according to your program's instruction format
    /// 2. Create and sign the transaction
    /// 3. Submit to the network
    /// 4. Return the transaction signature
    async fn submit_message(&self, msg: &FrostMessage) -> Result<Self::TxId, Self::Error> {
        info!("Submitting FrostMessage with ID: {:?}", msg.id);
        
        // TODO: Implement actual message submission logic
        // This would involve:
        // 1. Creating instruction data from the FrostMessage
        // 2. Building a transaction with the instruction
        // 3. Signing the transaction with the relayer key
        // 4. Submitting via send_and_confirm_transaction
        
        // For now, we are going to return a mock transaction signature
        let mock_signature = format!(
            "{}{}",
            msg.id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        );
        
        // Track the message in our cache
        let tracked_msg = TrackedMessage {
            id: msg.id,
            signature: Some(mock_signature.clone()),
            status: MessageStatus::Pending,
            submitted_at: SystemTime::now(),
            block_slot: None,
        };
        
        let mut cache = self.message_cache.write().await;
        
        warn!("Using stub implementation for message submission");
        Ok(mock_signature)
    }

    /// Listen for and return relevant blockchain events
    ///
    /// # Note
    /// This stub implementation returns empty events. In production, we would:
    /// 1. Subscribe to program logs or account changes
    /// 2. Parse relevant events from transaction logs
    /// 3. Filter events related to FrostMessages
    /// 4. Return structured MessageEvent objects
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, Self::Error> {
        debug!("Listening for blockchain events");
        
        // TODO: Implement actual event listening
        // This would involve:
        // 1. Subscribing to program account changes or logs
        // 2. Parsing events from transaction logs
        // 3. Converting to MessageEvent format
        // 4. Filtering for relevant events
        
        // Cleanup old cache entries periodically
        self.cleanup_message_cache().await;
        
        // Return empty events for now
        trace!("Returning empty events (stub implementation)");
        Ok(vec![])
    }

    /// Verify a FrostMessage exists and is valid on-chain
    /// 
    /// # Note
    /// This stub implementation always succeeds. In production, we would:
    /// 1. Query the on-chain verifier program
    /// 2. Check message integrity and authenticity
    /// 3. Validate cryptographic proofs
    /// 4. Return verification result
    async fn verify_on_chain(&self, msg: &FrostMessage) -> Result<(), Self::Error> {
        debug!("Verifying FrostMessage on-chain: {:?}", msg.id);
        
        // TODO: Implement actual on-chain verification
        // This would involve:
        // 1. Calling the on-chain verifier program
        // 2. Checking message integrity
        // 3. Validating cryptographic signatures
        // 4. Confirming message exists in program state
        
        warn!("Using stub implementation for on-chain verification");
        Ok(())
    }

    /// Estimate transaction fee for submitting a FrostMessage
    async fn estimate_fee(&self, msg: &FrostMessage) -> Result<u128, Self::Error> {
        debug!("Estimating fee for FrostMessage: {:?}", msg.id);

        let recent_fees = self.with_retry(
            || async { self.client.get_recent_prioritization_fees(&[]) },
            "get_recent_fees"
        ).await;

        let base_fee = match recent_fees {
            Ok(fees) if !fees.is_empty() => {
                let avg_fee = fees.iter().map(|f| f.prioritization_fee).sum::<u64>() / fees.len() as u64;
                debug!("Using average recent fee: {} lamports", avg_fee);
                avg_fee as u128
            }
            _ => {
                debug!("Using fallback fee estimation");
                5_000u128 // Fallback fee estimate
            }
        };
        
        // Adjust fee based on message complexity (stub logic)
        let complexity_multiplier = match msg.payload.len() {
            0..=100 => 1.0,
            101..=500 => 1.2,
            501..=1000 => 1.5,
            _ => 2.0,
        };

        let estimated_fee = (base_fee as f64 * complexity_multiplier) as u128;

        debug!("Estimated fee: {} lamports (base: {}, multiplier: {})", 
               estimated_fee, base_fee, complexity_multiplier);

        Ok(estimated_fee)
    }

    /// Get the current status of a submitted message
    async fn message_status(&self, id: &Uuid) -> Result<MessageStatus, Self::Error> {
        trace!("Checking status for message: {}", id);
        
        let cache = self.message_cache.read().await;
        if let Some(tracked_msg) = cache.get(id) {
            // If we have a signature, check its on-chain status
            if let Some(signature) = &tracked_msg.signature {
                match self.get_transaction(signature).await {
                    Ok(Some(_)) => {
                        // Transaction found on-chain, update status to confirmed
                        drop(cache); // Release read lock before acquiring write lock
                        self.update_message_status(id, MessageStatus::Confirmed, None).await;
                        return Ok(MessageStatus::Confirmed);
                    }
                    Ok(None) => {
                        // Transaction not yet on-chain
                        return Ok(MessageStatus::Pending);
                    }
                    Err(_) => {
                        // Error checking transaction, return cached status
                        return Ok(tracked_msg.status.clone());
                    }
                }
            }
            Ok(tracked_msg.status.clone())
        } else {
            Err(AdapterError::MessageNotFound(format!("Message {} not found in cache", id)))
        }
    }

    /// Perform health check on the adapter and underlying connection
    async fn health_check(&self) -> Result<(), Self::Error> {
        debug!("Performing health check");
        
        let start_time = Instant::now();
        
        // Test basic connectivity by fetching latest block
        match self.latest_block().await {
            Ok(slot) => {
                let latency = start_time.elapsed();
                
                // Update last successful health check time
                let mut last_check = self.last_health_check.write().await;
                *last_check = Some(Instant::now());
                
                info!(
                    "Health check passed - Latest slot: {}, RPC latency: {:?}, Adapter uptime: {:?}",
                    slot, latency, self.start_time.elapsed()
                );
                
                // Log cache statistics
                let cache = self.message_cache.read().await;
                debug!("Message cache size: {}", cache.len());
                
                Ok(())
            }
            Err(e) => {
                error!("Health check failed: {:?}", e);
                Err(e)
            }
        }
    }
}

impl Drop for SolanaAdapter {
    fn drop(&mut self) {
        info!("SolanaAdapter shutting down after {:?} uptime", self.start_time.elapsed());
    }
}