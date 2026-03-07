use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DataMetadata {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub timestamp: u64,
}

pub struct DataTransferManager {
    buffers: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    metadata: Arc<Mutex<HashMap<String, DataMetadata>>>,
}

impl DataTransferManager {
    pub fn new() -> Self {
        Self {
            buffers: Arc::new(Mutex::new(HashMap::new())),
            metadata: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn store_data(&self, producer_id: &str, data: Vec<u8>, metadata: DataMetadata) -> Result<()> {
        let mut buffers: tokio::sync::MutexGuard<'_, HashMap<String, Vec<u8>>> = self.buffers.lock().await;
        let mut metadata_map: tokio::sync::MutexGuard<'_, HashMap<String, DataMetadata>> = self.metadata.lock().await;

        buffers.insert(producer_id.to_string(), data);
        metadata_map.insert(producer_id.to_string(), metadata);

        Ok(())
    }

    pub async fn get_data(&self, consumer_id: &str) -> Result<(Vec<u8>, DataMetadata)> {
        let buffers = self.buffers.lock().await;
        let metadata_map = self.metadata.lock().await;

        if let Some(data) = buffers.get(consumer_id) {
            if let Some(metadata) = metadata_map.get(consumer_id) {
                return Ok((data.clone(), metadata.clone()));
            }
        }

        Err(anyhow::anyhow!("Data not found for consumer {}", consumer_id))
    }

    pub async fn remove_data(&self, consumer_id: &str) -> Result<()> {
        let mut buffers = self.buffers.lock().await;
        let mut metadata_map = self.metadata.lock().await;

        buffers.remove(consumer_id);
        metadata_map.remove(consumer_id);

        Ok(())
    }
}