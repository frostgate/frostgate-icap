#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::str::FromStr;
use async_trait::async_trait;
use subxt::{
    self,
    OnlineClient, PolkadotConfig as SubxtPolkadotConfig,
    tx::{PairSigner, TxStatus, TxProgress, TxInBlock, Signer},
    utils::H256,
    config::{ExtrinsicParams, substrate::H256 as SubxtHash, Config},
    metadata::Metadata,
    blocks::{BlockRef, Block},
    backend::legacy::rpc_methods::RuntimeVersion,
};
use sp_keyring::AccountKeyring;
use subxt::ext::sp_core::{sr25519::Pair as Sr25519Pair, Pair};
use subxt_signer::sr25519::Keypair as SubxtKeypair;
use subxt_signer::SecretUri;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, interval, MissedTickBehavior};
use tracing::{debug, error, info, warn, instrument, span, Level};
use uuid::Uuid;
use futures::StreamExt;
use futures::TryFutureExt;
use serde_json;

use crate::chainadapter::{ChainAdapter, AdapterError};
use crate::types::{HealthMetrics, ConnectionStatus};
use crate::substrate::types::{
    PolkadotConfig, TransactionInfo, TxTrackingStatus,
    DEFAULT_CONFIRMATIONS, MAX_FINALITY_WAIT_TIME, SUBSTRATE_BLOCK_TIME,
    DEFAULT_MAX_CONCURRENT_REQUESTS,
};
use crate::substrate::client::RpcClient;
use crate::substrate::events::EventHandler;
use frostgate_sdk::frostmessage::{FrostMessage, MessageEvent, MessageStatus};

/// Polkadot/Substrate chain adapter
pub struct PolkadotAdapter {
    /// RPC client for chain interaction
    client: RpcClient,
    
    /// Event handler for message events
    event_handler: EventHandler,
    
    /// Adapter configuration
    config: PolkadotConfig,
    
    /// Message ID to transaction mapping
    message_tx_mapping: Arc<RwLock<HashMap<Uuid, TransactionInfo>>>,
    
    /// Connection health metrics
    health_metrics: Arc<Mutex<HealthMetrics>>,
    
    /// Transaction signer
    signer: Arc<SubxtKeypair>,
    
    /// Background health monitor task handle
    _health_monitor_handle: tokio::task::JoinHandle<()>,
    
    /// Cached metadata for the chain
    metadata_cache: Arc<RwLock<Option<Metadata>>>,
    
    /// Hash of the current metadata
    metadata_hash: Arc<RwLock<Option<SubxtHash>>>,
    
    /// Current runtime version
    runtime_version: Arc<RwLock<Option<RuntimeVersion>>>,
}

impl PolkadotAdapter {
    /// Creates a new PolkadotAdapter instance
    pub async fn new(config: PolkadotConfig) -> Result<Self, AdapterError> {
        let span = span!(Level::INFO, "polkadot_adapter_init");
        let _enter = span.enter();
        
        info!("Initializing PolkadotAdapter for {}", config.rpc_url);
        
        // Create RPC client
        let timeout = config.rpc_timeout.unwrap_or(Duration::from_secs(30));
        let max_concurrent = config.max_concurrent_requests.unwrap_or(DEFAULT_MAX_CONCURRENT_REQUESTS) as usize;
        let mut client = RpcClient::new(&config.rpc_url, timeout, max_concurrent).await?;
        
        // Initialize transaction signer
        let signer = Self::create_signer(&config)?;
        
        // Create event handler
        let event_handler = EventHandler::new(
            Arc::new(client.client().clone()),
            config.message_pallet.clone().unwrap_or_else(|| "XcmpQueue".to_string()),
        );
        
        // Initialize health metrics
        let health_metrics = Arc::new(Mutex::new(HealthMetrics::default()));
        
        // Start health monitoring if configured
        let health_monitor_handle = if let Some(interval) = config.health_check_interval {
            Self::start_health_monitor(
                client.client().clone(),
                Arc::clone(&health_metrics),
                interval,
            )
        } else {
            tokio::spawn(async {})
        };
        
        let adapter = Self {
            client,
            event_handler,
            config,
            message_tx_mapping: Arc::new(RwLock::new(HashMap::new())),
            health_metrics,
            signer: Arc::new(signer),
            _health_monitor_handle: health_monitor_handle,
            metadata_cache: Arc::new(RwLock::new(None)),
            metadata_hash: Arc::new(RwLock::new(None)),
            runtime_version: Arc::new(RwLock::new(None)),
        };
        
        // Initialize metadata and runtime version
        adapter.update_metadata().await?;
        adapter.update_runtime_version().await?;
        
        // Perform initial health check
        adapter.health_check().await?;
        
        info!("PolkadotAdapter initialized successfully");
        Ok(adapter)
    }
    
