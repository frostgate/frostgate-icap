mod config;
pub use config::RegistryConfig;

use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::traits::ChainAdapter;
use crate::types::{AdapterError, HealthMetrics, ConnectionStatus};
use frostgate_sdk::frostmessage::ChainId;

/// Registry entry for a chain adapter
#[derive(Debug)]
struct RegistryEntry {
    /// The chain adapter instance
    adapter: Arc<dyn ChainAdapter>,
    /// When the adapter was registered
    registered_at: SystemTime,
    /// Last health check timestamp
    last_health_check: SystemTime,
    /// Health check metrics
    health_metrics: HealthMetrics,
    /// Whether the adapter is enabled
    enabled: bool,
    /// Success rate over last N operations
    success_rate: f64,
    /// Total operations since registration
    total_operations: u64,
}

/// Registry for chain adapters
pub struct AdapterRegistry {
    /// Registered adapters
    adapters: RwLock<HashMap<String, RegistryEntry>>,
    /// Registry configuration
    config: RegistryConfig,
    /// Chain adapter counts
    chain_counts: RwLock<HashMap<ChainId, usize>>,
}

impl AdapterRegistry {
    /// Create a new adapter registry with default configuration
    pub fn new() -> Self {
        Self::with_config(RegistryConfig::default())
    }

