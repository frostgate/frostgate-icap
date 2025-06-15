pub mod events;
pub mod client;
pub mod types;

pub use types::{PolkadotConfig, TransactionInfo, TxTrackingStatus};
pub use events::EventHandler;
pub use client::RpcClient;

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;
use async_trait::async_trait;
use subxt::{
    OnlineClient,
    PolkadotConfig as SubxtPolkadotConfig,
    blocks::{Block, BlockRef},
    utils::{AccountId32, H256},
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::traits::{
    ChainAdapter, FinalityProvider, MessageProver, MessageSubmitter,
    EventListener, CapabilityProvider,
};
use crate::types::{
    AdapterError, ChainCapabilities, ConnectionStatus, FinalizedBlock,
    HealthMetrics, SubmissionOptions, TransactionDetails, TransactionStatus,
    FinalityType,
};
use frostgate_sdk::message::{ChainId, FrostMessage, MessageEvent};

/// Substrate chain adapter configuration
#[derive(Debug, Clone)]
pub struct SubstrateConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Chain ID
    pub chain_id: u64,
    /// Pallet name for message handling
    pub pallet_name: String,
    /// Maximum weight per extrinsic
    pub max_weight: u64,
}

/// Substrate chain adapter
pub struct SubstrateAdapter {
    /// Adapter configuration
    config: SubstrateConfig,
    /// Subxt client
    client: Arc<OnlineClient<SubxtPolkadotConfig>>,
    /// Event handler
    event_handler: Arc<EventHandler>,
    /// Health metrics
    metrics: Arc<RwLock<HealthMetrics>>,
    /// Unique adapter ID
    id: String,
}

impl SubstrateAdapter {
    /// Create a new Substrate adapter
    pub async fn new(config: SubstrateConfig) -> Result<Self, AdapterError> {
        let client = OnlineClient::<SubxtPolkadotConfig>::from_url(&config.rpc_url)
            .await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to connect: {}", e)))?;

        let event_handler = Arc::new(EventHandler::new(
            Arc::new(client.clone()),
            config.pallet_name.clone(),
        ));

        let metrics = Arc::new(RwLock::new(HealthMetrics {
            last_successful: Some(SystemTime::now()),
            consecutive_failures: 0,
            total_operations: 0,
            failed_operations: 0,
            avg_response_time: Duration::from_secs(0),
            connection_status: ConnectionStatus::Healthy,
            latest_block: None,
            custom_metrics: HashMap::new(),
        }));

        Ok(Self {
            config,
            client: Arc::new(client),
            event_handler,
            metrics,
            id: format!("substrate-{}", Uuid::new_v4()),
        })
    }

    /// Update health metrics
    async fn update_metrics(&self, success: bool, response_time: Duration) {
        let mut metrics = self.metrics.write().await;
        
        metrics.total_operations += 1;
        if !success {
            metrics.failed_operations += 1;
            metrics.consecutive_failures += 1;
        } else {
            metrics.consecutive_failures = 0;
            metrics.last_successful = Some(SystemTime::now());
        }

        // Update average response time
        let total_ops = metrics.total_operations as u32;
        metrics.avg_response_time = Duration::from_nanos(
            ((metrics.avg_response_time.as_nanos() * (total_ops - 1) as u128 +
              response_time.as_nanos()) / total_ops as u128) as u64
        );

        // Update connection status
        metrics.connection_status = if metrics.consecutive_failures > 5 {
            ConnectionStatus::Unhealthy("Too many consecutive failures".into())
        } else if metrics.avg_response_time > Duration::from_secs(5) {
            ConnectionStatus::Degraded("High response time".into())
        } else {
            ConnectionStatus::Healthy
        };

        // Update latest block
        if let Ok(block) = self.client.blocks().at_latest().await {
            metrics.latest_block = Some(block.number() as u64);
        }
    }
}

#[async_trait]
impl ChainAdapter for SubstrateAdapter {
    fn chain_id(&self) -> ChainId {
        ChainId::from(self.config.chain_id)
    }

    fn adapter_id(&self) -> String {
        self.id.clone()
    }
}

#[async_trait]
impl FinalityProvider for SubstrateAdapter {
    type BlockId = H256;

