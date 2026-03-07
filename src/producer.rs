use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use chrono::Utc;

pub struct ProducerManager {
    producers: Arc<Mutex<Vec<Producer>>>,
}

impl ProducerManager {
    pub fn new() -> Self {
        Self {
            producers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn register(&self, name: &str, ty: &str) -> Producer {
        let producer = Producer {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            ty: ty.to_string(),
            created_at: Utc::now().to_rfc3339(),
        };

        let mut producers = self.producers.lock().await;
        producers.push(producer.clone());

        producer
    }

    pub async fn get_producer(&self, id: &str) -> Option<Producer> {
        let producers = self.producers.lock().await;
        producers.iter().find(|p| p.id == id).cloned()
    }

    pub async fn remove_producer(&self, id: &str) -> bool {
        let mut producers = self.producers.lock().await;
        let len = producers.len();
        producers.retain(|p| p.id != id);
        len != producers.len()
    }

    pub async fn list_producers(&self) -> Vec<Producer> {
        let producers = self.producers.lock().await;
        producers.clone()
    }
}

#[derive(Debug, Clone)]
pub struct Producer {
    pub id: String,
    pub name: String,
    pub ty: String,
    pub created_at: String,
}