    /// Creates and configures the transaction signer
    fn create_signer(config: &PolkadotConfig) -> Result<SubxtKeypair, AdapterError> {
        let secret_uri = config.signer_seed.parse::<SecretUri>()
            .map_err(|e| AdapterError::Configuration(format!("Invalid signer seed: {}", e)))?;
            
        SubxtKeypair::from_uri(&secret_uri)
            .map_err(|e| AdapterError::Other(format!("Failed to create keypair: {}", e)))
    }
    
    /// Starts the background health monitoring task
    fn start_health_monitor(
        client: OnlineClient<SubxtPolkadotConfig>,
        health_metrics: Arc<Mutex<HealthMetrics>>,
        check_interval: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval_timer = interval(check_interval);
            interval_timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
            
            loop {
                interval_timer.tick().await;
                
                match client.blocks().at_latest().await {
                    Ok(block) => {
                        let header = block.header();
                        let mut metrics = health_metrics.lock().await;
                        metrics.connection_status = ConnectionStatus::Healthy;
                        metrics.consecutive_failures = 0;
                        metrics.latest_block_seen = Some(header.number);
                        metrics.last_successful_call = Some(Instant::now());
                        metrics.last_successful_timestamp = Some(SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64);
                    }
                    Err(e) => {
                        warn!("Health check failed: {}", e);
                        let mut metrics = health_metrics.lock().await;
                        metrics.consecutive_failures += 1;
                        metrics.connection_status = ConnectionStatus::Unhealthy;
                    }
                }
            }
        })
    }
    
    /// Updates transaction status
    async fn update_transaction_status(
        &self,
        message_id: &Uuid,
        status: TxTrackingStatus,
        block_number: Option<u32>,
        error: Option<String>,
    ) {
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

    /// Monitors transaction progress and updates status
    async fn monitor_transaction_progress(
        &self,
        message_id: Uuid,
        mut progress: TxProgress<SubxtPolkadotConfig, OnlineClient<SubxtPolkadotConfig>>,
    ) {
        while let Some(status) = progress.next().await {
            match status {
                Ok(status) => {
                    if let Some(block_ref) = status.as_in_block() {
                        // Get block number from the block hash
                        let client = self.client.client();
                        // Extract the block hash from TxInBlock
                        let block_hash = block_ref.block_hash();
                        if let Ok(block) = client.blocks().at(block_hash).await {
                            let block_number = block.header().number;
                            self.update_transaction_status(
                                &message_id,
                                TxTrackingStatus::InBlock,
                                Some(block_number),
                                None,
                            ).await;
                        }
                    } else if status.as_finalized().is_some() {
                        self.update_transaction_status(
                            &message_id,
                            TxTrackingStatus::Finalized,
                            None,
                            None,
                        ).await;
                        break;
                    }
                }
                Err(e) => {
                    error!("Transaction failed for message {}: {}", message_id, e);
                    self.update_transaction_status(
                        &message_id,
                        TxTrackingStatus::Failed,
                        None,
                        Some(e.to_string()),
                    ).await;
                    break;
                }
            }
        }
    }

    /// Updates the chain metadata if it has changed
    async fn update_metadata(&self) -> Result<(), AdapterError> {
        let client = self.client.client();
        
        // Get current metadata hash
        let metadata = client.metadata();
        let new_hash = H256::from_slice(&metadata.hasher().hash());
        
        // Check if metadata has changed
        let mut hash_lock = self.metadata_hash.write().await;
        if hash_lock.as_ref() != Some(&new_hash) {
            info!("Chain metadata updated, hash: {:?}", new_hash);
            
            // Update cache
            let mut cache_lock = self.metadata_cache.write().await;
            *cache_lock = Some(metadata.clone());
            *hash_lock = Some(new_hash);
            
            // Validate metadata contains our pallet
            self.validate_metadata(&metadata).await?;
        }
        
        Ok(())
    }
    
    /// Validates that the metadata contains our required pallet and calls
    async fn validate_metadata(&self, metadata: &Metadata) -> Result<(), AdapterError> {
        let pallet_name = self.config.message_pallet.as_deref().unwrap_or("XcmpQueue");
        
        // Check pallet exists
        let pallet = metadata.pallet_by_name(pallet_name)
            .ok_or_else(|| AdapterError::Configuration(
                format!("Pallet {} not found in metadata", pallet_name)
            ))?;
            
        // Check required calls exist
        let variants = pallet.call_variants().ok_or_else(|| 
            AdapterError::Configuration("Failed to get call variants".to_string())
        )?;
        
        let _submit_message = variants.iter()
            .find(|call| call.name == "submit_message")
            .ok_or_else(|| AdapterError::Configuration(
                format!("Required call 'submit_message' not found in pallet {}", pallet_name)
            ))?;
            
        // Check required events exist
        let event_variants = pallet.event_variants().ok_or_else(|| 
            AdapterError::Configuration("Failed to get event variants".to_string())
        )?;
        
        let _message_sent = event_variants.iter()
            .find(|event| event.name == "MessageSent")
            .ok_or_else(|| AdapterError::Configuration(
                format!("Required event 'MessageSent' not found in pallet {}", pallet_name)
            ))?;
            
        Ok(())
    }
    
    /// Updates the runtime version if it has changed
    async fn update_runtime_version(&self) -> Result<(), AdapterError> {
        let client = self.client.client();
        // Get current runtime version from the backend
        let backend_version = client.backend().current_runtime_version()
            .await
            .map_err(|e| AdapterError::Other(format!("Failed to get runtime version: {}", e)))?;
            
        // Convert from subxt::backend::RuntimeVersion to subxt::backend::legacy::rpc_methods::RuntimeVersion
        let mut other = std::collections::HashMap::new();
        other.insert("specVersion".to_string(), serde_json::Value::Number(serde_json::Number::from(backend_version.spec_version)));
        other.insert("transactionVersion".to_string(), serde_json::Value::Number(serde_json::Number::from(backend_version.transaction_version)));
        
        let new_version = subxt::backend::legacy::rpc_methods::RuntimeVersion {
            spec_version: backend_version.spec_version,
            transaction_version: backend_version.transaction_version,
            other,
        };
        
        let mut version_lock = self.runtime_version.write().await;
        if version_lock.as_ref() != Some(&new_version) {
            info!(
                "Runtime version updated: spec_version={}, transaction_version={}",
                backend_version.spec_version, backend_version.transaction_version
            );
            *version_lock = Some(new_version);
        }
        
        Ok(())
    }
    
    /// Checks for runtime upgrades
    async fn check_runtime_upgrade(&self) -> Result<bool, AdapterError> {
        let old_version = {
            let version_lock = self.runtime_version.read().await;
            version_lock.clone()
        };
        
        self.update_runtime_version().await?;
        
        let new_version = {
            let version_lock = self.runtime_version.read().await;
            version_lock.clone()
        };
        
        Ok(old_version != new_version)
    }
}

