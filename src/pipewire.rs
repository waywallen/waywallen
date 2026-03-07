use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::vulkan_dma_buf::DmaBufImage;

#[derive(Debug, Clone)]
pub struct DmaBufferData {
    pub dma_buf: Arc<Mutex<Option<DmaBufImage>>>,
    pub metadata: serde_json::Value,
}

pub struct PipewireManager {
    producer_nodes: Arc<Mutex<HashMap<String, u32>>>,
    consumer_nodes: Arc<Mutex<HashMap<String, u32>>>,
    producer_senders: Arc<Mutex<HashMap<String, Vec<tokio::sync::mpsc::UnboundedSender<DmaBufferData>>>>>,
    consumer_receivers: Arc<Mutex<HashMap<String, tokio::sync::mpsc::UnboundedReceiver<DmaBufferData>>>>,
    pipewire_available: bool,
}

impl PipewireManager {
    pub async fn new() -> Result<Self> {
        log::info!("Initializing PipewireManager");

        log::info!("Using channel-based data transfer (Pipewire integration available via future enhancement)");

        Ok(Self {
            producer_nodes: Arc::new(Mutex::new(HashMap::new())),
            consumer_nodes: Arc::new(Mutex::new(HashMap::new())),
            producer_senders: Arc::new(Mutex::new(HashMap::new())),
            consumer_receivers: Arc::new(Mutex::new(HashMap::new())),
            pipewire_available: false,
        })
    }

    pub async fn register_producer(&self, producer_id: &str) -> Result<()> {
        log::info!("Registering producer: {}", producer_id);

        let mut producer_senders = self.producer_senders.lock().await;
        if !producer_senders.contains_key(producer_id) {
            producer_senders.insert(producer_id.to_string(), Vec::new());
            log::info!("Producer {} registered successfully", producer_id);
        }

        Ok(())
    }

    pub async fn register_consumer(&self, consumer_id: &str, producer_id: &str) -> Result<()> {
        log::info!("Registering consumer: {} for producer: {}", consumer_id, producer_id);

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut producer_senders = self.producer_senders.lock().await;
        if let Some(senders) = producer_senders.get_mut(producer_id) {
            senders.push(tx);
            log::info!("Consumer {} subscribed to producer {}", consumer_id, producer_id);
        } else {
            return Err(anyhow::anyhow!("Producer {} not found", producer_id));
        }

        let mut consumer_receivers = self.consumer_receivers.lock().await;
        consumer_receivers.insert(consumer_id.to_string(), rx);

        Ok(())
    }

    pub async fn send_data(
        &self,
        producer_id: &str,
        dma_buf: Arc<Mutex<Option<DmaBufImage>>>,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        log::debug!("Sending DMA-BUF from producer: {}", producer_id);

        let buffer_data = DmaBufferData {
            dma_buf,
            metadata: metadata.clone(),
        };

        let producer_senders = self.producer_senders.lock().await;
        if let Some(senders) = producer_senders.get(producer_id) {
            for sender in senders {
                sender.send(buffer_data.clone())
                    .map_err(|e| anyhow::anyhow!("Send failed: {}", e))?;
            }
            log::debug!("DMA-BUF sent to {} consumers", senders.len());
        } else {
            log::warn!("No senders for producer {}", producer_id);
        }

        Ok(())
    }

    pub async fn receive_data(
        &self,
        consumer_id: &str,
        timeout_ms: u64,
    ) -> Result<(Arc<Mutex<Option<DmaBufImage>>>, serde_json::Value)> {
        log::debug!("Receiving DMA-BUF for consumer: {}", consumer_id);

        let mut consumer_receivers = self.consumer_receivers.lock().await;

        if let Some(receiver) = consumer_receivers.get_mut(consumer_id) {
            use tokio::time::{timeout, Duration};
            let duration = Duration::from_millis(timeout_ms);

            match timeout(duration, receiver.recv()).await {
                Ok(Some(buffer_data)) => {
                    log::debug!("Received DMA-BUF for consumer {}", consumer_id);
                    return Ok((buffer_data.dma_buf, buffer_data.metadata));
                }
                Ok(None) => {
                    return Err(anyhow::anyhow!("Channel closed"));
                }
                Err(_) => {
                    return Err(anyhow::anyhow!("Timeout"));
                }
            }
        }

        Err(anyhow::anyhow!("Consumer {} not found", consumer_id))
    }

    pub fn get_pipewire_info(&self) -> HashMap<String, String> {
        let mut info = HashMap::new();
        info.insert("available".to_string(), "false".to_string());
        info.insert("implementation".to_string(), "tokio_channels".to_string());
        info.insert("note".to_string(), "Pipewire integration requires additional implementation".to_string());
        info
    }

    pub fn is_pipewire_available(&self) -> bool {
        self.pipewire_available
    }
}

unsafe impl Send for PipewireManager {}
unsafe impl Sync for PipewireManager {}
