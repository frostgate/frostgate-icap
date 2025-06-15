pub mod adapter;
pub mod events;
pub mod clients;
pub mod types;

pub use adapter::SolanaAdapter;
pub use types::{SolanaConfig, SolanaConfigBuilder, SolanaAdapterError, TrackedMessage};
pub use events::EventHandler;
pub use clients::SolanaRpcClient;

use std::sync::Arc;
use std::time::{Duration, SystemTime};
use std::collections::HashMap;
use async_trait::async_trait;
use solana_client::{
    rpc_client::RpcClient,
    rpc_config::{RpcBlockConfig, RpcTransactionConfig},
};
use solana_sdk::{
    hash::Hash,
    commitment_config::{CommitmentConfig, CommitmentLevel},
    signature::Signature,
    transaction::Transaction,
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

/// Solana chain adapter configuration
#[derive(Debug, Clone)]
pub struct SolanaConfig {
    /// RPC endpoint URL
    pub rpc_url: String,
    /// Chain ID (mainnet = 1, testnet = 2, devnet = 3)
    pub chain_id: u64,
    /// Program ID for message handling
    pub program_id: String,
    /// Commitment level for finality
    pub commitment: CommitmentLevel,
}

/// Solana chain adapter
pub struct SolanaAdapter {
    /// Adapter configuration
    config: SolanaConfig,
    /// RPC client
    client: Arc<RpcClient>,
    /// Health metrics
    metrics: Arc<RwLock<HealthMetrics>>,
    /// Unique adapter ID
    id: String,
}

impl SolanaAdapter {
    /// Create a new Solana adapter
    pub async fn new(config: SolanaConfig) -> Result<Self, AdapterError> {
        let client = RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            CommitmentConfig {
                commitment: config.commitment,
            },
        );

        // Check connection
        client.get_version()
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
            client: Arc::new(client),
            metrics,
            id: format!("solana-{}", Uuid::new_v4()),
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
        if let Ok(slot) = self.client.get_slot() {
            metrics.latest_block = Some(slot);
        }
    }
}

#[async_trait]
impl ChainAdapter for SolanaAdapter {
    fn chain_id(&self) -> ChainId {
        ChainId::from(self.config.chain_id)
    }

    fn adapter_id(&self) -> String {
        self.id.clone()
    }
}

#[async_trait]
impl FinalityProvider for SolanaAdapter {
    type BlockId = Hash;

    async fn latest_finalized_block(&self) -> Result<FinalizedBlock<Self::BlockId>, AdapterError> {
        let start = SystemTime::now();
        
        let slot = self.client.get_slot()
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get slot: {}", e)))?;

        let block = self.client.get_block_with_config(
            slot,
            RpcBlockConfig {
                encoding: None,
                transaction_details: None,
                rewards: None,
                commitment: Some(self.config.commitment),
                max_supported_transaction_version: None,
            },
        ).map_err(|e| AdapterError::FinalityError(format!("Failed to get block: {}", e)))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(FinalizedBlock {
            block: block.blockhash,
            finality_proof: None,
            finalized_at: SystemTime::now(),
            confirmations: None, // Solana uses commitment levels instead
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
            let status = self.client.get_signature_statuses(&[Signature::new(block.as_ref())])
                .map_err(|e| AdapterError::FinalityError(format!("Failed to get status: {}", e)))?
                .value[0]
                .as_ref()
                .ok_or_else(|| AdapterError::FinalityError("Block not found".into()))?;

            if status.confirmation_status.as_ref().map_or(false, |c| {
                matches!(c.as_str(), "finalized" | "confirmed")
            }) {
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
        
        let status = self.client.get_signature_statuses(&[Signature::new(block.as_ref())])
            .map_err(|e| AdapterError::FinalityError(format!("Failed to get status: {}", e)))?
            .value[0]
            .as_ref()
            .ok_or_else(|| AdapterError::FinalityError("Block not found".into()))?;

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;

        Ok(status.confirmation_status.as_ref().map_or(false, |c| {
            matches!(c.as_str(), "finalized" | "confirmed")
        }))
    }
}

#[async_trait]
impl MessageProver for SolanaAdapter {
    async fn generate_proof(&self, message: &FrostMessage) -> Result<Vec<u8>, AdapterError> {
        // Solana uses account state for verification
        Err(AdapterError::CapabilityError("Proof generation not supported".into()))
    }

    async fn verify_proof(&self, message: &FrostMessage) -> Result<bool, AdapterError> {
        // Similar to above
        Err(AdapterError::CapabilityError("Proof verification not supported".into()))
    }
}

#[async_trait]
impl MessageSubmitter for SolanaAdapter {
    type TxId = Signature;

    async fn submit_message(
        &self,
        message: &FrostMessage,
        options: Option<SubmissionOptions>,
    ) -> Result<Self::TxId, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Create program instruction
        // 2. Submit transaction
        // 3. Wait for confirmation
        
        // For now just return a dummy signature
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Signature::default())
    }

    async fn get_transaction(
        &self,
        tx_id: &Self::TxId,
    ) -> Result<Option<TransactionDetails>, AdapterError> {
        let start = SystemTime::now();
        
        let tx = match self.client.get_transaction_with_config(
            tx_id,
            RpcTransactionConfig {
                encoding: None,
                commitment: Some(self.config.commitment),
                max_supported_transaction_version: None,
            },
        ).map_err(|e| AdapterError::TransactionError(format!("Failed to get transaction: {}", e)))? {
            Some(tx) => tx,
            None => {
                self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
                return Ok(None);
            }
        };

        let status = match tx.meta.and_then(|m| m.err) {
            Some(_) => TransactionStatus::Failed("Transaction failed".into()),
            None => TransactionStatus::Confirmed,
        };

        let tx = TransactionDetails::Parsed(crate::types::ParsedTransaction {
            hash: tx_id.as_ref().to_vec(),
            from: None, // Would get from first account
            to: None, // Would get from program ID
            value: 0, // Would get from lamports
            data: Vec::new(), // Would get from instruction data
            status,
            metadata: HashMap::new(),
        });

        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(Some(tx))
    }

    async fn estimate_fee(&self, message: &FrostMessage) -> Result<u128, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Get recent blockhash
        // 2. Create dummy transaction
        // 3. Get lamports from simulation
        
        self.update_metrics(true, start.elapsed().unwrap_or_default()).await;
        Ok(5000) // 5000 lamports
    }
}

#[async_trait]
impl EventListener for SolanaAdapter {
    async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        let start = SystemTime::now();
        
        // Implementation would:
        // 1. Get program accounts
        // 2. Filter for message events
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
        // Solana doesn't support historical events directly
        // Would need to maintain an event index
        Ok(Vec::new())
    }

    async fn subscribe(&self) -> Result<crate::traits::EventSubscription, AdapterError> {
        // Would use websocket subscription
        Err(AdapterError::CapabilityError("Subscription not implemented".into()))
    }
}

#[async_trait]
impl CapabilityProvider for SolanaAdapter {
    async fn capabilities(&self) -> Result<ChainCapabilities, AdapterError> {
        Ok(ChainCapabilities {
            supports_smart_contracts: true,
            supports_native_tokens: true,
            supports_onchain_verification: true,
            max_message_size: 1024, // 1KB instruction data limit
            proof_types: vec!["none".into()],
            finality_type: FinalityType::Probabilistic {
                confirmations: 32, // Default for finalized
            },
            max_proof_size: None,
            supports_parallel_execution: true,
            features: HashMap::from([
                ("version".into(), "1.16.15".into()),
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