impl Clone for PolkadotAdapter {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            event_handler: self.event_handler.clone(),
            config: self.config.clone(),
            message_tx_mapping: Arc::clone(&self.message_tx_mapping),
            health_metrics: Arc::clone(&self.health_metrics),
            signer: Arc::clone(&self.signer),
            _health_monitor_handle: tokio::spawn(async {}),
            metadata_cache: Arc::clone(&self.metadata_cache),
            metadata_hash: Arc::clone(&self.metadata_hash),
            runtime_version: Arc::clone(&self.runtime_version),
        }
    }
}

#[async_trait]
impl ChainAdapter for PolkadotAdapter {
    type BlockId = u32;
    type TxId = H256;
    type Error = AdapterError;

    async fn latest_block(&self) -> Result<Self::BlockId, Self::Error> {
        let client = self.client.client();
        let block = client.blocks().at_latest().await?;
        Ok(block.header().number)
    }

    async fn get_transaction(&self, tx: &Self::TxId) -> Result<Option<Vec<u8>>, Self::Error> {
        // Substrate doesn't have direct transaction lookup by hash
        // This would require either indexing or block scanning
        warn!("Transaction lookup not implemented for Substrate chains");
        Ok(None)
    }

    async fn wait_for_finality(&self, block: &Self::BlockId) -> Result<(), Self::Error> {
        let confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);
        let target_block = *block + confirmations;
        let start_time = Instant::now();
        
