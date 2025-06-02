#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use std::time::{Duration, Instant};
use ethers::prelude::*;
use reqwest::{Client, Url};
use tokio::time::sleep;
use tracing::{debug, warn};
use crate::chainadapter::AdapterError;
use crate::evm::types::{MAX_RETRY_ATTEMPTS, RETRY_BASE_DELAY, DEFAULT_RPC_TIMEOUT};
use std::sync::Arc;

/// RPC client wrapper with retry and error handling capabilities
#[derive(Debug, Clone)]
pub struct RpcClient {
    provider: Arc<Provider<Http>>,
    min_call_interval: Duration,
    last_call: Option<Instant>,
}

impl RpcClient {
    /// Creates a new RPC client with the given endpoint and timeout
    pub fn new(rpc_url: &str, timeout: Duration) -> Result<Self, AdapterError> {
        let url = reqwest::Url::parse(rpc_url)
            .map_err(|e| AdapterError::Configuration(format!("Invalid RPC URL: {}", e)))?;
            
        let http = Http::new_with_client(
            url,
            Client::builder()
                .timeout(timeout)
                .build()
                .map_err(|e| AdapterError::Configuration(format!("Failed to create HTTP client: {}", e)))?
        );
        
        let provider = Provider::new(http)
            .interval(Duration::from_millis(50));

        Ok(Self {
            provider: Arc::new(provider),
            min_call_interval: Duration::from_millis(100),
            last_call: None,
        })
    }

    /// Executes an RPC call with retry logic and rate limiting
    pub async fn execute<T, F, Fut>(&mut self, mut operation: F) -> Result<T, AdapterError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, ProviderError>>,
    {
        self.enforce_rate_limit().await;
        
        let mut last_error = None;
        for attempt in 0..MAX_RETRY_ATTEMPTS {
            match tokio::time::timeout(DEFAULT_RPC_TIMEOUT, operation()).await {
                Ok(Ok(result)) => {
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!("RPC call failed on attempt {}: {}", attempt + 1, e);
                    last_error = Some(e);
                    
                    if attempt < MAX_RETRY_ATTEMPTS - 1 {
                        let delay = RETRY_BASE_DELAY * 2_u32.pow(attempt);
                        sleep(delay).await;
                    }
                }
                Err(_) => {
                    warn!("RPC call timed out on attempt {}", attempt + 1);
                    return Err(AdapterError::Network("Request timed out".to_string()));
                }
            }
        }
        
        let error_msg = last_error
            .map(|e| format!("RPC call failed after {} attempts: {}", MAX_RETRY_ATTEMPTS, e))
            .unwrap_or_else(|| "RPC call failed after maximum retry attempts".to_string());
        
        Err(AdapterError::Network(error_msg))
    }

    /// Enforces rate limiting between RPC calls
    async fn enforce_rate_limit(&mut self) {
        if let Some(last_time) = self.last_call {
            let elapsed = last_time.elapsed();
            if elapsed < self.min_call_interval {
                let sleep_duration = self.min_call_interval - elapsed;
                sleep(sleep_duration).await;
            }
        }
        self.last_call = Some(Instant::now());
    }

    /// Gets the underlying provider
    pub fn provider(&self) -> Arc<Provider<Http>> {
        Arc::clone(&self.provider)
    }
}

impl std::fmt::Display for RpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RpcClient")
    }
}

// We don't implement Middleware anymore since we're using the provider directly