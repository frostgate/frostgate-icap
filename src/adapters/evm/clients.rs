#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use std::time::Duration;
use std::sync::Arc;
use ethers::prelude::*;
use crate::chainadapter::AdapterError;
use tracing::{debug, error};

/// RPC client for Ethereum chain interaction
pub struct RpcClient {
    /// Provider for Ethereum interactions
    provider: Arc<Provider<Http>>,
}

impl RpcClient {
    /// Create a new RPC client
    pub async fn new(rpc_url: &str, timeout: Duration) -> Result<Self, AdapterError> {
        debug!("Creating RPC client for URL: {}", rpc_url);

        // Create provider with custom settings
        let provider = Provider::<Http>::try_from(rpc_url)
            .map_err(|e| AdapterError::Configuration(format!("Failed to create provider: {}", e)))?
            .interval(Duration::from_millis(50));

        Ok(Self {
            provider: Arc::new(provider),
        })
    }

    /// Get the provider
    pub fn provider(&self) -> Arc<Provider<Http>> {
        self.provider.clone()
    }
}