    /// Create a new adapter registry with custom configuration
    pub fn with_config(config: RegistryConfig) -> Self {
        Self {
            adapters: RwLock::new(HashMap::new()),
            config,
            chain_counts: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new chain adapter
    pub async fn register_adapter(&self, adapter: Arc<dyn ChainAdapter>) -> Result<(), AdapterError> {
        let adapter_id = adapter.adapter_id();
        let chain_id = adapter.chain_id();

        // Check adapter health before registering
        let health = adapter.health_metrics().await?;
        
        // Check if we've reached the maximum adapters for this chain
        let mut chain_counts = self.chain_counts.write().await;
        let count = chain_counts.entry(chain_id).or_insert(0);
        
        if *count >= self.config.max_adapters_per_chain {
            return Err(AdapterError::ConfigurationError(
                format!("Maximum number of adapters ({}) reached for chain {}", 
                    self.config.max_adapters_per_chain, chain_id)
            ));
        }

        let entry = RegistryEntry {
            adapter: adapter.clone(),
            registered_at: SystemTime::now(),
            last_health_check: SystemTime::now(),
            health_metrics: health,
            enabled: true,
            success_rate: 1.0, // Start with perfect score
            total_operations: 0,
        };

        let mut adapters = self.adapters.write().await;
        
        // Check if we already have this adapter
        if let Some(existing) = adapters.get(&adapter_id) {
            // If existing adapter is healthy and we don't allow multiple, reject
            if !self.config.allow_multiple_adapters && 
               matches!(existing.health_metrics.connection_status, ConnectionStatus::Healthy) {
                return Err(AdapterError::ConfigurationError(
                    format!("Adapter {} for chain {} already registered and healthy", 
                        adapter_id, chain_id)
                ));
            }
        }

        info!("Registering adapter {} for chain {}", adapter_id, chain_id);
        adapters.insert(adapter_id.clone(), entry);
        *count += 1;

        Ok(())
    }

    /// Deregister a chain adapter
    pub async fn deregister_adapter(&self, adapter_id: &str) -> Result<(), AdapterError> {
        let mut adapters = self.adapters.write().await;
        let mut chain_counts = self.chain_counts.write().await;
        
        if let Some(entry) = adapters.remove(adapter_id) {
            let chain_id = entry.adapter.chain_id();
            if let Some(count) = chain_counts.get_mut(&chain_id) {
                *count = count.saturating_sub(1);
            }
            
            info!("Deregistered adapter {} for chain {}", adapter_id, chain_id);
            Ok(())
        } else {
            Err(AdapterError::ConfigurationError(
                format!("Adapter {} not found", adapter_id)
            ))
        }
    }

    /// Get an adapter by chain ID
    pub async fn get_adapter(&self, chain_id: ChainId) -> Result<Arc<dyn ChainAdapter>, AdapterError> {
        let adapters = self.adapters.read().await;
        
        // Find all healthy adapters for this chain
        let healthy_adapters: Vec<_> = adapters.values()
            .filter(|entry| {
                entry.enabled && 
                entry.adapter.chain_id() == chain_id &&
                matches!(entry.health_metrics.connection_status, ConnectionStatus::Healthy) &&
                entry.success_rate >= self.config.min_health_success_rate
            })
            .collect();

        match healthy_adapters.len() {
            0 => Err(AdapterError::ConfigurationError(
                format!("No healthy adapter found for chain {}", chain_id)
            )),
            1 => Ok(healthy_adapters[0].adapter.clone()),
            _ => {
                // If multiple healthy adapters, choose the one with best metrics
                let best_adapter = healthy_adapters.iter()
                    .max_by(|a, b| {
                        // Compare based on success rate first
                        a.success_rate.partial_cmp(&b.success_rate)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            // Then by response time
                            .then_with(|| {
                                b.health_metrics.avg_response_time
                                    .cmp(&a.health_metrics.avg_response_time)
                            })
                    })
                    .unwrap();
                Ok(best_adapter.adapter.clone())
            }
        }
    }

    /// Get all registered adapters
    pub async fn list_adapters(&self) -> Vec<(String, ChainId, ConnectionStatus)> {
        let adapters = self.adapters.read().await;
        adapters.iter()
            .map(|(id, entry)| (
                id.clone(),
                entry.adapter.chain_id(),
                entry.health_metrics.connection_status.clone()
            ))
            .collect()
    }

    /// Run health checks on all adapters
    pub async fn run_health_checks(&self) {
        let mut adapters = self.adapters.write().await;
        let now = SystemTime::now();

        for (adapter_id, entry) in adapters.iter_mut() {
            if let Ok(metrics) = entry.adapter.health_metrics().await {
                // Update success rate
                if entry.total_operations > 0 {
                    entry.success_rate = (metrics.total_operations - metrics.failed_operations) as f64 
                        / metrics.total_operations as f64;
                }
                
                entry.health_metrics = metrics;
                entry.last_health_check = now;
                entry.total_operations = metrics.total_operations;

                // Update enabled status based on health
                match &entry.health_metrics.connection_status {
                    ConnectionStatus::Healthy => {
                        if !entry.enabled && self.config.auto_reenable {
                            info!("Re-enabling healthy adapter {}", adapter_id);
                            entry.enabled = true;
                        }
                    },
                    ConnectionStatus::Unhealthy(reason) => {
                        // Disable if unhealthy for too long
                        if let Ok(duration) = now.duration_since(entry.last_health_check) {
                            if duration > self.config.max_unhealthy_duration && entry.enabled {
                                warn!("Disabling unhealthy adapter {}: {}", adapter_id, reason);
                                entry.enabled = false;
                            }
                        }
                    },
                    ConnectionStatus::Degraded(reason) => {
                        // Check if response time is too high
                        if entry.health_metrics.avg_response_time > self.config.max_response_time {
                            warn!("Adapter {} marked as degraded: {}", adapter_id, reason);
                        }
                    },
                    ConnectionStatus::Unknown => {
                        warn!("Adapter {} has unknown status", adapter_id);
                    }
                }

                // Check success rate
                if entry.success_rate < self.config.min_health_success_rate {
                    warn!(
                        "Adapter {} success rate ({:.2}%) below threshold ({:.2}%)", 
                        adapter_id,
                        entry.success_rate * 100.0,
                        self.config.min_health_success_rate * 100.0
                    );
                }
            } else {
                warn!("Health check failed for adapter {}", adapter_id);
            }
        }
    }

    /// Start the health check background task
    pub async fn start_health_checks(self: Arc<Self>) {
        let interval = self.config.health_check_interval;
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);
            loop {
                interval.tick().await;
                self.run_health_checks().await;
            }
        });
    }

    /// Update registry configuration
    pub fn update_config(&mut self, config: RegistryConfig) {
        self.config = config;
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use frostgate_sdk::frostmessage::{FrostMessage, MessageEvent};

    // Mock adapter for testing
    struct MockAdapter {
        id: String,
        chain_id: ChainId,
        health_status: Arc<RwLock<ConnectionStatus>>,
        call_count: Arc<AtomicU64>,
    }

    impl MockAdapter {
        fn new(id: &str, chain_id: ChainId) -> Self {
            Self {
                id: id.to_string(),
                chain_id,
                health_status: Arc::new(RwLock::new(ConnectionStatus::Healthy)),
                call_count: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    #[async_trait]
    impl ChainAdapter for MockAdapter {
        fn chain_id(&self) -> ChainId {
            self.chain_id
        }

        fn adapter_id(&self) -> String {
            self.id.clone()
        }
    }

    // Implement other required traits...
    
    #[tokio::test]
    async fn test_adapter_registration() {
        let registry = AdapterRegistry::new();
        let adapter = Arc::new(MockAdapter::new("test1", ChainId::Ethereum));
        
        // Test registration
        registry.register_adapter(adapter.clone()).await.unwrap();
        
        // Test retrieval
        let retrieved = registry.get_adapter(ChainId::Ethereum).await.unwrap();
        assert_eq!(retrieved.adapter_id(), "test1");
        
        // Test deregistration
        registry.deregister_adapter("test1").await.unwrap();
        
        // Verify adapter is gone
        assert!(registry.get_adapter(ChainId::Ethereum).await.is_err());
    }

    #[tokio::test]
    async fn test_multiple_adapters() {
        let config = RegistryConfig::default()
            .with_max_adapters(2)
            .with_multiple_adapters(true);
        let registry = AdapterRegistry::with_config(config);

        let adapter1 = Arc::new(MockAdapter::new("test1", ChainId::Ethereum));
        let adapter2 = Arc::new(MockAdapter::new("test2", ChainId::Ethereum));
        
        // Register both adapters
        registry.register_adapter(adapter1.clone()).await.unwrap();
        registry.register_adapter(adapter2.clone()).await.unwrap();
        
        // Try to register a third adapter
        let adapter3 = Arc::new(MockAdapter::new("test3", ChainId::Ethereum));
        assert!(registry.register_adapter(adapter3.clone()).await.is_err());
    }
} 