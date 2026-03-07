use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use chrono::Utc;

pub struct ConsumerManager {
    consumers: Arc<Mutex<Vec<Consumer>>>,
}

impl ConsumerManager {
    pub fn new() -> Self {
        Self {
            consumers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn register(&self, name: &str, ty: &str) -> Consumer {
        let consumer = Consumer {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            ty: ty.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };

        let mut consumers = self.consumers.lock().await;
        consumers.push(consumer.clone());

        consumer
    }

    pub async fn get_consumer(&self, id: &str) -> Option<Consumer> {
        let consumers = self.consumers.lock().await;
        consumers.iter().find(|c| c.id == id).cloned()
    }

    pub async fn remove_consumer(&self, id: &str) -> bool {
        let mut consumers = self.consumers.lock().await;
        let len = consumers.len();
        consumers.retain(|c| c.id != id);
        len != consumers.len()
    }

    pub async fn list_consumers(&self) -> Vec<Consumer> {
        let consumers = self.consumers.lock().await;
        consumers.clone()
    }
}

#[derive(Debug, Clone)]
pub struct Consumer {
    pub id: String,
    pub name: String,
    pub ty: String,
    pub created_at: String,
}