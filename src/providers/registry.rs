//! Provider Registry and Lifecycle Manager.
//!
//! Conforms strictly to ARCHITECTURE.md §2 (`providers` module) and MASTER_SPEC.md §18 Phase 4.

use crate::models::ProviderMetadata;
use crate::providers::{ProviderAdapter, ProviderError};
use crate::registry::ModelRegistry;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Thread-safe Provider Adapter Registry and Lifecycle Manager.
pub struct ProviderRegistry {
    adapters: RwLock<HashMap<String, Arc<dyn ProviderAdapter>>>,
    statuses: RwLock<HashMap<String, String>>,
}

impl ProviderRegistry {
    /// Create a new empty provider registry.
    pub fn new() -> Self {
        Self {
            adapters: RwLock::new(HashMap::new()),
            statuses: RwLock::new(HashMap::new()),
        }
    }

    /// Register a provider adapter.
    ///
    /// Validates provider_id is non-empty. Safely replaces any existing adapter with the same ID.
    pub fn register(&self, adapter: Arc<dyn ProviderAdapter>) -> Result<(), ProviderError> {
        let info = adapter.provider_info();
        let id = info.provider_id.trim().to_string();
        if id.is_empty() {
            return Err(ProviderError::InvalidRequest(
                "Provider ID cannot be empty".to_string(),
            ));
        }

        let initial_status = if info.enabled {
            info.status
        } else {
            "offline".to_string()
        };

        {
            let mut adapters = self.adapters.write().unwrap();
            adapters.insert(id.clone(), adapter);
        }
        {
            let mut statuses = self.statuses.write().unwrap();
            statuses.insert(id, initial_status);
        }

        Ok(())
    }

    /// Retrieve an adapter by its unique provider ID.
    ///
    /// Returns `ProviderError::ProviderNotFound` if the provider is not registered.
    /// Returns `ProviderError::ProviderDisabled` if the provider is registered but disabled.
    pub fn get(&self, provider_id: &str) -> Result<Arc<dyn ProviderAdapter>, ProviderError> {
        let adapters = self.adapters.read().unwrap();
        let adapter = adapters
            .get(provider_id)
            .ok_or_else(|| ProviderError::ProviderNotFound(provider_id.to_string()))?;

        let info = adapter.provider_info();
        if !info.enabled {
            return Err(ProviderError::ProviderDisabled(provider_id.to_string()));
        }

        Ok(adapter.clone())
    }

    /// Check if a provider is registered (regardless of enabled/disabled state).
    pub fn has_provider(&self, provider_id: &str) -> bool {
        let adapters = self.adapters.read().unwrap();
        adapters.contains_key(provider_id)
    }

    /// Deregister a provider by its unique identifier.
    pub fn deregister(&self, provider_id: &str) -> Option<Arc<dyn ProviderAdapter>> {
        let mut adapters = self.adapters.write().unwrap();
        let mut statuses = self.statuses.write().unwrap();
        statuses.remove(provider_id);
        adapters.remove(provider_id)
    }

    /// Return all registered adapters.
    pub fn list_adapters(&self) -> Vec<Arc<dyn ProviderAdapter>> {
        let adapters = self.adapters.read().unwrap();
        adapters.values().cloned().collect()
    }

    /// Return normalized metadata for all registered providers.
    ///
    /// The returned metadata reflects the dynamic operational status tracked by the registry.
    pub fn list_metadata(&self) -> Vec<ProviderMetadata> {
        let adapters = self.adapters.read().unwrap();
        let statuses = self.statuses.read().unwrap();

        adapters
            .values()
            .map(|adapter| {
                let mut meta = adapter.provider_info();
                if let Some(status) = statuses.get(&meta.provider_id) {
                    meta.status = status.clone();
                }
                meta
            })
            .collect()
    }

    /// Return the count of active (enabled and operational) providers.
    pub fn active_providers_count(&self) -> usize {
        self.active_provider_ids().len()
    }

    /// Return the list of provider IDs for currently active (enabled and operational) providers.
    pub fn active_provider_ids(&self) -> Vec<String> {
        let adapters = self.adapters.read().unwrap();
        let statuses = self.statuses.read().unwrap();

        adapters
            .values()
            .filter(|adapter| {
                let info = adapter.provider_info();
                if !info.enabled {
                    return false;
                }
                let status = statuses
                    .get(&info.provider_id)
                    .map(|s| s.as_str())
                    .unwrap_or("operational");
                status == "operational"
            })
            .map(|adapter| adapter.provider_info().provider_id)
            .collect()
    }

    /// Update status for a registered provider ("operational", "degraded", "offline").
    pub fn set_status(&self, provider_id: &str, status: &str) -> Result<(), ProviderError> {
        if !self.has_provider(provider_id) {
            return Err(ProviderError::ProviderNotFound(provider_id.to_string()));
        }
        let mut statuses = self.statuses.write().unwrap();
        statuses.insert(provider_id.to_string(), status.to_string());
        Ok(())
    }

    /// Check health of a single provider and update its status.
    pub async fn check_provider_health(&self, provider_id: &str) -> Result<bool, ProviderError> {
        let adapter = self.get(provider_id)?;
        match adapter.check_health().await {
            Ok(true) => {
                let _ = self.set_status(provider_id, "operational");
                Ok(true)
            }
            Ok(false) => {
                let _ = self.set_status(provider_id, "offline");
                Ok(false)
            }
            Err(err) => {
                let _ = self.set_status(provider_id, "offline");
                Err(err)
            }
        }
    }

    /// Check health of all registered providers concurrently.
    ///
    /// Failure of one provider is isolated and does not affect other checks.
    pub async fn check_all_health(&self) -> HashMap<String, Result<bool, ProviderError>> {
        let adapters: Vec<(String, Arc<dyn ProviderAdapter>)> = {
            let map = self.adapters.read().unwrap();
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        let mut results = HashMap::new();
        for (id, adapter) in adapters {
            let res = adapter.check_health().await;
            match &res {
                Ok(true) => {
                    let _ = self.set_status(&id, "operational");
                }
                Ok(false) | Err(_) => {
                    let _ = self.set_status(&id, "offline");
                }
            }
            results.insert(id, res);
        }

        results
    }

    /// Discover models from all registered adapters and register them into the central ModelRegistry.
    ///
    /// Validates model metadata and associates models with their provider.
    /// Returns the total number of successfully synced models.
    pub async fn sync_models_to_registry(
        &self,
        model_registry: &ModelRegistry,
    ) -> Result<usize, ProviderError> {
        let adapters = self.list_adapters();
        let mut total_synced = 0;

        for adapter in adapters {
            let info = adapter.provider_info();
            // Also register provider metadata into ModelRegistry
            model_registry.register_provider(info.clone());

            let models = adapter.discover_models().await?;
            for mut model in models {
                // Ensure model metadata is properly tied to this provider
                if model.provider_id.is_empty() {
                    model.provider_id = info.provider_id.clone();
                }
                model_registry
                    .register_model(model)
                    .map_err(|e| ProviderError::InvalidRequest(e.to_string()))?;
                total_synced += 1;
            }
        }

        Ok(total_synced)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
