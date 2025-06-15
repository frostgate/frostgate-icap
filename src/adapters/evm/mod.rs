pub mod events;
pub mod clients;
pub mod types;


pub use types::{EthereumConfig, EthereumConfigBuilder};
pub use events::EventHandler;
pub use clients::RpcClient;

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;
use async_trait::async_trait;
use ethers::{
    providers::{Provider, Http, Middleware},
    types::{H256, Block, BlockNumber, TransactionReceipt, U256},
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

/// EVM chain adapter configuration
#[derive(Debug, Clone)]
pub struct EvmConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Chain ID
    pub chain_id: u64,
    /// Required confirmations for finality
    pub required_confirmations: u32,
    /// Contract addresses
    pub contracts: EvmContracts,
    /// Maximum gas price (in wei)
    pub max_gas_price: Option<U256>,
}

/// EVM contract addresses
#[derive(Debug, Clone)]
pub struct EvmContracts {
    /// Message handler contract
    pub message_handler: String,
    /// Verifier contract
    pub verifier: String,
}

/// EVM chain adapter
pub struct EvmAdapter {
    /// Adapter configuration
    config: EvmConfig,
    /// Ethers provider
    provider: Arc<Provider<Http>>,
    /// Health metrics
    metrics: Arc<RwLock<HealthMetrics>>,
    /// Unique adapter ID
    id: String,
}

impl EvmAdapter {
    /// Create a new EVM adapter
    pub async fn new(config: EvmConfig) -> Result<Self, AdapterError> {
        let provider = Provider::<Http>::try_from(&config.rpc_url)
            .map_err(|e| AdapterError::ConfigurationError(format!("Invalid RPC URL: {}", e)))?;
        
        // Check connection
        provider.get_block_number().await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to connect: {}", e)))?;

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
            provider: Arc::new(provider),
            metrics,
            id: format!("evm-{}", Uuid::new_v4()),
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
        if let Ok(block_number) = self.provider.get_block_number().await {
            metrics.latest_block = Some(block_number.as_u64());
        }
    }
}

#[async_trait]
impl ChainAdapter for EvmAdapter {
    fn chain_id(&self) -> ChainId {
        ChainId::from(self.config.chain_id)
    }

    fn adapter_id(&self) -> String {
        self.id.clone()
    }
}

#[async_trait]
impl FinalityProvider for EvmAdapter {
    type BlockId = H256;

    async fn latest_finalized_block(&self) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        
        let latest = self.provider.get_block_number().await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to get latest block: {}", e)))?;
        
        let finalized = latest.saturating_sub(self.config.required_confirmations.into());
        let block = self.provider.get_block(finalized).await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?
            .ok_or_else(|| AdapterError::FinalityError("Block not found".into()))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(FinalizedBlock {
            block: block.hash.unwrap_or_default(),
            finality_proof: None, // EVM doesn't have explicit finality proofs
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
        
        let target_block = self.provider.get_block(*block).await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?
            .ok_or_else(|| AdapterError::FinalityError("Block not found".into()))?;

        let target_number = target_block.number
            .ok_or_else(|| AdapterError::FinalityError("Block number not found".into()))?;

        loop {
            let latest = self.provider.get_block_number().await
                .map_err(|e| AdapterError::ConnectionError(format!("Failed to get latest block: {}", e)))?;

            if latest >= target_number + self.config.required_confirmations.into() {
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
        
        let target_block = self.provider.get_block(*block).await
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?
            .ok_or_else(|| AdapterError::FinalityError("Block not found".into()))?;

        let target_number = target_block.number
            .ok_or_else(|| AdapterError::FinalityError("Block number not found".into()))?;

        let latest = self.provider.get_block_number().await
            .map_err(|e| AdapterError::ConnectionError(format!("Failed to get latest block: {}", e)))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(latest >= target_number + self.config.required_confirmations.into())
    }
}

#[async_trait]
impl MessageProver for EvmAdapter {
    async fn generate_proof(&self, message: &FrostMessage) -> Result<Vec<u8>, AdapterError> {
        // EVM chains typically don't need message proofs
        // This would be implemented for L2s that need them
        Err(AdapterError::CapabilityError("Proof generation not supported".into()))
    }

    async fn verify_proof(&self, message: &FrostMessage) -> Result<bool, AdapterError> {
        // Similar to above
        Err(AdapterError::CapabilityError("Proof verification not supported".into()))
    }
}

#[async_trait]
impl MessageSubmitter for EvmAdapter {
    type TxId = H256;

    async fn submit_message(
        &self,
        message: &FrostMessage,
        options: Option<SubmissionOptions>,
    ) -> Result<Self::TxId, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create contract call to message handler
        // 2. Submit transaction
        // 3. Wait for receipt
        
        // For now just return a dummy hash
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(H256::zero())
    }

    async fn get_transaction(
        &self,
        tx_id: &Self::TxId,
    ) -> Result<Option<TransactionDetails>, AdapterError> {
        let start = SystemTime::now();
        
        let receipt = match self.provider.get_transaction_receipt(*tx_id).await
            .map_err(|e| AdapterError::TransactionError(format!("Failed to get receipt: {}", e)))? {
            Some(r) => r,
            None => {
                self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
                return Ok(None);
            }
        };

        let status = match receipt.status {
            Some(s) if s.as_u64() == 1 => TransactionStatus::Confirmed,
            Some(_) => TransactionStatus::Failed("Transaction reverted".into()),
            None => TransactionStatus::Pending,
        };

        let tx = TransactionDetails::Parsed(crate::types::ParsedTransaction {
            hash: tx_id.as_bytes().to_vec(),
            from: Some(receipt.from.as_bytes().to_vec()),
            to: receipt.to.map(|a| a.as_bytes().to_vec()),
            value: receipt.value.unwrap_or_default().as_u128(),
            data: receipt.input.to_vec(),
            status,
            metadata: HashMap::new(),
        });

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Some(tx))
    }

    async fn estimate_fee(&self, message: &FrostMessage) -> Result<u128, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would estimate gas * gas price
        // For now return a dummy value
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(1_000_000_000_000_000_u128) // 0.001 ETH
    }
}

#[async_trait]
impl EventListener for EvmAdapter {
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Get latest block
        // 2. Get events from message handler contract
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
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create filter
        // 2. Get matching events
        // 3. Convert to MessageEvents
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Vec::new())
    }

    async fn subscribe(&self) -> Result<crate::traits::EventSubscription, AdapterError> {
        Err(AdapterError::CapabilityError("Subscription not supported".into()))
    }
}

#[async_trait]
impl CapabilityProvider for EvmAdapter {
    async fn capabilities(&self) -> Result<ChainCapabilities, AdapterError> {
        Ok(ChainCapabilities {
            supports_smart_contracts: true,
            supports_native_tokens: true,
            supports_onchain_verification: true,
            max_message_size: 32_768, // 32KB
            proof_types: vec!["none".into()],
            finality_type: FinalityType::Probabilistic {
                confirmations: self.config.required_confirmations,
            },
            max_proof_size: None,
            supports_parallel_execution: false,
            features: HashMap::from([
                ("eip1559".into(), "true".into()),
                ("chain_id".into(), self.config.chain_id.to_string()),
            ]),
        })
    }

    async fn supports_capability(&self, capability: &str) -> Result<bool, AdapterError> {
        Ok(match capability {
            "smart_contracts" | "native_tokens" | "onchain_verification" => true,
            "parallel_execution" | "instant_finality" => false,
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