#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(unused_must_use)]
#![allow(dead_code)]

use subxt::{
    OnlineClient,
    PolkadotConfig as SubxtPolkadotConfig,
    events::{EventsClient, StaticEvent},
};
use frostgate_sdk::frostmessage::MessageEvent;
use crate::chainadapter::AdapterError;
use tracing::{debug, info};
use std::sync::Arc;

/// Event handler for Substrate chain events
pub struct EventHandler {
    client: Arc<OnlineClient<SubxtPolkadotConfig>>,
    pallet_name: String,
}

impl EventHandler {
    /// Creates a new event handler
    pub fn new(client: Arc<OnlineClient<SubxtPolkadotConfig>>, pallet_name: String) -> Self {
        Self {
            client,
            pallet_name,
        }
    }

    /// Listens for contract events
    pub async fn listen_for_events(&self) -> Result<Vec<MessageEvent>, AdapterError> {
        debug!("Listening for events from pallet: {}", self.pallet_name);

        // TODO: Implement actual event filtering
        // This would typically involve:
        // 1. Creating event filters for the messaging pallet
        // 2. Querying recent blocks for matching events
        // 3. Parsing event logs into MessageEvent structures
        // 4. Managing event cursor/checkpoint for pagination

        // Example implementation:
        // let latest_block = self.client.blocks().at_latest().await
        //     .map_err(|e| AdapterError::Network(format!("Failed to get latest block: {}", e)))?;
        // 
        // let events = latest_block.events().await
        //     .map_err(|e| AdapterError::Network(format!("Failed to get events: {}", e)))?;
        // 
        // let mut message_events = Vec::new();
        // for event in events.iter() {
        //     let event = event.map_err(|e| AdapterError::Other(format!("Event decode error: {}", e)))?;
        //     
        //     if event.pallet_name() == self.pallet_name {
        //         message_events.push(self.convert_substrate_event_to_message_event(event)?);
        //     }
        // }

        // For now, return empty vector
        Ok(Vec::new())
    }

    /// Verifies a message proof on-chain
    pub async fn verify_proof(&self, event: &MessageEvent) -> Result<(), AdapterError> {
        info!("Verifying event proof for message {:?}", event);

        // TODO: Implement actual on-chain proof verification
        // This would involve:
        // 1. Encoding the message and proof data
        // 2. Calling the pallet's verify function
        // 3. Handling revert reasons and errors
        // 4. Managing verification state

        Ok(())
    }

    /// Converts a Substrate event to a Frostgate message event
    fn convert_substrate_event_to_message_event<T>(
        &self,
        _event: T,
    ) -> Result<MessageEvent, AdapterError>
    where
        T: StaticEvent,
    {
        // TODO: Implement event conversion logic
        unimplemented!("Event conversion not yet implemented")
    }
}
