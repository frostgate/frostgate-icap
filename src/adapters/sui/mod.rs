pub mod adapter;
pub mod client;
pub mod events;
pub mod types;

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;
use async_trait::async_trait;
use sui_sdk::{
    SuiClient,
    SuiClientBuilder,
    types::{
        base_types::{ObjectID, SuiAddress, TransactionDigest},
        transaction::{Transaction, TransactionData},
        messages::{ExecuteTransactionRequestType, TransactionEffects},
    },
    rpc_types::SuiExecuteTransactionResponse,
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

/// Sui chain adapter configuration
#[derive(Debug, Clone)]
pub struct SuiConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Chain ID (mainnet = 1, testnet = 2, devnet = 3)
    pub chain_id: u64,
    /// Package ID for message handling
    pub package_id: ObjectID,
    /// Required checkpoint confirmations
    pub required_confirmations: u64,
}

/// Sui chain adapter
pub struct SuiAdapter {
    /// Adapter configuration
    config: SuiConfig,
    /// Sui client
    client: Arc<SuiClient>,
    /// Health metrics
    metrics: Arc<RwLock<HealthMetrics>>,
    /// Unique adapter ID
    id: String,
}

impl SuiAdapter {
    /// Create a new Sui adapter
    pub async fn new(config: SuiConfig) -> Result<Self, AdapterError> {
        let client = SuiClientBuilder::default()
            .build(config.rpc_url.as_str())
            .await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to connect: {}", e)))?;

        // Check connection
        client.available_rpc_methods()
            .await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to get RPC methods: {}", e)))?;

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
            metrics,
            id: format!("sui-{}", Uuid::new_v4()),
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
        if let Ok(checkpoint) = self.client.read_api().get_latest_checkpoint_sequence_number().await {
            metrics.latest_block = Some(checkpoint);
        }
    }
}

#[async_trait]
impl ChainAdapter for SuiAdapter {
    fn chain_id(&self) -> ChainId {
        ChainId::from(self.config.chain_id)
    }

    fn adapter_id(&self) -> String {
        self.id.clone()
    }
}

#[async_trait]
impl FinalityProvider for SuiAdapter {
    type BlockId = u64; // Checkpoint sequence number

    async fn latest_finalized_block(&self) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        
        let latest = self.client.read_api().get_latest_checkpoint_sequence_number().await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get checkpoint: {}", e)))?;

        let finalized = latest.saturating_sub(self.config.required_confirmations);

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(FinalizedBlock {
            block: finalized,
            finality_proof: None,
            finalized_at: SystemTime::now(),
            confirmations: Some(self.config.required_confirmations),
        })
    }

    async fn wait_for_finality(
        &self,
        block: &Self::BlockId,
        timeout: Option<Duration>,
    ) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        let timeout = timeout.unwrap_or(Duration::from_secs(300));
        
        loop {
            let latest = self.client.read_api().get_latest_checkpoint_sequence_number().await
                .map_err(|e| AdapterError::ConnectionError(format!("Failed to get checkpoint: {}", e)))?;

            if latest >= block + self.config.required_confirmations {
                self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
                return Ok(FinalizedBlock {
                    block: *block,
                    finality_proof: None,
                    finalized_at: SystemTime::now(),
                    confirmations: Some(self.config.required_confirmations),
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
        
        let latest = self.client.read_api().get_latest_checkpoint_sequence_number().await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to get checkpoint: {}", e)))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(latest >= block + self.config.required_confirmations)
    }
}

#[async_trait]
impl MessageProver for SuiAdapter {
    async fn generate_proof(&self, message: &FrostMessage) -> Result<Vec<u8>, AdapterError> {
        // Sui uses object state for verification
        Err(AdapterError::CapabilityError("Proof generation not supported".into()))
    }

    async fn verify_proof(&self, message: &FrostMessage) -> Result<bool, AdapterError> {
        // Similar to above
        Err(AdapterError::CapabilityError("Proof verification not supported".into()))
    }
}

#[async_trait]
impl MessageSubmitter for SuiAdapter {
    type TxId = TransactionDigest;

    async fn submit_message(
        &self,
        message: &FrostMessage,
        options: Option<SubmissionOptions>,
    ) -> Result<Self::TxId, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create move call
        // 2. Submit transaction
        // 3. Wait for effects
        
        // For now just return a dummy digest
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(TransactionDigest::default())
    }

    async fn get_transaction(
        &self,
        tx_id: &Self::TxId,
    ) -> Result<Option<TransactionDetails>, AdapterError> {
        let start = SystemTime::now();
        
        let tx = match self.client.read_api().get_transaction(*tx_id).await
            .map_err(|e| AdapterError::TransactionError(format!("Failed to get transaction: {}", e)))? {
            Some(tx) => tx,
            None => {
                self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
                return Ok(None);
            }
        };

        let status = match tx.effects.status() {
            sui_types::execution_status::ExecutionStatus::Success => TransactionStatus::Confirmed,
            _ => TransactionStatus::Failed("Transaction failed".into()),
        };

        let tx = TransactionDetails::Parsed(crate::types::ParsedTransaction {
            hash: tx_id.to_vec(),
            from: None, // Would get from sender
            to: None, // Would get from package ID
            value: 0, // Would get from gas used
            data: Vec::new(), // Would get from move call
            status,
            metadata: HashMap::new(),
        });

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Some(tx))
    }

    async fn estimate_fee(&self, message: &FrostMessage) -> Result<u128, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create dry-run call
        // 2. Get gas budget
        // 3. Convert to MIST
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(1_000_000) // 1 MIST
    }
}

#[async_trait]
impl EventListener for SuiAdapter {
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Get latest checkpoint
        // 2. Filter for package events
        // 3. Convert to MessageEvents
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Vec::new())
    }

    async fn filter_events(
        &self,
        from_block: Option<u64>,
        to_block: Option<u64>,
        event_types: Option<Vec<String>>,
    ) -> Result<Vec<MessageEvent>, AdapterError> {
        // Implementation would use checkpoint range query
        Ok(Vec::new())
    }

    async fn subscribe(&self) -> Result<crate::traits::EventSubscription, AdapterError> {
        // Would use websocket subscription
        Err(AdapterError::CapabilityError("Subscription not implemented".into()))
    }
}

#[async_trait]
impl CapabilityProvider for SuiAdapter {
    async fn capabilities(&self) -> Result<ChainCapabilities, AdapterError> {
        Ok(ChainCapabilities {
            supports_smart_contracts: true,
            supports_native_tokens: true,
            supports_onchain_verification: true,
            max_message_size: 16_384, // 16KB
            proof_types: vec!["none".into()],
            finality_type: FinalityType::Probabilistic {
                confirmations: self.config.required_confirmations,
            },
            max_proof_size: None,
            supports_parallel_execution: true,
            features: HashMap::from([
                ("version".into(), "1.0.0".into()),
                ("chain_id".into(), self.config.chain_id.to_string()),
            ]),
        })
    }

    async fn supports_capability(&self, capability: &str) -> Result<bool, AdapterError> {
        Ok(match capability {
            "smart_contracts" | "native_tokens" | "parallel_execution" => true,
            "instant_finality" => false,
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