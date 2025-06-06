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
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use futures::StreamExt;
use parity_scale_codec::Decode;
use std::convert::TryFrom;
use frostgate_circuits::sp1::{Sp1Plug, Sp1PlugConfig, types::Sp1ProofType};
use blake2::Digest;

const MAX_BLOCKS_PER_QUERY: u32 = 100;
const VERIFIER_CACHE_TTL: u64 = 3600; // 1 hour TTL for verifier cache
const MAX_VERIFIER_CACHE_SIZE: usize = 1000;

/// Cache entry for verification keys
#[derive(Clone)]
struct VerifierCacheEntry {
    program_info: ProgramInfo,
    last_used: SystemTime,
    use_count: u64,
}

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

    /// Cache for verification keys
    verifier_cache: Arc<RwLock<HashMap<H256, VerifierCacheEntry>>>,

    /// SP1 plug instance for verification
    sp1_plug: Arc<Sp1Plug>,
}

impl EventHandler {
    /// Creates a new event handler
    pub fn new(client: Arc<OnlineClient<SubxtPolkadotConfig>>, pallet_name: String) -> Self {
        // Initialize SP1 plug with default config
        let config = Sp1PlugConfig::default();
        let sp1_plug = Arc::new(Sp1Plug::new(config));

        Self {
            client,
            pallet_name,
            last_block: Arc::new(RwLock::new(None)),
            event_cache: Arc::new(RwLock::new(HashMap::new())),
            verifier_cache: Arc::new(RwLock::new(HashMap::new())),
            sp1_plug,
        }
    }

    /// Get or create verifier for a message
    async fn get_or_create_verifier(&self, message: &FrostMessage) -> Result<ProgramInfo, AdapterError> {
        // Create a unique hash for the message type/format
        let mut hasher = blake2::Blake2b512::new();
        hasher.update(&message.from_chain.to_u64().to_be_bytes());
        hasher.update(&message.to_chain.to_u64().to_be_bytes());
        if let Some(ref proof) = message.proof {
            hasher.update(&bincode::serialize(proof).map_err(|e| AdapterError::Serialization(e.to_string()))?);
        }
        let hash = H256::from_slice(&hasher.finalize()[..32]);

        // Try to get from cache first
        let mut cache = self.verifier_cache.write().await;
        
        if let Some(entry) = cache.get_mut(&hash) {
            // Update usage stats
            entry.last_used = SystemTime::now();
            entry.use_count += 1;
            return Ok(entry.program_info.clone());
        }

        // Not in cache, create new verifier
        let program_info = self.create_verifier(message).await?;

        // Add to cache
        cache.insert(hash, VerifierCacheEntry {
            program_info: program_info.clone(),
            last_used: SystemTime::now(),
            use_count: 1,
        });

        // Cleanup old entries if needed
        self.cleanup_verifier_cache(&mut cache).await;

        Ok(program_info)
    }

    /// Create a new verifier for a message
    async fn create_verifier(&self, message: &FrostMessage) -> Result<ProgramInfo, AdapterError> {
        // This would normally compile the verification circuit
        // For now, we'll use a default program that can verify our message format
        let program_bytes = include_bytes!("../../../frostgate-circuits/programs/message_verifier.sp1");
        
        // Setup the program in SP1
        let mut programs = self.sp1_plug.programs.write().await;
        let program_hash = frostgate_circuits::sp1::prover::setup_program(
            &self.sp1_plug.backend,
            &mut programs,
            program_bytes,
        ).await.map_err(|e| AdapterError::Other(format!("Failed to setup verifier: {}", e)))?;

        // Get the program info
        let program_info = programs.entries()
            .get(&program_hash)
            .ok_or_else(|| AdapterError::Other("Program not found after setup".to_string()))?
            .clone();

        Ok(program_info)
    }

    /// Cleanup old entries from the verifier cache
    async fn cleanup_verifier_cache(&self, cache: &mut HashMap<H256, VerifierCacheEntry>) {
        let now = SystemTime::now();

        // Remove expired entries
        cache.retain(|_, entry| {
            if let Ok(age) = now.duration_since(entry.last_used) {
                age.as_secs() < VERIFIER_CACHE_TTL
            } else {
                false
            }
        });

        // If still too large, remove least used entries
        if cache.len() > MAX_VERIFIER_CACHE_SIZE {
            let mut entries: Vec<_> = cache.drain().collect();
            entries.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.use_count));
            entries.truncate(MAX_VERIFIER_CACHE_SIZE);
            
