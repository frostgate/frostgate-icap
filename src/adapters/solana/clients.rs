#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use solana_client::{
    rpc_client::RpcClient,
    rpc_config::{RpcTransactionConfig, RpcSendTransactionConfig},
    client_error::{ClientError, ClientErrorKind},
    rpc_request::RpcRequest,
};
use solana_sdk::{
    commitment_config::{CommitmentConfig, CommitmentLevel},
    signature::Signature,
    transaction::{Transaction, VersionedTransaction},
};
use solana_transaction_status::{UiTransactionEncoding, EncodedTransactionWithStatusMeta};
use std::time::{Duration, Instant};
use std::str::FromStr;
use tokio::time::sleep;
use tracing::{debug, warn};
use crate::chainadapter::AdapterError;
use crate::solana::types::{SolanaConfig, SolanaAdapterError, constants::*};

/// RPC client wrapper with retry and error handling capabilities
pub struct SolanaRpcClient {
    /// Inner RPC client from solana-client
    client: RpcClient,
    
    /// Configuration parameters
    config: SolanaConfig,
    
    /// Last RPC call timestamp for rate limiting
    last_call: Option<Instant>,
}

impl SolanaRpcClient {
    /// Creates a new RPC client with the given configuration
    pub fn new(config: SolanaConfig) -> Self {
        let commitment_config = CommitmentConfig {
            commitment: config.commitment,
        };
        
        let client = RpcClient::new_with_commitment(
            config.rpc_url.clone(),
            commitment_config,
        );
        
        Self {
            client,
            config,
            last_call: None,
        }
    }
    
    /// Executes an RPC call with retry logic and rate limiting
    pub async fn execute<T, F>(&mut self, mut operation: F) -> Result<T, SolanaAdapterError>
    where
        F: FnMut() -> Result<T, ClientError>,
    {
        self.enforce_rate_limit().await;
        
        let mut last_error = None;
        for attempt in 0..=self.config.max_retries {
            match operation() {
                Ok(result) => {
                    if attempt > 0 {
                        debug!("RPC call succeeded on attempt {}", attempt + 1);
                    }
                    return Ok(result);
                }
                Err(error) => {
                    last_error = Some(error);
                    
                    if attempt < self.config.max_retries {
                        let delay = if self.config.exponential_backoff {
                            Duration::from_millis(self.config.retry_delay_ms * 2_u64.pow(attempt))
                        } else {
                            Duration::from_millis(self.config.retry_delay_ms)
                        };
                        
                        warn!(
                            "RPC call failed on attempt {} (will retry in {:?}): {:?}",
                            attempt + 1, delay, last_error
                        );
                        
                        sleep(delay).await;
                    }
                }
            }
        }
        
        let error = last_error.unwrap();
        Err(SolanaAdapterError::RpcClient(error))
    }
    
    /// Gets the latest block slot
    pub async fn get_slot(&mut self) -> Result<u64, SolanaAdapterError> {
        // First enforce rate limit
        self.enforce_rate_limit().await;
        
        // Then execute the operation
        self.client.get_slot()
            .map_err(SolanaAdapterError::RpcClient)
    }
    
    /// Gets transaction data by signature
    pub async fn get_transaction(
        &mut self,
        signature: &Signature,
    ) -> Result<Option<Transaction>, SolanaAdapterError> {
        // First enforce rate limit
        self.enforce_rate_limit().await;
        
        let config = RpcTransactionConfig {
            encoding: None,
            commitment: Some(self.client.commitment()),
            max_supported_transaction_version: None,
        };
        
        match self.client.get_transaction_with_config(signature, config) {
            Ok(tx_response) => {
                if let Some(decoded) = tx_response.transaction.transaction.decode() {
                    Ok(Some(decoded))
                } else {
                    let error = ClientError::new_with_request(
                        ClientErrorKind::Custom("Failed to decode transaction".to_string()),
                        RpcRequest::GetTransaction
                    );
                    Err(SolanaAdapterError::RpcClient(error))
                }
            }
            Err(error) => {
                if error.to_string().contains("not found") {
                    Ok(None)
                } else {
                    Err(SolanaAdapterError::RpcClient(error))
                }
            }
        }
    }
    
    /// Submits a transaction to the network
    pub async fn send_transaction(&mut self, transaction: &Transaction) -> Result<Signature, SolanaAdapterError> {
        // First enforce rate limit
        self.enforce_rate_limit().await;
        
        let config = RpcSendTransactionConfig {
            skip_preflight: false,
            preflight_commitment: Some(self.client.commitment()),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(self.config.max_retries as usize),
            min_context_slot: None,
        };
        
        self.client.send_transaction_with_config(transaction, config)
            .map_err(SolanaAdapterError::RpcClient)
    }
    
    /// Enforces rate limiting between RPC calls
    async fn enforce_rate_limit(&mut self) {
        const MIN_CALL_INTERVAL: Duration = Duration::from_millis(50);
        
        if let Some(last_time) = self.last_call {
            let elapsed = last_time.elapsed();
            if elapsed < MIN_CALL_INTERVAL {
                sleep(MIN_CALL_INTERVAL - elapsed).await;
            }
        }
        self.last_call = Some(Instant::now());
    }
    
    /// Gets the underlying RPC client
    pub fn client(&self) -> &RpcClient {
        &self.client
    }
}
