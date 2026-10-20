//! Central in-memory Model and Provider Registry.
//!
//! Conforms strictly to ARCHITECTURE.md §2, API_SPEC.md §3, and MASTER_SPEC.md §5 & §18 Phase 5.

use crate::errors::AppError;
use crate::models::{ModelMetadata, ProviderMetadata};
use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Strongly typed errors originating from model registry operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    ModelNotFound(String),
    InvalidModel(String),
    DuplicateModel(String),
    ProviderNotFound(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ModelNotFound(id) => write!(f, "Model '{id}' not found in registry"),
            Self::InvalidModel(reason) => write!(f, "Invalid model metadata: {reason}"),
            Self::DuplicateModel(id) => write!(f, "Model '{id}' is already registered"),
            Self::ProviderNotFound(p) => write!(f, "Provider '{p}' not found in registry"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<RegistryError> for AppError {
    fn from(err: RegistryError) -> Self {
        match err {
            RegistryError::ModelNotFound(id) => {
                AppError::NotFound(format!("Model '{id}' not found"))
            }
            RegistryError::InvalidModel(reason) => AppError::InvalidRequest(reason),
            RegistryError::DuplicateModel(id) => {
                AppError::InvalidRequest(format!("Model '{id}' is already registered"))
            }
            RegistryError::ProviderNotFound(p) => {
                AppError::NotFound(format!("Provider '{p}' not found"))
            }
        }
    }
}

/// Query filter parameters for the central model catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelFilter {
    pub modality: Option<String>,
    pub speed_tier: Option<String>,
    pub provider: Option<String>,
    pub available_only: Option<bool>,
}

impl ModelFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_modality(mut self, modality: impl Into<String>) -> Self {
        self.modality = Some(modality.into());
        self
    }

    pub fn with_speed_tier(mut self, speed_tier: impl Into<String>) -> Self {
        self.speed_tier = Some(speed_tier.into());
        self
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn with_available_only(mut self, available_only: bool) -> Self {
        self.available_only = Some(available_only);
        self
    }
}

/// Thread-safe central model and provider catalog.
pub struct ModelRegistry {
    models: RwLock<HashMap<String, ModelMetadata>>,
    providers: RwLock<HashMap<String, ProviderMetadata>>,
}

impl ModelRegistry {
    /// Create a new registry pre-loaded with reference models from API_SPEC.md §3.1.
    pub fn new() -> Self {
        let registry = Self::empty();

        // 1. Pre-register standard providers
        registry.register_provider(ProviderMetadata {
            provider_id: "groq-api".to_string(),
            display_name: "Groq Cloud Fast Inference".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            status: "operational".to_string(),
            enabled: true,
            models_count: 2,
            rate_limit_tier: "free-tier-standard".to_string(),
        });

        registry.register_provider(ProviderMetadata {
            provider_id: "openrouter-api".to_string(),
            display_name: "OpenRouter Unified Router".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            status: "operational".to_string(),
            enabled: true,
            models_count: 1,
            rate_limit_tier: "standard-tier".to_string(),
        });

        // 2. Pre-register reference models matching API_SPEC.md §3.1
        let _ = registry.register_model(ModelMetadata {
            model_id: "fast-chat-v1".to_string(),
            display_name: "Llama 3.3 70B Instruct (Groq)".to_string(),
            provider_id: "groq-api".to_string(),
            supported_modalities: vec!["text".to_string(), "code".to_string()],
            context_window: 131072,
            max_output_tokens: 8192,
            streaming_supported: true,
            tool_calling_supported: true,
            speed_tier: "fast".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec![
                "simple_chat".to_string(),
                "fast_reasoning".to_string(),
                "coding".to_string(),
            ],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        });

        let _ = registry.register_model(ModelMetadata {
            model_id: "deep-reasoner-v1".to_string(),
            display_name: "DeepSeek R1 Advanced Reasoning".to_string(),
            provider_id: "openrouter-api".to_string(),
            supported_modalities: vec![
                "text".to_string(),
                "code".to_string(),
                "document".to_string(),
            ],
            context_window: 65536,
            max_output_tokens: 16384,
            streaming_supported: true,
            tool_calling_supported: true,
            speed_tier: "advanced".to_string(),
            cost_tier: "low-cost".to_string(),
            available: true,
            capabilities: vec![
                "complex_analysis".to_string(),
                "deep_reasoning".to_string(),
                "coding".to_string(),
            ],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        });

        let _ = registry.register_model(ModelMetadata {
            model_id: "balanced-assistant-v1".to_string(),
            display_name: "Mixtral 8x7B Instruct".to_string(),
            provider_id: "groq-api".to_string(),
            supported_modalities: vec!["text".to_string()],
            context_window: 32768,
            max_output_tokens: 4096,
            streaming_supported: true,
            tool_calling_supported: false,
            speed_tier: "balanced".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec![
                "simple_chat".to_string(),
                "general_conversation".to_string(),
            ],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        });

        registry
    }

    /// Create a completely empty model and provider catalog without pre-loaded reference entries.
    pub fn empty() -> Self {
        Self {
            models: RwLock::new(HashMap::new()),
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// Validate model metadata invariants against project specifications.
    pub fn validate_model(model: &ModelMetadata) -> Result<(), RegistryError> {
        let id = model.model_id.trim();
        if id.is_empty() {
            return Err(RegistryError::InvalidModel(
                "model_id cannot be empty".to_string(),
            ));
        }
        if id.len() > 128 {
            return Err(RegistryError::InvalidModel(
                "model_id exceeds maximum length of 128 characters".to_string(),
            ));
        }
        if !id.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/' || c == ':'
        }) {
            return Err(RegistryError::InvalidModel(
                "model_id contains invalid characters; only alphanumeric, -, _, ., /, : allowed"
                    .to_string(),
            ));
        }

        let name = model.display_name.trim();
        if name.is_empty() {
            return Err(RegistryError::InvalidModel(
                "display_name cannot be empty".to_string(),
            ));
        }
        if name.len() > 256 {
            return Err(RegistryError::InvalidModel(
                "display_name exceeds maximum length of 256 characters".to_string(),
            ));
        }

        let prov = model.provider_id.trim();
        if prov.is_empty() {
            return Err(RegistryError::InvalidModel(
                "provider_id cannot be empty".to_string(),
            ));
        }
        if prov.len() > 128 {
            return Err(RegistryError::InvalidModel(
                "provider_id exceeds maximum length of 128 characters".to_string(),
            ));
        }

        if model.supported_modalities.is_empty() {
            return Err(RegistryError::InvalidModel(
                "supported_modalities must contain at least one modality".to_string(),
            ));
        }
        for mod_str in &model.supported_modalities {
            if mod_str.trim().is_empty() {
                return Err(RegistryError::InvalidModel(
                    "supported_modalities cannot contain empty strings".to_string(),
                ));
            }
        }

        if model.context_window == 0 {
            return Err(RegistryError::InvalidModel(
                "context_window must be greater than 0".to_string(),
            ));
        }

        if model.max_output_tokens == 0 {
            return Err(RegistryError::InvalidModel(
                "max_output_tokens must be greater than 0".to_string(),
            ));
        }

        if model.max_output_tokens > model.context_window {
            return Err(RegistryError::InvalidModel(format!(
                "max_output_tokens ({}) cannot exceed context_window ({})",
                model.max_output_tokens, model.context_window
            )));
        }

        let speed = model.speed_tier.to_ascii_lowercase();
        if !["fast", "balanced", "advanced"].contains(&speed.as_str()) {
            return Err(RegistryError::InvalidModel(format!(
                "speed_tier '{speed}' is invalid; must be 'fast', 'balanced', or 'advanced'"
            )));
        }

        let cost = model.cost_tier.to_ascii_lowercase();
        if !["free-tier", "low-cost", "premium"].contains(&cost.as_str()) {
            return Err(RegistryError::InvalidModel(format!(
                "cost_tier '{cost}' is invalid; must be 'free-tier', 'low-cost', or 'premium'"
            )));
        }

        Ok(())
    }

    /// Register or update a validated model in the central catalog.
    pub fn register_model(&self, model: ModelMetadata) -> Result<(), RegistryError> {
        Self::validate_model(&model)?;
        let mut map = self.models.write().unwrap();
        map.insert(model.model_id.clone(), model);
        Ok(())
    }

    /// Register a new model, returning an error if a model with the same ID is already registered.
    pub fn register_new_model(&self, model: ModelMetadata) -> Result<(), RegistryError> {
        Self::validate_model(&model)?;
        let mut map = self.models.write().unwrap();
        if map.contains_key(&model.model_id) {
            return Err(RegistryError::DuplicateModel(model.model_id));
        }
        map.insert(model.model_id.clone(), model);
        Ok(())
    }

    /// Update an existing model, returning an error if the model does not exist.
    pub fn update_model(&self, model: ModelMetadata) -> Result<(), RegistryError> {
        Self::validate_model(&model)?;
        let mut map = self.models.write().unwrap();
        if !map.contains_key(&model.model_id) {
            return Err(RegistryError::ModelNotFound(model.model_id));
        }
        map.insert(model.model_id.clone(), model);
        Ok(())
    }

    /// Remove a model from the catalog by its ID, returning the removed model.
    pub fn remove_model(&self, model_id: &str) -> Result<ModelMetadata, RegistryError> {
        let mut map = self.models.write().unwrap();
        map.remove(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))
    }

    /// Retrieve a model by its unique identifier (preserves Option-based API).
    pub fn get_model(&self, model_id: &str) -> Option<ModelMetadata> {
        let map = self.models.read().unwrap();
        map.get(model_id).cloned()
    }

    /// Lookup a model by ID, returning typed RegistryError if not found.
    pub fn lookup(&self, model_id: &str) -> Result<ModelMetadata, RegistryError> {
        self.get_model(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))
    }

    /// Check if a model exists in the catalog.
    pub fn has_model(&self, model_id: &str) -> bool {
        let map = self.models.read().unwrap();
        map.contains_key(model_id)
    }

    /// Query and filter models according to filter parameters.
    pub fn query_models(&self, filter: &ModelFilter) -> Vec<ModelMetadata> {
        let map = self.models.read().unwrap();
        map.values()
            .filter(|m| {
                if let Some(ref req_mod) = filter.modality {
                    let req_mod_trimmed = req_mod.trim();
                    if !req_mod_trimmed.is_empty()
                        && !m
                            .supported_modalities
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(req_mod_trimmed))
                    {
                        return false;
                    }
                }

                if let Some(ref req_tier) = filter.speed_tier {
                    let req_tier_trimmed = req_tier.trim();
                    if !req_tier_trimmed.is_empty()
                        && !m.speed_tier.eq_ignore_ascii_case(req_tier_trimmed)
                    {
                        return false;
                    }
                }

                if let Some(ref req_prov) = filter.provider {
                    let req_prov_trimmed = req_prov.trim();
                    if !req_prov_trimmed.is_empty()
                        && !m.provider_id.eq_ignore_ascii_case(req_prov_trimmed)
                    {
                        return false;
                    }
                }

                if filter.available_only == Some(true) && !m.available {
                    return false;
                }

                true
            })
            .cloned()
            .collect()
    }

    /// Query and filter models according to query parameters (preserves existing 3-arg signature).
    pub fn list_models(
        &self,
        modality: Option<&str>,
        speed_tier: Option<&str>,
        provider: Option<&str>,
    ) -> Vec<ModelMetadata> {
        let filter = ModelFilter {
            modality: modality.map(String::from),
            speed_tier: speed_tier.map(String::from),
            provider: provider.map(String::from),
            available_only: None,
        };
        self.query_models(&filter)
    }

    /// Update availability state for a specific model and refresh its health check timestamp.
    pub fn set_model_availability(
        &self,
        model_id: &str,
        available: bool,
    ) -> Result<(), RegistryError> {
        let mut map = self.models.write().unwrap();
        let model = map
            .get_mut(model_id)
            .ok_or_else(|| RegistryError::ModelNotFound(model_id.to_string()))?;

        model.available = available;
        model.last_health_check = rfc3339_now();
        Ok(())
    }

    /// Bulk update availability for all models associated with a provider.
    ///
    /// Returns the number of models updated.
    pub fn set_provider_models_availability(&self, provider_id: &str, available: bool) -> usize {
        let mut map = self.models.write().unwrap();
        let mut updated = 0;
        let now = rfc3339_now();

        for model in map.values_mut() {
            if model.provider_id.eq_ignore_ascii_case(provider_id) {
                model.available = available;
                model.last_health_check = now.clone();
                updated += 1;
            }
        }

        updated
    }

    /// Register a new or updated provider.
    pub fn register_provider(&self, provider: ProviderMetadata) {
        let mut map = self.providers.write().unwrap();
        map.insert(provider.provider_id.clone(), provider);
    }

    /// Retrieve provider metadata by ID.
    pub fn get_provider(&self, provider_id: &str) -> Option<ProviderMetadata> {
        let map = self.providers.read().unwrap();
        map.get(provider_id).cloned()
    }

    /// List all registered providers.
    pub fn list_providers(&self) -> Vec<ProviderMetadata> {
        let map = self.providers.read().unwrap();
        map.values().cloned().collect()
    }

    /// Return the total count of registered models.
    pub fn model_count(&self) -> usize {
        let map = self.models.read().unwrap();
        map.len()
    }

    /// Return the count of models with available = true.
    pub fn available_model_count(&self) -> usize {
        let map = self.models.read().unwrap();
        map.values().filter(|m| m.available).count()
    }

    /// Return the total count of registered providers.
    pub fn provider_count(&self) -> usize {
        let map = self.providers.read().unwrap();
        map.len()
    }

    /// Clear all models and providers from the registry.
    pub fn clear(&self) {
        let mut models = self.models.write().unwrap();
        let mut providers = self.providers.write().unwrap();
        models.clear();
        providers.clear();
    }
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to generate RFC 3339 UTC timestamp without third-party crates.
fn rfc3339_now() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();

    let days = (secs / 86400) as i64;
    let time_of_day = (secs % 86400) as u32;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hours, minutes, seconds
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_initialization() {
        let reg = ModelRegistry::new();
        assert_eq!(reg.model_count(), 3);
        assert_eq!(reg.available_model_count(), 3);
        assert_eq!(reg.provider_count(), 2);

        let models = reg.list_models(None, None, None);
        assert_eq!(models.len(), 3);

        let providers = reg.list_providers();
        assert_eq!(providers.len(), 2);
    }

    #[test]
    fn test_empty_registry() {
        let reg = ModelRegistry::empty();
        assert_eq!(reg.model_count(), 0);
        assert_eq!(reg.available_model_count(), 0);
        assert_eq!(reg.provider_count(), 0);
        assert!(reg.list_models(None, None, None).is_empty());
    }

    #[test]
    fn test_model_validation_rules() {
        // Valid model
        let valid = ModelMetadata {
            model_id: "valid-model-1".to_string(),
            display_name: "Valid Model".to_string(),
            provider_id: "test-prov".to_string(),
            supported_modalities: vec!["text".to_string()],
            context_window: 4096,
            max_output_tokens: 2048,
            streaming_supported: true,
            tool_calling_supported: false,
            speed_tier: "fast".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec!["chat".to_string()],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        };
        assert!(ModelRegistry::validate_model(&valid).is_ok());

        // Empty ID
        let mut invalid = valid.clone();
        invalid.model_id = "   ".to_string();
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));

        // Invalid chars in ID
        let mut invalid = valid.clone();
        invalid.model_id = "model with spaces".to_string();
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));

        // Output tokens exceed context window
        let mut invalid = valid.clone();
        invalid.max_output_tokens = 8192;
        invalid.context_window = 4096;
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));

        // Invalid speed tier
        let mut invalid = valid.clone();
        invalid.speed_tier = "ultra-sonic".to_string();
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));

        // Invalid cost tier
        let mut invalid = valid.clone();
        invalid.cost_tier = "expensive".to_string();
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));

        // Empty modalities
        let mut invalid = valid.clone();
        invalid.supported_modalities = vec![];
        assert!(matches!(
            ModelRegistry::validate_model(&invalid),
            Err(RegistryError::InvalidModel(_))
        ));
    }

    #[test]
    fn test_register_new_model_and_duplicates() {
        let reg = ModelRegistry::empty();
        let model = ModelMetadata {
            model_id: "unique-model-1".to_string(),
            display_name: "Unique Model".to_string(),
            provider_id: "prov-1".to_string(),
            supported_modalities: vec!["text".to_string()],
            context_window: 8192,
            max_output_tokens: 4096,
            streaming_supported: true,
            tool_calling_supported: false,
            speed_tier: "balanced".to_string(),
            cost_tier: "free-tier".to_string(),
            available: true,
            capabilities: vec![],
            last_health_check: "2026-10-19T18:00:00Z".to_string(),
        };

        assert!(reg.register_new_model(model.clone()).is_ok());
        assert_eq!(reg.model_count(), 1);

        // Attempting to register again as new returns DuplicateModel
        match reg.register_new_model(model) {
            Err(RegistryError::DuplicateModel(id)) => assert_eq!(id, "unique-model-1"),
            _ => panic!("Expected DuplicateModel error"),
        }
    }

    #[test]
    fn test_lookup_and_removal() {
        let reg = ModelRegistry::new();
        assert!(reg.has_model("fast-chat-v1"));

        let model = reg.lookup("fast-chat-v1").unwrap();
        assert_eq!(model.display_name, "Llama 3.3 70B Instruct (Groq)");

        match reg.lookup("nonexistent") {
            Err(RegistryError::ModelNotFound(id)) => assert_eq!(id, "nonexistent"),
            _ => panic!("Expected ModelNotFound error"),
        }

        let removed = reg.remove_model("fast-chat-v1").unwrap();
        assert_eq!(removed.model_id, "fast-chat-v1");
        assert!(!reg.has_model("fast-chat-v1"));
        assert_eq!(reg.model_count(), 2);
    }

    #[test]
    fn test_availability_tracking() {
        let reg = ModelRegistry::new();

        // 1. Initial state: all 3 available
        assert_eq!(reg.available_model_count(), 3);

        // 2. Set individual model unavailable
        reg.set_model_availability("fast-chat-v1", false).unwrap();
        assert_eq!(reg.available_model_count(), 2);
        let m = reg.get_model("fast-chat-v1").unwrap();
        assert!(!m.available);

        // Filter available_only excludes it
        let filter = ModelFilter::new().with_available_only(true);
        let avail_models = reg.query_models(&filter);
        assert_eq!(avail_models.len(), 2);
        assert!(!avail_models.iter().any(|x| x.model_id == "fast-chat-v1"));

        // Standard list_models still includes it (with available: false)
        let all_models = reg.list_models(None, None, None);
        assert_eq!(all_models.len(), 3);

        // 3. Provider-level availability update
        // groq-api has fast-chat-v1 and balanced-assistant-v1
        let updated = reg.set_provider_models_availability("groq-api", false);
        assert_eq!(updated, 2);
        assert_eq!(reg.available_model_count(), 1); // Only deep-reasoner-v1 (openrouter-api) is left available
    }

    #[test]
    fn test_advanced_query_filtering() {
        let reg = ModelRegistry::new();

        // Modality filter (case-insensitive)
        let filter = ModelFilter::new().with_modality("CODE");
        let code_models = reg.query_models(&filter);
        assert_eq!(code_models.len(), 2);

        // Speed tier filter
        let filter = ModelFilter::new().with_speed_tier("fast");
        let fast_models = reg.query_models(&filter);
        assert_eq!(fast_models.len(), 1);
        assert_eq!(fast_models[0].model_id, "fast-chat-v1");

        // Provider filter
        let filter = ModelFilter::new().with_provider("openrouter-api");
        let or_models = reg.query_models(&filter);
        assert_eq!(or_models.len(), 1);
        assert_eq!(or_models[0].model_id, "deep-reasoner-v1");

        // Combined filter with no matches
        let filter = ModelFilter::new()
            .with_modality("image")
            .with_provider("groq-api");
        assert!(reg.query_models(&filter).is_empty());
    }

    #[test]
    fn test_registry_error_into_app_error() {
        let err = RegistryError::ModelNotFound("missing-model".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 404);
        assert_eq!(app_err.error_code(), "NOT_FOUND");

        let err = RegistryError::InvalidModel("context window must be > 0".to_string());
        let app_err: AppError = err.into();
        assert_eq!(app_err.status_code(), 400);
        assert_eq!(app_err.error_code(), "INVALID_REQUEST");
    }
}