        info!(
            "Waiting for block {} to reach finality (target: {})",
            block, target_block
        );
        
        let block_time = self.config.chain_specific
            .as_ref()
            .and_then(|c| c.block_time)
            .unwrap_or(SUBSTRATE_BLOCK_TIME);
        
        loop {
            if start_time.elapsed() > MAX_FINALITY_WAIT_TIME {
                return Err(AdapterError::Timeout(
                    format!("Finality wait timeout for block {}", block)
                ));
            }
            
            let current_block = self.latest_block().await?;
            if current_block >= target_block {
                info!("Block {} reached finality", block);
                return Ok(());
            }
            
            sleep(block_time).await;
        }
    }

    /// Submits a message to the chain
    async fn submit_message(&self, msg: &FrostMessage) -> Result<Self::TxId, Self::Error> {
        info!("Submitting message {} to chain", msg.id);
        
        // Check for runtime upgrades before submitting
        if self.check_runtime_upgrade().await? {
            // Metadata might have changed with runtime
            self.update_metadata().await?;
        }
        
        let client = self.client.client();
        
        // Create the call arguments
        use subxt::ext::scale_value::{Value, Composite, Primitive};
        let call_args = vec![
            Value::primitive(Primitive::U128(msg.from_chain as u128)),
            Value::primitive(Primitive::U128(msg.to_chain as u128)),
            Value::from_bytes(msg.payload.clone()),
            Value::primitive(Primitive::U128(msg.nonce as u128)),
            Value::primitive(Primitive::U128(msg.timestamp as u128)),
            Value::primitive(Primitive::U128(msg.fee.unwrap_or(0u128))),
        ];
        
        // Create a dynamic payload
        let call = subxt::dynamic::tx(
            "FrostgateMessage",
            "submit_message",
            Composite::from(call_args),
        );
        
        // Create PairSigner from our keypair
        let pair_signer = PairSigner::new(Sr25519Pair::from_string(&self.config.signer_seed, None)
            .map_err(|e| AdapterError::Other(format!("Failed to create signer: {}", e)))?);
            
        // Create and sign the extrinsic
        let tx = client.tx()
            .create_signed(&call, &pair_signer, Default::default())
            .await
            .map_err(|e| AdapterError::Other(format!("Failed to create transaction: {}", e)))?;
            
        // Submit and wait for in-block
        let tx_progress = tx.submit_and_watch().await
            .map_err(|e| AdapterError::Network(format!("Failed to submit transaction: {}", e)))?;
            
        // Store initial transaction info
        let tx_hash = tx_progress.extrinsic_hash();
        let tx_info = TransactionInfo {
            message_id: msg.id,
            tx_hash,
            block_number: None,
            status: TxTrackingStatus::Submitted,
            submitted_at: Instant::now(),
            confirmations: 0,
            error: None,
        };
        
        let mut mapping = self.message_tx_mapping.write().await;
        mapping.insert(msg.id, tx_info);
        drop(mapping);
        
        // Monitor transaction progress
        let this = self.clone();
        let message_id = msg.id;
        tokio::spawn(async move {
            this.monitor_transaction_progress(message_id, tx_progress).await;
        });
        
        info!("Message {} submitted with tx hash {}", msg.id, tx_hash);
        Ok(tx_hash)
    }

    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, Self::Error> {
        self.event_handler.listen_for_events().await
    }

    async fn verify_on_chain(&self, msg: &FrostMessage) -> Result<(), Self::Error> {
        // TODO: Implement actual verification
        Ok(())
    }

    async fn estimate_fee(&self, msg: &FrostMessage) -> Result<u128, Self::Error> {
        let client = self.client.client();
        
        use subxt::ext::scale_value::{Value, Composite, Primitive};
        
        // Create the call arguments with explicit types
        let call_args = vec![
            Value::primitive(Primitive::u128(msg.from_chain as u128)),
            Value::primitive(Primitive::u128(msg.to_chain as u128)),
            Value::from_bytes(msg.payload.clone()),
            Value::primitive(Primitive::u128(msg.nonce as u128)),
            Value::primitive(Primitive::u128(msg.timestamp as u128)),
            Value::primitive(Primitive::u128(msg.fee.unwrap_or(0u128))),
        ];
        
        // Create the call for fee estimation
        let call = subxt::dynamic::tx(
            "FrostgateMessage",
            "submit_message",
            Composite::from(call_args),
        );
        
        // Create PairSigner from our keypair
        let pair_signer = PairSigner::new(Sr25519Pair::from_string(&self.config.signer_seed, None)
            .map_err(|e| AdapterError::Other(format!("Failed to create signer: {}", e)))?);
            
        // Get fee estimate using the correct API
        let tx = client.tx()
            .create_signed(&call, &pair_signer, Default::default())
            .await
            .map_err(|e| AdapterError::Other(format!("Failed to create transaction: {}", e)))?;
            
        let fee_details = tx.partial_fee_estimate().await
            .map_err(|e| AdapterError::Other(format!("Failed to estimate fee: {}", e)))?;
            
        Ok(fee_details)
    }

    async fn message_status(&self, id: &Uuid) -> Result<MessageStatus, Self::Error> {
        let mapping = self.message_tx_mapping.read().await;
        
        if let Some(tx_info) = mapping.get(id) {
            let status = match tx_info.status {
                TxTrackingStatus::Submitted => MessageStatus::Pending,
                TxTrackingStatus::InBlock => {
                    if let Some(block_num) = tx_info.block_number {
                        let current_block = self.latest_block().await?;
                        let confirmations = current_block.saturating_sub(block_num);
                        let required_confirmations = self.config.confirmations.unwrap_or(DEFAULT_CONFIRMATIONS);
                        
                        if confirmations >= required_confirmations {
                            MessageStatus::Confirmed
                        } else {
                            MessageStatus::Pending
                        }
                    } else {
                        MessageStatus::Pending
                    }
                }
                TxTrackingStatus::Finalized => MessageStatus::Confirmed,
                TxTrackingStatus::Failed => {
                    MessageStatus::Failed(tx_info.error.clone().unwrap_or_else(|| "Unknown error".to_string()))
                }
                TxTrackingStatus::Unknown => MessageStatus::Pending,
            };
            
            Ok(status)
        } else {
            Ok(MessageStatus::Pending)
        }
    }

    async fn health_check(&self) -> Result<(), Self::Error> {
        // Check basic connectivity
        let latest_block = self.latest_block().await?;
        debug!("Health check - latest block: {}", latest_block);
        
        // Check metrics
        let metrics = self.health_metrics.lock().await;
        match metrics.connection_status {
            ConnectionStatus::Healthy => {
                info!("Health check passed");
                Ok(())
            }
            ConnectionStatus::Degraded => {
                warn!("Health check warning - connection degraded");
                Ok(())
            }
            ConnectionStatus::Unhealthy => {
                Err(AdapterError::Network("Connection unhealthy".to_string()))
            }
            ConnectionStatus::Unknown => {
                warn!("Health check - status unknown");
                Ok(())
            }
        }
    }
}

impl Drop for PolkadotAdapter {
    fn drop(&mut self) {
        info!("PolkadotAdapter is being dropped - cleaning up resources");
    }
}
