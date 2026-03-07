use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::vulkan_dma_buf::{DmaBufImage, VulkanDmaBufProducer};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub id: String,
    pub producer_id: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub created_at: String,
    pub frame_count: u64,
}

#[derive(Debug, Clone)]
pub struct StreamFrame {
    pub frame_number: u64,
    pub timestamp_ms: u64,
    pub dma_buf: Arc<Mutex<Option<DmaBufImage>>>,
}

pub struct DmaBufStreamManager {
    streams: Arc<RwLock<HashMap<String, StreamInfo>>>,
    stream_frames: Arc<RwLock<HashMap<String, mpsc::Sender<StreamFrame>>>>,
    vulkan_producer: Arc<Mutex<Option<VulkanDmaBufProducer>>>,
}

impl DmaBufStreamManager {
    pub fn new() -> Self {
        log::info!("Initializing DmaBufStreamManager");
        
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
            stream_frames: Arc::new(RwLock::new(HashMap::new())),
            vulkan_producer: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn initialize_vulkan(&self) -> Result<()> {
        let mut vulkan = self.vulkan_producer.lock().await;
        if vulkan.is_none() {
            match VulkanDmaBufProducer::new() {
                Ok(producer) => {
                    log::info!("Vulkan initialized for stream manager");
                    *vulkan = Some(producer);
                }
                Err(e) => {
                    log::error!("Failed to initialize Vulkan: {}", e);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    pub async fn create_stream(
        &self,
        stream_id: String,
        producer_id: String,
        width: u32,
        height: u32,
        format: String,
    ) -> Result<StreamInfo> {
        log::info!("Creating DMA-BUF stream: {} ({}x{} {})", stream_id, width, height, format);

        self.initialize_vulkan().await?;

        let stream_info = StreamInfo {
            id: stream_id.clone(),
            producer_id,
            width,
            height,
            format,
            created_at: chrono::Utc::now().to_rfc3339(),
            frame_count: 0,
        };

        let (tx, _rx) = mpsc::channel(100);
        
        let mut streams = self.streams.write().await;
        streams.insert(stream_id.clone(), stream_info.clone());

        let mut stream_frames = self.stream_frames.write().await;
        stream_frames.insert(stream_id, tx);

        log::info!("Stream created successfully");
        Ok(stream_info)
    }

    pub async fn push_frame(
        &self,
        stream_id: &str,
        dma_buf: Arc<Mutex<Option<DmaBufImage>>>,
    ) -> Result<()> {
        let stream_frames = self.stream_frames.read().await;
        
        if let Some(sender) = stream_frames.get(stream_id) {
            let mut streams = self.streams.write().await;
            if let Some(stream) = streams.get_mut(stream_id) {
                stream.frame_count += 1;
            }
            
            let frame = StreamFrame {
                frame_number: streams.get(stream_id).map(|s| s.frame_count).unwrap_or(0),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                dma_buf,
            };

            sender.send(frame).await.map_err(|e| anyhow::anyhow!("Failed to send frame: {}", e))?;
            log::debug!("Frame pushed to stream {}", stream_id);
        }
        
        Ok(())
    }

    pub async fn subscribe_stream(&self, stream_id: &str) -> Result<mpsc::Receiver<StreamFrame>> {
        let stream_frames = self.stream_frames.read().await;
        
        if let Some(_sender) = stream_frames.get(stream_id) {
            let (tx, rx) = mpsc::channel(100);
            
            drop(stream_frames);
            
            let mut frames = self.stream_frames.write().await;
            if let Some(sender) = frames.get_mut(stream_id) {
                *sender = tx;
            }
            
            log::info!("Subscribed to stream {}", stream_id);
            Ok(rx)
        } else {
            Err(anyhow::anyhow!("Stream {} not found", stream_id))
        }
    }

    pub async fn list_streams(&self) -> Vec<StreamInfo> {
        let streams = self.streams.read().await;
        streams.values().cloned().collect()
    }

    pub async fn get_stream(&self, stream_id: &str) -> Option<StreamInfo> {
        let streams = self.streams.read().await;
        streams.get(stream_id).cloned()
    }

    pub async fn close_stream(&self, stream_id: &str) -> Result<()> {
        let mut streams = self.streams.write().await;
        streams.remove(stream_id);
        
        let mut stream_frames = self.stream_frames.write().await;
        stream_frames.remove(stream_id);
        
        log::info!("Stream {} closed", stream_id);
        Ok(())
    }

    pub fn get_vulkan_producer(&self) -> Arc<Mutex<Option<VulkanDmaBufProducer>>> {
        self.vulkan_producer.clone()
    }
}

impl Default for DmaBufStreamManager {
    fn default() -> Self {
        Self::new()
    }
}
