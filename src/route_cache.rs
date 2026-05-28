use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use crate::models::{RouteRequest, RouteResponse};

const TTL: Duration = Duration::from_secs(3600); // 1 h
const MAX_ENTRIES: usize = 500;

struct Entry {
    value:   RouteResponse,
    expires: Instant,
}

#[derive(Clone, Default)]
pub struct RouteCache {
    inner: Arc<RwLock<HashMap<String, Entry>>>,
}

impl RouteCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clé de cache — coordonnées arrondies à 5 décimales (~1 m) + options.
    pub fn key(req: &RouteRequest) -> String {
        format!(
            "{:.5},{:.5}|{:.5},{:.5}|{}|{}|{}",
            req.start.lat, req.start.lng,
            req.end.lat,   req.end.lng,
            req.range_preset.as_deref().unwrap_or("standard"),
            req.avoid_cams.unwrap_or(true),
            req.include_direct.unwrap_or(false),
        )
    }

    pub async fn get(&self, key: &str) -> Option<RouteResponse> {
        let map = self.inner.read().await;
        map.get(key)
            .filter(|e| e.expires > Instant::now())
            .map(|e| e.value.clone())
    }

    pub async fn insert(&self, key: String, value: RouteResponse) {
        let mut map = self.inner.write().await;

        // Éviction des entrées expirées avant d'insérer
        if map.len() >= MAX_ENTRIES {
            let now = Instant::now();
            map.retain(|_, e| e.expires > now);
            // Si toujours plein après purge TTL, retirer la première entrée (FIFO approximatif)
            if map.len() >= MAX_ENTRIES {
                if let Some(k) = map.keys().next().cloned() {
                    map.remove(&k);
                }
            }
        }

        map.insert(key, Entry { value, expires: Instant::now() + TTL });
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }
}
