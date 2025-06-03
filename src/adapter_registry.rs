#![allow(async_fn_in_trait)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_imports)]

use crate::chainadapter::{
    AdapterError, ChainAdapter,
};
use ethers::core::k256::elliptic_curve::Error;
use frostgate_sdk::frostmessage::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::fmt;
use subxt::config::substrate::H256;

/// Type alias for a dynamic chain adapter with generic block and transaction IDs
pub type DynChainAdapter = dyn ChainAdapter<
    Error = AdapterError,
    BlockId = String,
    TxId = String
> + Send + Sync;

/// Errors that can occur when working with the adapter registry
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterRegistryError {
    /// Adapter for the specified chain ID already exists
    AdapterAlreadyExists(ChainId),
    /// No adapter found for the specified chain ID
    AdapterNotFound(ChainId),
    /// Failed to acquire lock (should be rare in practice)
    LockError(String),
}

impl fmt::Display for AdapterRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AdapterAlreadyExists(chain_id) => {
                write!(f, "Adapter for chain ID {:?} already exists", chain_id)
            }
            Self::AdapterNotFound(chain_id) => {
                write!(f, "No adapter found for chain ID {:?}", chain_id)
            }
            Self::LockError(msg) => write!(f, "Lock error: {}", msg),
        }
    }
}

impl std::error::Error for AdapterRegistryError {}

/// Thread-safe registry for managing chain adapters
pub struct AdapterRegistry {
    adapters: RwLock<HashMap<ChainId, Arc<DynChainAdapter>>>,
}

impl AdapterRegistry {
    /// Creates a new empty adapter registry
    pub fn new() -> Self {
        Self {
            adapters: RwLock::new(HashMap::new()),
        }
    }

    /// Registers a new adapter for the specified chain ID
    /// 
    /// Returns an error if an adapter for this chain ID already exists
    pub async fn register_adapter(
        &self,
        chain_id: ChainId,
        adapter: Arc<DynChainAdapter>,
    ) -> Result<(), AdapterRegistryError> {
        let mut adapters = self.adapters.write().await;

        if adapters.contains_key(&chain_id) {
            return Err(AdapterRegistryError::AdapterAlreadyExists(chain_id));
        }

        adapters.insert(chain_id, adapter);
        Ok(())
    }

    /// Registers or updates an adapter for the specified chain ID
    /// 
    /// If an adapter already exists for this chain ID, it will be replaced
    pub async fn register_or_update_adapter(
        &self,
        chain_id: ChainId,
        adapter: Arc<DynChainAdapter>,
    ) -> Result<Option<Arc<DynChainAdapter>>, AdapterRegistryError> {
        let mut adapters = self.adapters.write().await;
        Ok(adapters.insert(chain_id, adapter))
    }

    /// Retrieves an adapter for the specified chain ID
    pub async fn get_adapter(&self, chain_id: &ChainId) -> Result<Arc<DynChainAdapter>, AdapterRegistryError> {
        let adapters = self.adapters.read().await;
        adapters.get(chain_id)
            .cloned()
            .ok_or_else(|| AdapterRegistryError::AdapterNotFound(*chain_id))
    }

    /// Checks if an adapter exists for the specified chain ID
    pub async fn has_adapter(&self, chain_id: &ChainId) -> Result<bool, AdapterRegistryError> {
        let adapters = self.adapters.read().await;
        Ok(adapters.contains_key(chain_id))
    }

    /// Removes an adapter for the specified chain ID
    /// 
    /// Returns the removed adapter if it existed
    pub async fn remove_adapter(&self, chain_id: &ChainId) -> Result<Option<Arc<DynChainAdapter>>, AdapterRegistryError> {
        let mut adapters = self.adapters.write().await;
        Ok(adapters.remove(chain_id))
    }

    /// Returns a list of all registered chain IDs
    pub async fn list_chain_ids(&self) -> Result<Vec<ChainId>, AdapterRegistryError> {
        let adapters = self.adapters.read().await;
        Ok(adapters.keys().copied().collect())
    }

    /// Returns the number of registered adapters
    pub async fn count(&self) -> Result<usize, AdapterRegistryError> {
        let adapters = self.adapters.read().await;
        Ok(adapters.len())
    }

    /// Clears all registered adapters
    pub async fn clear(&self) -> Result<(), AdapterRegistryError> {
        let mut adapters = self.adapters.write().await;
        adapters.clear();
        Ok(())
    }

    /// Executes a closure with read access to all adapters
    /// 
    /// This is useful for operations that need to work with multiple adapters atomically
    pub async fn with_adapters<F, R>(&self, f: F) -> Result<R, AdapterRegistryError>
    where
        F: FnOnce(&HashMap<ChainId, Arc<DynChainAdapter>>) -> R,
    {
        let adapters = self.adapters.read().await;
        Ok(f(&*adapters))
    }

