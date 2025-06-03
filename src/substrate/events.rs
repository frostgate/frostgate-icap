#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use subxt::{
    OnlineClient,
    PolkadotConfig as SubxtPolkadotConfig,
    events::{EventsClient, Events},
    blocks::{Block, BlockRef},
    utils::{AccountId32, H256},
    blocks::BlocksClient,
};
use frostgate_sdk::frostmessage::{MessageEvent, FrostMessage, ChainId};
use crate::chainadapter::AdapterError;
use tracing::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use uuid::Uuid;
use std::time::{SystemTime, UNIX_EPOCH};
use futures::StreamExt;

const MAX_BLOCKS_PER_QUERY: u32 = 100;

/// Event handler for Substrate chain events
pub struct EventHandler {
    /// Client for chain interaction
    client: Arc<OnlineClient<SubxtPolkadotConfig>>,
    
    /// Pallet name to monitor for events
    pallet_name: String,
    
    /// Last processed block
    last_block: Arc<RwLock<Option<u32>>>,
    
    /// Event cache to deduplicate events
    event_cache: Arc<RwLock<HashMap<H256, MessageEvent>>>,
}

impl EventHandler {
    /// Creates a new event handler
    pub fn new(client: Arc<OnlineClient<SubxtPolkadotConfig>>, pallet_name: String) -> Self {
        Self {
            client,
            pallet_name,
            last_block: Arc::new(RwLock::new(None)),
            event_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Listens for contract events
    pub async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        debug!("Listening for events from pallet: {}", self.pallet_name);

        // Get current block
        let current_block = self.client.blocks()
            .at_latest()
            .await?
            .number();

        // Get last processed block
        let mut last_block = self.last_block.write().await;
        let from_block = last_block.unwrap_or(current_block.saturating_sub(MAX_BLOCKS_PER_QUERY));

        // Don't process more than MAX_BLOCKS_PER_QUERY blocks at once
        let to_block = std::cmp::min(
            current_block,
            from_block.saturating_add(MAX_BLOCKS_PER_QUERY)
        );

        debug!(
            "Scanning blocks {} to {} for events",
            from_block, to_block
        );

        let mut events = Vec::new();
        
        // Subscribe to finalized blocks
        let mut blocks = self.client.blocks().subscribe_finalized().await?;
        
        // Process blocks as they come in
        while let Some(block) = blocks.next().await {
            let block = block?;
            let block_number = block.number();
            
            // Skip blocks outside our range
            if block_number < from_block || block_number > to_block {
                continue;
            }

            // Get events for this block
            let block_events = match block.events().await {
                Ok(events) => events,
                Err(e) => {
                    warn!("Failed to get events for block {}: {}", block_number, e);
                    continue;
                }
            };

            // Process events in this block
            for event in block_events.iter() {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        warn!("Failed to decode event: {}", e);
                        continue;
                    }
                };

                // Check if event is from our pallet
                if event.pallet_name() == self.pallet_name {
                    // Convert event details directly
                    if let Ok(message_event) = self.convert_event_to_message_event(&event, block_number) {
                        events.push(message_event);
                    }
                }
            }

            // Break if we've processed all blocks in our range
            if block_number >= to_block {
                break;
            }
        }

        // Update last processed block
        *last_block = Some(to_block);

        // Deduplicate events using cache
        let mut new_events = Vec::new();
        {
            let cache = self.event_cache.read().await;
            for event in events {
                let event_hash = self.compute_event_hash(&event);
                if !cache.contains_key(&event_hash) {
                    new_events.push((event_hash, event));
                }
            }
        }

        // Now update cache with new events
        let mut cache = self.event_cache.write().await;
        let mut deduplicated_events = Vec::new();
        for (hash, event) in new_events {
            cache.insert(hash, event.clone());
            deduplicated_events.push(event);
        }

        // Cleanup old cache entries
        self.cleanup_event_cache(&mut cache).await;

        debug!("Retrieved {} unique events", deduplicated_events.len());
        Ok(deduplicated_events)
    }

    /// Verifies a message proof on-chain
    pub async fn verify_proof(&self, event: &MessageEvent) -> Result<(), AdapterError> {
        info!("Verifying event proof for message {:?}", event);

        // TODO: Implement actual on-chain proof verification
        // This would involve:
        // 1. Calling the pallet's verify function
        // 2. Checking the verification result
        // 3. Handling any errors or invalid proofs

        Ok(())
    }

    /// Converts a Substrate event to a Frostgate message event
    fn convert_event_to_message_event(
        &self,
        event: &subxt::events::EventDetails<SubxtPolkadotConfig>,
        block_number: u32,
    ) -> Result<MessageEvent, AdapterError> {
        // Expected event fields:
        // - from_chain: u32
        // - to_chain: u32
        // - sender: AccountId32
        // - payload: Vec<u8>
        // - nonce: u64
        // - timestamp: u64

        // TODO: Replace with actual field extraction once we have the event structure
        // For now, create a mock event
        let mock_message = FrostMessage {
            id: Uuid::new_v4(),
            from_chain: ChainId::Polkadot,
            to_chain: ChainId::Ethereum,
            payload: vec![],
            proof: None,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            nonce: 0,
            signature: None,
            fee: Some(0),
            metadata: Some(HashMap::new()),
        };

        Ok(MessageEvent {
            message: mock_message,
            block_number: Some(block_number as u64),
            tx_hash: None,
        })
    }

    /// Compute a unique hash for an event for deduplication
    fn compute_event_hash(&self, event: &MessageEvent) -> H256 {
        // Create a unique hash based on:
        // - Message ID
        // - Block number
        // - Transaction hash (if available)
        
        let mut data = Vec::new();
        
        // Add message ID bytes
        data.extend_from_slice(event.message.id.as_bytes());
        
        // Add block number
        if let Some(block) = event.block_number {
            data.extend_from_slice(&block.to_be_bytes());
        }
        
        // Add transaction hash if available
        if let Some(ref tx_hash) = event.tx_hash {
            data.extend(tx_hash.iter());
        }
        
        // Compute Blake2 hash
        use blake2::{Blake2b512, Digest};
        let mut hasher = Blake2b512::new();
        hasher.update(&data);
        let result = hasher.finalize();
        
        // Convert to H256
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result[..32]);
        H256(hash)
    }

    /// Cleanup old entries from the event cache
    async fn cleanup_event_cache(&self, cache: &mut HashMap<H256, MessageEvent>) {
        const MAX_CACHE_AGE: u64 = 3600; // 1 hour
        const MAX_CACHE_SIZE: usize = 10000;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Remove old entries
        cache.retain(|_, event| {
            let event_time = event.message.timestamp;
            now.saturating_sub(event_time) < MAX_CACHE_AGE
        });

        // If still too large, remove oldest entries
        if cache.len() > MAX_CACHE_SIZE {
            // Create a temporary vector of entries
            let mut entries: Vec<_> = cache.drain().collect();
            entries.sort_by_key(|(_, event)| event.message.timestamp);
            entries.truncate(MAX_CACHE_SIZE);
            
            // Reinsert the kept entries
            for (hash, event) in entries {
                cache.insert(hash, event);
            }
        }
    }
}