    async fn latest_finalized_block(&self) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        
        let block = self.client.blocks().at_finalized().await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get finalized block: {}", e)))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(FinalizedBlock {
            block: block.hash(),
            finality_proof: None, // Substrate has GRANDPA but we don't expose it here
            finalized_at: SystemTime::now(),
            confirmations: None, // Deterministic finality
        })
    }

    async fn wait_for_finality(
        &self,
        block: &Self::BlockId,
        timeout: Option<Duration>,
    ) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        let timeout = timeout.unwrap_or(Duration::from_secs(300));
        
        let target_block = self.client.blocks().at(BlockRef::Hash(*block)).await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?;

        loop {
            let finalized = self.client.blocks().at_finalized().await
                .map_err(|e| AdapterError::ConnectionError(format!("Failed to get finalized block: {}", e)))?;

            if finalized.number() >= target_block.number() {
                self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
                return Ok(FinalizedBlock {
                    block: *block,
                    finality_proof: None,
                    finalized_at: SystemTime::now(),
                    confirmations: None,
                });
            }

            if start.elapsed().unwrap_or_default() > timeout {
                return Err(AdapterError::FinalityError("Timeout waiting for finality".into()));
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn is_finalized(&self, block: &Self::BlockId) -> Result<bool, AdapterError> {
        let start = SystemTime::now();
        
        let target_block = self.client.blocks().at(BlockRef::Hash(*block)).await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?;

        let finalized = self.client.blocks().at_finalized().await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to get finalized block: {}", e)))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(finalized.number() >= target_block.number())
    }
}

#[async_trait]
impl MessageProver for SubstrateAdapter {
    async fn generate_proof(&self, message: &FrostMessage) -> Result<Vec<u8>, AdapterError> {
        // Substrate chains typically don't need message proofs
        Err(AdapterError::CapabilityError("Proof generation not supported".into()))
    }

    async fn verify_proof(&self, message: &FrostMessage) -> Result<bool, AdapterError> {
        // Similar to above
        Err(AdapterError::CapabilityError("Proof verification not supported".into()))
    }
}

#[async_trait]
impl MessageSubmitter for SubstrateAdapter {
    type TxId = H256;

    async fn submit_message(
        &self,
        message: &FrostMessage,
        options: Option<SubmissionOptions>,
    ) -> Result<Self::TxId, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create extrinsic call to message pallet
        // 2. Submit and wait for in-block
        // 3. Return block hash
        
        // For now just return a dummy hash
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(H256::zero())
    }

    async fn get_transaction(
        &self,
        tx_id: &Self::TxId,
    ) -> Result<Option<TransactionDetails>, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Get block containing transaction
        // 2. Find extrinsic in block
        // 3. Convert to TransactionDetails
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(None)
    }

    async fn estimate_fee(&self, message: &FrostMessage) -> Result<u128, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create dry-run call
        // 2. Get weight
        // 3. Convert to fee
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(1_000_000_000) // 1 DOT
    }
}

#[async_trait]
impl EventListener for SubstrateAdapter {
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        // Delegate to event handler
        self.event_handler.listen_for_events().await
    }

    async fn filter_events(
        &self,
        from_block: Option<u64>,
        to_block: Option<u64>,
        event_types: Option<Vec<String>>,
    ) -> Result<Vec<MessageEvent>, AdapterError> {
        // Implementation would use storage queries to get historical events
        Ok(Vec::new())
    }

    async fn subscribe(&self) -> Result<crate::traits::EventSubscription, AdapterError> {
        // Implementation would use client.subscribe_finalized_blocks()
        Err(AdapterError::CapabilityError("Subscription not implemented".into()))
    }
}

#[async_trait]
impl CapabilityProvider for SubstrateAdapter {
    async fn capabilities(&self) -> Result<ChainCapabilities, AdapterError> {
        Ok(ChainCapabilities {
            supports_smart_contracts: false, // Unless using contracts pallet
            supports_native_tokens: true,
            supports_onchain_verification: true,
            max_message_size: 16_384, // 16KB
            proof_types: vec!["none".into()],
            finality_type: FinalityType::Deterministic,
            max_proof_size: None,
            supports_parallel_execution: false,
            features: HashMap::from([
                ("runtime_version".into(), "9430".into()),
                ("chain_id".into(), self.config.chain_id.to_string()),
            ]),
        })
    }

    async fn supports_capability(&self, capability: &str) -> Result<bool, AdapterError> {
        Ok(match capability {
            "native_tokens" | "onchain_verification" => true,
            "smart_contracts" | "parallel_execution" => false,
            _ => false,
        })
    }

    async fn connection_status(&self) -> Result<ConnectionStatus, AdapterError> {
        Ok(self.metrics.read().await.connection_status.clone())
    }

    async fn health_metrics(&self) -> Result<HealthMetrics, AdapterError> {
        Ok(self.metrics.read().await.clone())
    }
}
