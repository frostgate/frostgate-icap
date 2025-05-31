#![allow(async_fn_in_trait)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_imports)]

use crate::chainadapter::{
    AdapterError, ChainAdapter,
};
use frostgate_sdk::frostmessage::*;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::fmt;

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
    adapters: RwLock<HashMap<ChainId, Arc<dyn ChainAdapter>>>,
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
    pub fn register_adapter(
        &self,
        chain_id: ChainId,
        adapter: Arc<dyn ChainAdapter>,
    ) -> Result<(), AdapterRegistryError> {
        let mut adapters = self.adapters.write()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        if adapters.contains_key(&chain_id) {
            return Err(AdapterRegistryError::AdapterAlreadyExists(chain_id));
        }

        adapters.insert(chain_id, adapter);
        Ok(())
    }

    /// Registers or updates an adapter for the specified chain ID
    /// 
    /// If an adapter already exists for this chain ID, it will be replaced
    pub fn register_or_update_adapter(
        &self,
        chain_id: ChainId,
        adapter: Arc<dyn ChainAdapter>,
    ) -> Result<Option<Arc<dyn ChainAdapter>>, AdapterRegistryError> {
        let mut adapters = self.adapters.write()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(adapters.insert(chain_id, adapter))
    }

    /// Retrieves an adapter for the specified chain ID
    pub fn get_adapter(&self, chain_id: &ChainId) -> Result<Arc<dyn ChainAdapter>, AdapterRegistryError> {
        let adapters = self.adapters.read()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        adapters.get(chain_id)
            .cloned()
            .ok_or_else(|| AdapterRegistryError::AdapterNotFound(*chain_id))
    }

    /// Checks if an adapter exists for the specified chain ID
    pub fn has_adapter(&self, chain_id: &ChainId) -> Result<bool, AdapterRegistryError> {
        let adapters = self.adapters.read()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(adapters.contains_key(chain_id))
    }

    /// Removes an adapter for the specified chain ID
    /// 
    /// Returns the removed adapter if it existed
    pub fn remove_adapter(&self, chain_id: &ChainId) -> Result<Option<Arc<dyn ChainAdapter>>, AdapterRegistryError> {
        let mut adapters = self.adapters.write()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(adapters.remove(chain_id))
    }

    /// Returns a list of all registered chain IDs
    pub fn list_chain_ids(&self) -> Result<Vec<ChainId>, AdapterRegistryError> {
        let adapters = self.adapters.read()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(adapters.keys().copied().collect())
    }

    /// Returns the number of registered adapters
    pub fn count(&self) -> Result<usize, AdapterRegistryError> {
        let adapters = self.adapters.read()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(adapters.len())
    }

    /// Clears all registered adapters
    pub fn clear(&self) -> Result<(), AdapterRegistryError> {
        let mut adapters = self.adapters.write()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        adapters.clear();
        Ok(())
    }

    /// Executes a closure with read access to all adapters
    /// 
    /// This is useful for operations that need to work with multiple adapters atomically
    pub fn with_adapters<F, R>(&self, f: F) -> Result<R, AdapterRegistryError>
    where
        F: FnOnce(&HashMap<ChainId, Arc<dyn ChainAdapter>>) -> R,
    {
        let adapters = self.adapters.read()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

        Ok(f(&*adapters))
    }

    /// Bulk register multiple adapters
    /// 
    /// If any registration fails, all previous registrations in this batch are rolled back
    pub fn bulk_register(&self, adapters: Vec<(ChainId, Arc<dyn ChainAdapter>)>) -> Result<(), AdapterRegistryError> {
        let mut registry = self.adapters.write()
            .map_err(|e| AdapterRegistryError::LockError(e.to_string()))?;

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

// Thread-safe clone implementation
impl Clone for AdapterRegistry {
    fn clone(&self) -> Self {
        let adapters = self.adapters.read().expect("Failed to acquire read lock for cloning");
        Self {
            adapters: RwLock::new(adapters.clone()),
        }
    }
}

impl fmt::Debug for AdapterRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.adapters.read() {
            Ok(adapters) => {
                f.debug_struct("AdapterRegistry")
                    .field("chain_ids", &adapters.keys().collect::<Vec<_>>())
                    .field("count", &adapters.len())
                    .finish()
            }
            Err(_) => {
                f.debug_struct("AdapterRegistry")
                    .field("error", &"Failed to acquire read lock")
                    .finish()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock adapter for testing
    struct MockAdapter {
        chain_id: ChainId,
    }

    impl ChainAdapter for MockAdapter {
        // Implement required methods based on your ChainAdapter trait
        // This is just a placeholder - you'll need to implement actual methods
    }

    #[test]
    fn test_register_and_get_adapter() {
        let registry = AdapterRegistry::new();
        let chain_id = ChainId::from(1u64);
        let adapter = Arc::new(MockAdapter { chain_id });

        // Register adapter
        assert!(registry.register_adapter(chain_id, adapter.clone()).is_ok());

        // Retrieve adapter
        let retrieved = registry.get_adapter(&chain_id).unwrap();
        assert_eq!(Arc::as_ptr(&adapter), Arc::as_ptr(&retrieved));
    }

    #[test]
    fn test_duplicate_registration_fails() {
        let registry = AdapterRegistry::new();
        let chain_id = ChainId::from(1u64);
        let adapter1 = Arc::new(MockAdapter { chain_id });
        let adapter2 = Arc::new(MockAdapter { chain_id });

        // First registration should succeed
        assert!(registry.register_adapter(chain_id, adapter1).is_ok());

        // Second registration should fail
        let result = registry.register_adapter(chain_id, adapter2);
        assert!(matches!(result, Err(AdapterRegistryError::AdapterAlreadyExists(_))));
    }

    #[test]
    fn test_register_or_update() {
        let registry = AdapterRegistry::new();
        let chain_id = ChainId::from(1u64);
        let adapter1 = Arc::new(MockAdapter { chain_id });
        let adapter2 = Arc::new(MockAdapter { chain_id });

        // First registration
        let result = registry.register_or_update_adapter(chain_id, adapter1.clone()).unwrap();
        assert!(result.is_none());

        // Update registration
        let result = registry.register_or_update_adapter(chain_id, adapter2.clone()).unwrap();
        assert!(result.is_some());

        // Verify new adapter is active
        let retrieved = registry.get_adapter(&chain_id).unwrap();
        assert_eq!(Arc::as_ptr(&adapter2), Arc::as_ptr(&retrieved));
    }

    #[test]
    fn test_nonexistent_adapter() {
        let registry = AdapterRegistry::new();
        let chain_id = ChainId::from(999u64);

        let result = registry.get_adapter(&chain_id);
        assert!(matches!(result, Err(AdapterRegistryError::AdapterNotFound(_))));
    }

    #[test]
    fn test_list_chain_ids() {
        let registry = AdapterRegistry::new();
        let chain_ids = vec![ChainId::from(1u64), ChainId::from(2u64), ChainId::from(3u64)];

        for &chain_id in &chain_ids {
            let adapter = Arc::new(MockAdapter { chain_id });
            registry.register_adapter(chain_id, adapter).unwrap();
        }

        let mut listed_ids = registry.list_chain_ids().unwrap();
        listed_ids.sort();
        let mut expected_ids = chain_ids.clone();
        expected_ids.sort();

        assert_eq!(listed_ids, expected_ids);
    }

    #[test]
    fn test_bulk_register() {
        let registry = AdapterRegistry::new();
        let adapters = vec![
            (ChainId::from(1u64), Arc::new(MockAdapter { chain_id: ChainId::from(1u64) }) as Arc<dyn ChainAdapter>),
            (ChainId::from(2u64), Arc::new(MockAdapter { chain_id: ChainId::from(2u64) }) as Arc<dyn ChainAdapter>),
        ];

        assert!(registry.bulk_register(adapters).is_ok());
        assert_eq!(registry.count().unwrap(), 2);
    }
}