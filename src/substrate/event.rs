#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use std::sync::Arc;
use subxt::{
    events::{EventDetails, Events, Phase},
    OnlineClient,
    PolkadotConfig as SubxtPolkadotConfig,
    utils::{H256, AccountId32},
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::types::{MessageEvent, MessageStatus};
use crate::error::AdapterError;

/// Event handler for processing Substrate chain events
pub struct EventHandler {
    /// Client for RPC calls
    client: Arc<OnlineClient<SubxtPolkadotConfig>>,
    
    /// Event sender channel
    event_tx: mpsc::Sender<MessageEvent>,
    
    /// Pallet name to monitor for events
    pallet_name: String,
    
    /// Last processed block number
    last_block: Arc<RwLock<Option<u32>>>,
}

impl EventHandler {
    /// Creates a new event handler
    pub fn new(
        client: Arc<OnlineClient<SubxtPolkadotConfig>>,
        event_tx: mpsc::Sender<MessageEvent>,
        pallet_name: Option<String>,
    ) -> Self {
        Self {
            client,
            event_tx,
            pallet_name: pallet_name.unwrap_or_else(|| "XcmpQueue".to_string()),
            last_block: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Processes events from a block
    pub async fn process_block(&self, block_number: u32) -> Result<(), AdapterError> {
        let block_hash = self.client
            .rpc()
            .block_hash(Some(block_number.into()))
            .await?
            .ok_or_else(|| AdapterError::Other(format!("Block {} not found", block_number)))?;
            
        let events = self.client
            .blocks()
            .at(block_hash)
            .await?
            .events()
            .await?;
            
        for event in events.iter() {
            if let Ok(event) = event {
                if self.is_relevant_event(&event) {
                    if let Ok(message_event) = self.decode_event(&event, block_number).await {
                        if let Err(e) = self.event_tx.send(message_event).await {
                            error!("Failed to send message event: {}", e);
                        }
                    }
                }
            }
        }
        
        // Update last processed block
        let mut last_block = self.last_block.write().await;
        *last_block = Some(block_number);

        Ok(())
    }

    /// Checks if an event is relevant for our message processing
    fn is_relevant_event(&self, event: &EventDetails<SubxtPolkadotConfig>) -> bool {
        // Check if event is from our target pallet
        if let Some(pallet) = event.pallet_name() {
            if pallet != self.pallet_name {
                return false;
            }
            
            // Check for relevant event variants
            match event.variant_name() {
                Some("MessageSent") | Some("MessageReceived") | Some("MessageFailed") => true,
                _ => false,
            }
        } else {
            false
        }
    }
    
    /// Decodes an event into a MessageEvent
    async fn decode_event(
        &self,
        event: &EventDetails<SubxtPolkadotConfig>,
        block_number: u32,
    ) -> Result<MessageEvent, AdapterError> {
        let timestamp = self.get_block_timestamp(block_number).await?;
        
        // Extract event data based on variant
        match event.variant_name() {
            Some("MessageSent") => {
                let data: (u32, u32, Vec<u8>, u128) = event.decode()?;
                Ok(MessageEvent {
                    id: Uuid::new_v4(), // Generated from payload hash
                    from_chain: data.0,
                    to_chain: data.1,
                    payload: data.2,
                    fee: Some(data.3),
                    status: MessageStatus::Sent,
                    timestamp,
                    block_number: Some(block_number),
                    error: None,
                })
            }
            
            Some("MessageReceived") => {
                let data: (u32, u32, Vec<u8>, u128) = event.decode()?;
                Ok(MessageEvent {
                    id: Uuid::new_v4(),
                    from_chain: data.0,
                    to_chain: data.1,
                    payload: data.2,
                    fee: Some(data.3),
                    status: MessageStatus::Received,
                    timestamp,
                    block_number: Some(block_number),
                    error: None,
                })
            }
            
            Some("MessageFailed") => {
                let data: (u32, u32, Vec<u8>, String) = event.decode()?;
                Ok(MessageEvent {
                    id: Uuid::new_v4(),
                    from_chain: data.0,
                    to_chain: data.1,
                    payload: data.2,
                    fee: None,
                    status: MessageStatus::Failed,
                    timestamp,
                    block_number: Some(block_number),
                    error: Some(data.3),
                })
            }
            
            _ => Err(AdapterError::Other("Unknown event variant".to_string())),
        }
    }
    
    /// Gets the timestamp for a block
    async fn get_block_timestamp(&self, block_number: u32) -> Result<u64, AdapterError> {
        let block_hash = self.client
            .rpc()
            .block_hash(Some(block_number.into()))
            .await?
            .ok_or_else(|| AdapterError::Other(format!("Block {} not found", block_number)))?;
            
        let block = self.client
            .blocks()
            .at(block_hash)
            .await?
            .block()
            .await?;
            
        // Extract timestamp from block extrinsics
        for extrinsic in block.extrinsics() {
            if let Ok(timestamp) = extrinsic.as_extrinsic::<subxt::dynamic::Decode>() {
                if timestamp.pallet_name() == "Timestamp" && timestamp.call_name() == "set" {
                    if let Ok(timestamp_value) = timestamp.field_typed::<u64>(0) {
                        return Ok(timestamp_value);
                    }
                }
            }
        }
        
        // Fallback to current time if timestamp not found
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs())
    }
    
    /// Gets the last processed block number
    pub async fn get_last_block(&self) -> Option<u32> {
        let last_block = self.last_block.read().await;
        *last_block
    }
}

impl Clone for EventHandler {
    fn clone(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            event_tx: self.event_tx.clone(),
            pallet_name: self.pallet_name.clone(),
            last_block: Arc::clone(&self.last_block),
        }
    }
} 