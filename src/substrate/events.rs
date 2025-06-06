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

#[derive(Clone)]
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
            .await
            .map_err(|e| AdapterError::Network(format!("Failed to get latest block: {}", e)))?
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
        let mut blocks = self.client.blocks()
            .subscribe_finalized()
            .await
            .map_err(|e| AdapterError::Network(format!("Failed to subscribe to finalized blocks: {}", e)))?;
        
        // Process blocks as they come in
        while let Some(block) = blocks.next().await {
            let block = block.map_err(|e| AdapterError::Network(format!("Failed to get block: {}", e)))?;
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

                // Check if event is from our pallet and is a message event
                if event.pallet_name() == self.pallet_name {
                    match event.variant_name() {
                        Some(name) if ["MessageSent", "MessageReceived", "MessageFailed"].contains(&name) => {
                            match self.convert_event_to_message_event(&event, block_number) {
                                Ok(message_event) => {
                                    debug!(
                                        "Found {} event in block {} for message {}",
                                        name,
                                        block_number,
                                        message_event.message.id
                                    );
                                    events.push(message_event);
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to convert {} event in block {}: {}",
                                        name,
                                        block_number,
                                        e
                                    );
                                }
                            }
                        }
                        Some(name) => {
                            debug!("Ignoring non-message event: {}", name);
                        }
                        None => {
                            warn!("Event without variant name in block {}", block_number);
                        }
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

        // Update cache with new events
        let mut cache = self.event_cache.write().await;
        let mut deduplicated_events = Vec::with_capacity(new_events.len());
        
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

    /// Generates a deterministic UUID for a message based on its content
    fn generate_deterministic_uuid(
        &self,
        from_chain: u32,
        to_chain: u32,
        payload: &[u8],
        nonce: u64,
        timestamp: u64,
    ) -> Uuid {
        use blake2::{Blake2b512, Digest};
        
        // Create a unique seed by combining all message fields
        let mut data = Vec::with_capacity(
            8 + // from_chain
            8 + // to_chain
            payload.len() +
            8 + // nonce
            8   // timestamp
        );
        
        // Add all fields in a deterministic order
        data.extend_from_slice(&from_chain.to_be_bytes());
        data.extend_from_slice(&to_chain.to_be_bytes());
        data.extend_from_slice(payload);
        data.extend_from_slice(&nonce.to_be_bytes());
        data.extend_from_slice(&timestamp.to_be_bytes());
        
        // Hash the data
        let mut hasher = Blake2b512::new();
        hasher.update(&data);
        let result = hasher.finalize();
        
        // Use first 16 bytes for UUID
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&result[..16]);
        
        // Set version 5 (SHA1) and variant bits
        bytes[6] = (bytes[6] & 0x0f) | 0x50; // Version 5
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 1
        
        Uuid::from_bytes(bytes)
    }

    /// Converts a Substrate event to a Frostgate message event
    fn convert_event_to_message_event(
        &self,
        event: &subxt::events::EventDetails<SubxtPolkadotConfig>,
        block_number: u32,
    ) -> Result<MessageEvent, AdapterError> {
        let variant_name = event.variant_name()
            .ok_or_else(|| AdapterError::Other("Event variant name not found".to_string()))?;

        match variant_name {
            "MessageSent" => {
                #[derive(scale::Decode)]
                struct MessageSentEvent {
                    from_chain: u32,
                    to_chain: u32,
                    sender: AccountId32,
                    payload: Vec<u8>,
                    nonce: u64,
                    timestamp: u64,
                    fee: Option<u128>,
                }

                let event_data: MessageSentEvent = event.decode()
                    .map_err(|e| AdapterError::Deserialization(format!("Failed to decode MessageSent event: {}", e)))?;

                let message_id = self.generate_deterministic_uuid(
                    event_data.from_chain,
                    event_data.to_chain,
                    &event_data.payload,
                    event_data.nonce,
                    event_data.timestamp,
                );

                let message = FrostMessage {
                    id: message_id,
                    from_chain: ChainId::from(event_data.from_chain),
                    to_chain: ChainId::from(event_data.to_chain),
                    payload: event_data.payload,
                    proof: None,
                    timestamp: event_data.timestamp,
                    nonce: event_data.nonce,
                    signature: None,
                    fee: event_data.fee,
                    metadata: Some(HashMap::from([
                        ("sender".to_string(), event_data.sender.to_string()),
                        ("event_type".to_string(), "MessageSent".to_string()),
                    ])),
                };

                Ok(MessageEvent {
                    message,
                    block_number: Some(block_number as u64),
                    tx_hash: None,
                })
            },

            "MessageReceived" => {
                #[derive(scale::Decode)]
                struct MessageReceivedEvent {
                    from_chain: u32,
                    to_chain: u32,
                    recipient: AccountId32,
                    payload: Vec<u8>,
                    nonce: u64,
                    timestamp: u64,
                }

                let event_data: MessageReceivedEvent = event.decode()
                    .map_err(|e| AdapterError::Deserialization(format!("Failed to decode MessageReceived event: {}", e)))?;

                let message_id = self.generate_deterministic_uuid(
                    event_data.from_chain,
                    event_data.to_chain,
                    &event_data.payload,
                    event_data.nonce,
                    event_data.timestamp,
                );

                let message = FrostMessage {
                    id: message_id,
                    from_chain: ChainId::from(event_data.from_chain),
                    to_chain: ChainId::from(event_data.to_chain),
                    payload: event_data.payload,
                    proof: None,
                    timestamp: event_data.timestamp,
                    nonce: event_data.nonce,
                    signature: None,
                    fee: None,
                    metadata: Some(HashMap::from([
                        ("recipient".to_string(), event_data.recipient.to_string()),
                        ("event_type".to_string(), "MessageReceived".to_string()),
                    ])),
                };

                Ok(MessageEvent {
                    message,
                    block_number: Some(block_number as u64),
                    tx_hash: None,
                })
            },

            "MessageFailed" => {
                #[derive(scale::Decode)]
                struct MessageFailedEvent {
                    from_chain: u32,
                    to_chain: u32,
                    payload: Vec<u8>,
                    error: String,
                    timestamp: u64,
                }

                let event_data: MessageFailedEvent = event.decode()
                    .map_err(|e| AdapterError::Deserialization(format!("Failed to decode MessageFailed event: {}", e)))?;

                let message_id = self.generate_deterministic_uuid(
                    event_data.from_chain,
                    event_data.to_chain,
                    &event_data.payload,
                    0, // Failed messages don't have a nonce
                    event_data.timestamp,
                );

                let message = FrostMessage {
                    id: message_id,
                    from_chain: ChainId::from(event_data.from_chain),
                    to_chain: ChainId::from(event_data.to_chain),
                    payload: event_data.payload,
                    proof: None,
                    timestamp: event_data.timestamp,
                    nonce: 0,
                    signature: None,
                    fee: None,
                    metadata: Some(HashMap::from([
                        ("error".to_string(), event_data.error),
                        ("event_type".to_string(), "MessageFailed".to_string()),
                    ])),
                };

                Ok(MessageEvent {
                    message,
                    block_number: Some(block_number as u64),
                    tx_hash: None,
                })
            },

            _ => Err(AdapterError::Other(format!("Unsupported event variant: {}", variant_name))),
        }
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