            for (hash, entry) in entries {
                cache.insert(hash, entry);
            }
        }
    }

    /// Verifies a message proof on-chain
    pub async fn verify_proof(&self, event: &MessageEvent) -> Result<(), AdapterError> {
        info!("Verifying event proof for message {:?}", event);

        // Get the proof from the message
        let proof = event.message.proof.as_ref()
            .ok_or_else(|| AdapterError::Other("Message has no attached proof".to_string()))?;

        // Convert the proof to SP1 format
        let sp1_proof = match bincode::deserialize::<frostgate_circuits::sp1::types::Sp1ProofType>(&proof.proof) {
            Ok(p) => p,
            Err(e) => return Err(AdapterError::Deserialization(format!("Failed to deserialize SP1 proof: {}", e))),
        };

        // Get or create verifier
        let program_info = self.get_or_create_verifier(&event.message).await?;

        // Serialize message data for verification
        let mut data = Vec::new();
        data.extend_from_slice(&event.message.from_chain.to_u64().to_be_bytes());
        data.extend_from_slice(&event.message.to_chain.to_u64().to_be_bytes());
        data.extend_from_slice(&event.message.payload);
        data.extend_from_slice(&event.message.nonce.to_be_bytes());
        data.extend_from_slice(&event.message.timestamp.to_be_bytes());

        // Verify the proof using cached verifier
        let is_valid = frostgate_circuits::sp1::verifier::verify_proof(
            &self.sp1_plug.backend,
            &sp1_proof,
            &program_info.verifying_key,
        ).await.map_err(|e| AdapterError::Other(format!("Proof verification failed: {}", e)))?;

        if !is_valid {
            return Err(AdapterError::Other("Invalid proof".to_string()));
        }

        info!("Successfully verified proof for message {}", event.message.id);
        Ok(())
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
                    let variant_name = event.variant_name();
                    if ["MessageSent", "MessageReceived", "MessageFailed"].contains(&variant_name) {
                        match self.convert_event_to_message_event(&event, block_number) {
                            Ok(message_event) => {
                                debug!(
                                    "Found {} event in block {} for message {}",
                                    variant_name,
                                    block_number,
                                    message_event.message.id
                                );
                                events.push(message_event);
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to convert {} event in block {}: {}",
                                    variant_name,
                                    block_number,
                                    e
                                );
                            }
                        }
                    } else {
                        debug!("Ignoring non-message event: {}", variant_name);
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
        let variant_name = event.variant_name();

        match variant_name {
            "MessageSent" => {
                #[derive(Decode)]
                struct MessageSentEvent {
                    from_chain: u32,
                    to_chain: u32,
                    sender: AccountId32,
                    payload: Vec<u8>,
                    nonce: u64,
                    timestamp: u64,
                    fee: Option<u128>,
                }

                let event_data: MessageSentEvent = Decode::decode(&mut &event.bytes()[..])
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
                    from_chain: ChainId::try_from(event_data.from_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid from_chain id: {}", event_data.from_chain)))?,
                    to_chain: ChainId::try_from(event_data.to_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid to_chain id: {}", event_data.to_chain)))?,
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
                #[derive(Decode)]
                struct MessageReceivedEvent {
                    from_chain: u32,
                    to_chain: u32,
                    recipient: AccountId32,
                    payload: Vec<u8>,
                    nonce: u64,
                    timestamp: u64,
                }

                let event_data: MessageReceivedEvent = Decode::decode(&mut &event.bytes()[..])
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
                    from_chain: ChainId::try_from(event_data.from_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid from_chain id: {}", event_data.from_chain)))?,
                    to_chain: ChainId::try_from(event_data.to_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid to_chain id: {}", event_data.to_chain)))?,
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
                #[derive(Decode)]
                struct MessageFailedEvent {
                    from_chain: u32,
                    to_chain: u32,
                    payload: Vec<u8>,
                    error: String,
                    timestamp: u64,
                }

                let event_data: MessageFailedEvent = Decode::decode(&mut &event.bytes()[..])
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
                    from_chain: ChainId::try_from(event_data.from_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid from_chain id: {}", event_data.from_chain)))?,
                    to_chain: ChainId::try_from(event_data.to_chain as u64)
                        .map_err(|_| AdapterError::Other(format!("Invalid to_chain id: {}", event_data.to_chain)))?,
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
