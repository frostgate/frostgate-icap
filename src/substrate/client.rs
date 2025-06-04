#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]


use std::time::{Duration, Instant};
use subxt::{OnlineClient, PolkadotConfig as SubxtPolkadotConfig, error::Error as SubxtError};
use tokio::time::sleep;
use tokio::sync::Semaphore;
use tracing::{debug, warn};
use std::sync::Arc;
use crate::chainadapter::AdapterError;
use crate::substrate::types::{
    MAX_RETRY_ATTEMPTS, RETRY_BASE_DELAY, MAX_RETRY_DELAY,
    DEFAULT_RPC_TIMEOUT, DEFAULT_MIN_CALL_INTERVAL,
};

#[derive(Clone)]
pub struct RpcClient {
    client: OnlineClient<SubxtPolkadotConfig>,
    min_call_interval: Duration,
    last_call: Option<Instant>,
    rpc_semaphore: Arc<Semaphore>,
}

impl RpcClient {
    /// Creates a new RPC client with the given endpoint and timeout
    pub async fn new(rpc_url: &str, timeout: Duration, max_concurrent: usize) -> Result<Self, AdapterError> {
        let client = tokio::time::timeout(
            timeout,
            OnlineClient::<SubxtPolkadotConfig>::from_url(rpc_url)
        ).await
            .map_err(|_| AdapterError::Network(format!("Connection timeout to {}", rpc_url)))?
            .map_err(|e| AdapterError::Network(format!("Failed to connect to {}: {}", rpc_url, e)))?;

        debug!("Successfully connected to {}", rpc_url);

        Ok(Self {
            client,
            min_call_interval: DEFAULT_MIN_CALL_INTERVAL,
            last_call: None,
            rpc_semaphore: Arc::new(Semaphore::new(max_concurrent)),
        })
    }

    /// Executes an RPC call with retry logic and rate limiting
    pub async fn execute<T, F, Fut>(&mut self, mut operation: F) -> Result<T, AdapterError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, SubxtError>>,
    {
        // First enforce rate limit before acquiring semaphore
        self.enforce_rate_limit().await;
        
        // Now acquire the semaphore
        let _permit = self.rpc_semaphore.acquire().await;
        
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

    /// Gets the underlying client
    pub fn client(&self) -> &OnlineClient<SubxtPolkadotConfig> {
        &self.client
    }

    /// Sets the minimum interval between calls
    pub fn set_min_call_interval(&mut self, interval: Duration) {
        self.min_call_interval = interval;
    }
}