    /// Bulk register multiple adapters
    /// 
    /// If any registration fails, all previous registrations in this batch are rolled back
    pub async fn bulk_register(&self, adapters: Vec<(ChainId, Arc<DynChainAdapter>)>) -> Result<(), AdapterRegistryError> {
        let mut registry = self.adapters.write().await;

        // Check for conflicts first
        for (chain_id, _) in &adapters {
            if registry.contains_key(chain_id) {
                return Err(AdapterRegistryError::AdapterAlreadyExists(*chain_id));
            }
        }

        // If no conflicts, register all adapters
        for (chain_id, adapter) in adapters {
            registry.insert(chain_id, adapter);
        }

        Ok(())
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AdapterRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdapterRegistry")
            .field("adapters", &"<opaque>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use uuid::Uuid;

    // Mock adapter for testing
    struct MockAdapter {
        chain_id: ChainId,
    }

    #[async_trait]
    impl ChainAdapter for MockAdapter {
        type BlockId = String;
        type TxId = String;
        type Error = AdapterError;

        async fn latest_block(&self) -> Result<Self::BlockId, Self::Error> {
            Ok("0".to_string())
        }

        async fn get_transaction(&self, _tx: &Self::TxId) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(None)
        }

        async fn wait_for_finality(&self, _block: &Self::BlockId) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn submit_message(&self, _msg: &FrostMessage) -> Result<Self::TxId, Self::Error> {
            Ok("0x0".to_string())
        }

        async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, Self::Error> {
            Ok(vec![])
        }

        async fn verify_on_chain(&self, _msg: &FrostMessage) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn estimate_fee(&self, _msg: &FrostMessage) -> Result<u128, Self::Error> {
            Ok(0)
        }

        async fn message_status(&self, _id: &Uuid) -> Result<MessageStatus, Self::Error> {
            Ok(MessageStatus::Pending)
        }

        async fn health_check(&self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_register_and_get_adapter() {
        let registry = AdapterRegistry::new();
        let adapter = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });

        registry.register_adapter(ChainId::Ethereum, adapter.clone()).await.unwrap();
        let retrieved = registry.get_adapter(&ChainId::Ethereum).await.unwrap();
        assert!(Arc::ptr_eq(&adapter, &retrieved));
    }

    #[tokio::test]
    async fn test_duplicate_registration_fails() {
        let registry = AdapterRegistry::new();
        let adapter = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });

        registry.register_adapter(ChainId::Ethereum, adapter.clone()).await.unwrap();
        let result = registry.register_adapter(ChainId::Ethereum, adapter.clone()).await;
        assert!(matches!(result, Err(AdapterRegistryError::AdapterAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_register_or_update() {
        let registry = AdapterRegistry::new();
        let adapter1 = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });
        let adapter2 = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });

        // First registration
        registry.register_or_update_adapter(ChainId::Ethereum, adapter1.clone()).await.unwrap();
        let retrieved = registry.get_adapter(&ChainId::Ethereum).await.unwrap();
        assert!(Arc::ptr_eq(&adapter1, &retrieved));

        // Update
        registry.register_or_update_adapter(ChainId::Ethereum, adapter2.clone()).await.unwrap();
        let retrieved = registry.get_adapter(&ChainId::Ethereum).await.unwrap();
        assert!(Arc::ptr_eq(&adapter2, &retrieved));
    }

    #[tokio::test]
    async fn test_nonexistent_adapter() {
        let registry = AdapterRegistry::new();
        let result = registry.get_adapter(&ChainId::Ethereum).await;
        assert!(matches!(result, Err(AdapterRegistryError::AdapterNotFound(_))));
    }

    #[tokio::test]
    async fn test_list_chain_ids() {
        let registry = AdapterRegistry::new();
        let adapter1 = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });
        let adapter2 = Arc::new(MockAdapter { chain_id: ChainId::Solana });

        registry.register_adapter(ChainId::Ethereum, adapter1).await.unwrap();
        registry.register_adapter(ChainId::Solana, adapter2).await.unwrap();

        let chain_ids = registry.list_chain_ids().await.unwrap();
        assert_eq!(chain_ids.len(), 2);
        assert!(chain_ids.contains(&ChainId::Ethereum));
        assert!(chain_ids.contains(&ChainId::Solana));
    }

    #[tokio::test]
    async fn test_bulk_register() {
        let registry = AdapterRegistry::new();
        let adapter1 = Arc::new(MockAdapter { chain_id: ChainId::Ethereum });
        let adapter2 = Arc::new(MockAdapter { chain_id: ChainId::Solana });

        let adapters = vec![
            (ChainId::Ethereum, adapter1.clone()),
            (ChainId::Solana, adapter2.clone()),
        ];

        registry.bulk_register(adapters).await.unwrap();

        assert!(registry.has_adapter(&ChainId::Ethereum).await.unwrap());
        assert!(registry.has_adapter(&ChainId::Solana).await.unwrap());
    }
}