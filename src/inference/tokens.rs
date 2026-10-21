//! Token estimation, caching, and conservative prompt optimization engine.
//!
//! Conforms to MASTER_SPEC.md §10, ARCHITECTURE.md §2 & §8, and API_SPEC.md §5 & §6.
//!
//! Provides model-family-aware deterministic token approximation, collision-safe bounded
//! LRU token caching, conservative prompt compaction preserving code indentation,
//! and consecutive duplicate system prompt removal.

use crate::models::ChatMessage;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use tiktoken_rs::CoreBPE;

/// Lazily initialized static instance of OpenAI's `cl100k_base` BPE tokenizer.
/// Used for GPT-4, GPT-3.5-Turbo, and compatible models.
static CL100K_BPE: std::sync::LazyLock<Option<CoreBPE>> =
    std::sync::LazyLock::new(|| tiktoken_rs::cl100k_base().ok());

/// Lazily initialized static instance of OpenAI's `o200k_base` BPE tokenizer.
/// Used for GPT-4o, GPT-4o-mini, o1, o3, and next-generation OpenAI models.
static O200K_BPE: std::sync::LazyLock<Option<CoreBPE>> =
    std::sync::LazyLock::new(|| tiktoken_rs::o200k_base().ok());

/// Tokenizer capability level reflecting whether exact native tokenization is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerCapability {
    /// Verified native tokenizer is available for exact raw-text BPE encoding.
    Exact,
    /// Only calibrated or conservative heuristic estimation is available.
    Approximate,
    /// Authoritative post-execution usage count returned directly by the provider.
    ProviderReported,
}

/// Tokenizer encoding variant supported natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BpeEncoding {
    Cl100k,
    O200k,
}

impl BpeEncoding {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Cl100k => "cl100k",
            Self::O200k => "o200k",
        }
    }

    pub fn get_bpe(&self) -> Option<&'static CoreBPE> {
        match self {
            Self::Cl100k => CL100K_BPE.as_ref(),
            Self::O200k => O200K_BPE.as_ref(),
        }
    }
}

/// Classification of model-to-tokenizer mapping provenance and authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingKind {
    /// Exact model ID with verified tokenizer encoding from authoritative source (e.g. tiktoken).
    ExactVerified,
    /// Model name matching an authoritative family prefix (e.g. `gpt-4o-`, `o1-`, `o3-`).
    FamilyPrefixVerified,
    /// Explicit application or reference alias mapped to a tokenizer family (e.g. `fast-chat-v1` -> Llama).
    ExplicitAlias,
    /// Model registered in the ModelRegistry whose tokenizer mapping is uncalibrated / unverified.
    RegisteredUnverified,
    /// Unknown model identifier not recognized in verified mappings or registry.
    Unknown,
}

/// Comprehensive resolution result for a model identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokenizerResolution {
    pub normalized_model_id: String,
    pub mapping_kind: MappingKind,
    pub family: TokenizerFamily,
    pub bpe_encoding: Option<BpeEncoding>,
    pub is_exact_raw_text_supported: bool,
    pub rationale: &'static str,
}

/// Identified tokenizer family for model-specific tokenization heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerFamily {
    /// Meta Llama 3 / 3.1 / 3.2 family (128k BPE vocabulary).
    Llama,
    /// Mistral / Mixtral family (32k SPM vocabulary).
    Mistral,
    /// Google Gemma / Gemma 2 family (256k SentencePiece vocabulary).
    Gemma,
    /// Alibaba Qwen 2 / 2.5 family (152k BPE vocabulary).
    Qwen,
    /// DeepSeek-V2 / V3 / R1 family (128k BPE vocabulary).
    DeepSeek,
    /// OpenAI GPT-4 / GPT-4o / o1 / o3 / o4-mini / GPT-5 family (100k-200k tiktoken BPE vocabulary).
    Gpt,
    /// Fallback conservative generic tokenizer heuristic.
    Generic,
}

struct ExactMapping {
    id: &'static str,
    family: TokenizerFamily,
    encoding: Option<BpeEncoding>,
    rationale: &'static str,
}

static EXACT_VERIFIED_MODELS: &[ExactMapping] = &[
    // OpenAI o-series reasoning models (verified o200k_base in tiktoken MODEL_TO_ENCODING)
    ExactMapping {
        id: "o1",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o1 reasoning model (o200k_base)",
    },
    ExactMapping {
        id: "o3",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o3 reasoning model (o200k_base)",
    },
    ExactMapping {
        id: "o4-mini",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o4-mini reasoning model (o200k_base)",
    },
    // OpenAI GPT next-generation models (verified o200k_base in tiktoken MODEL_TO_ENCODING)
    ExactMapping {
        id: "gpt-4o",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI GPT-4o model (o200k_base)",
    },
    ExactMapping {
        id: "gpt-4o-mini",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI GPT-4o-mini model (o200k_base)",
    },
    ExactMapping {
        id: "gpt-4.1",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI GPT-4.1 model (o200k_base)",
    },
    ExactMapping {
        id: "gpt-4.5",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI GPT-4.5 model (o200k_base)",
    },
    ExactMapping {
        id: "gpt-5",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI GPT-5 model (o200k_base)",
    },
    ExactMapping {
        id: "chatgpt-4o-latest",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI ChatGPT-4o-Latest dynamic model (o200k_base)",
    },
    // OpenAI GPT prior-generation models (verified cl100k_base in tiktoken MODEL_TO_ENCODING)
    ExactMapping {
        id: "gpt-4",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-4 model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-4-turbo",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-4-Turbo model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-4-turbo-preview",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-4-Turbo Preview model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-4-32k",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-4-32k model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-3.5-turbo",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-3.5-Turbo model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-3.5",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI GPT-3.5 model (cl100k_base)",
    },
    ExactMapping {
        id: "gpt-35-turbo",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Azure OpenAI GPT-35-Turbo deployment alias (cl100k_base)",
    },
];

static EXPLICIT_APPLICATION_ALIASES: &[ExactMapping] = &[
    ExactMapping {
        id: "fast-chat-v1",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Registered reference alias for Llama 3.3 70B Instruct (Groq)",
    },
    ExactMapping {
        id: "deep-reasoner-v1",
        family: TokenizerFamily::DeepSeek,
        encoding: None,
        rationale: "Registered reference alias for DeepSeek R1 Advanced Reasoning (OpenRouter)",
    },
    ExactMapping {
        id: "balanced-assistant-v1",
        family: TokenizerFamily::Mistral,
        encoding: None,
        rationale: "Registered reference alias for Mixtral 8x7B Instruct (Groq)",
    },
];

struct PrefixMapping {
    prefix: &'static str,
    family: TokenizerFamily,
    encoding: Option<BpeEncoding>,
    rationale: &'static str,
}

static VERIFIED_PREFIX_MAPPINGS: &[PrefixMapping] = &[
    // o200k_base prefixes (verified from tiktoken MODEL_PREFIX_TO_ENCODING)
    PrefixMapping {
        prefix: "o1-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o1- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "o3-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o3- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "o4-mini-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI o4-mini- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "gpt-4o-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI gpt-4o- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "chatgpt-4o-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI chatgpt-4o- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "gpt-4.1-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI gpt-4.1- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "gpt-4.5-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI gpt-4.5- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "gpt-5-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative OpenAI gpt-5- prefix (o200k_base)",
    },
    PrefixMapping {
        prefix: "ft:gpt-4o",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::O200k),
        rationale: "Authoritative fine-tuned ft:gpt-4o prefix (o200k_base)",
    },
    // cl100k_base prefixes (verified from tiktoken MODEL_PREFIX_TO_ENCODING)
    PrefixMapping {
        prefix: "gpt-4-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI gpt-4- prefix (cl100k_base)",
    },
    PrefixMapping {
        prefix: "gpt-3.5-turbo-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative OpenAI gpt-3.5-turbo- prefix (cl100k_base)",
    },
    PrefixMapping {
        prefix: "gpt-35-turbo-",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Azure OpenAI gpt-35-turbo- deployment prefix (cl100k_base)",
    },
    PrefixMapping {
        prefix: "ft:gpt-4",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative fine-tuned ft:gpt-4 prefix (cl100k_base)",
    },
    PrefixMapping {
        prefix: "ft:gpt-3.5-turbo",
        family: TokenizerFamily::Gpt,
        encoding: Some(BpeEncoding::Cl100k),
        rationale: "Authoritative fine-tuned ft:gpt-3.5-turbo prefix (cl100k_base)",
    },
    // Non-BPE model families (calibrated subword heuristics)
    PrefixMapping {
        prefix: "llama-",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Meta Llama family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "llama2-",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Meta Llama 2 family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "llama3-",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Meta Llama 3 family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "llama-3",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Meta Llama 3 family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "meta-llama/",
        family: TokenizerFamily::Llama,
        encoding: None,
        rationale: "Meta Llama repository prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "mistral-",
        family: TokenizerFamily::Mistral,
        encoding: None,
        rationale: "Mistral AI family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "mixtral-",
        family: TokenizerFamily::Mistral,
        encoding: None,
        rationale: "Mixtral MoE family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "mistralai/",
        family: TokenizerFamily::Mistral,
        encoding: None,
        rationale: "Mistral AI repository prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "gemma-",
        family: TokenizerFamily::Gemma,
        encoding: None,
        rationale: "Google Gemma family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "gemma2-",
        family: TokenizerFamily::Gemma,
        encoding: None,
        rationale: "Google Gemma 2 family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "google/gemma",
        family: TokenizerFamily::Gemma,
        encoding: None,
        rationale: "Google Gemma repository prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "qwen-",
        family: TokenizerFamily::Qwen,
        encoding: None,
        rationale: "Alibaba Qwen family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "qwen2",
        family: TokenizerFamily::Qwen,
        encoding: None,
        rationale: "Alibaba Qwen 2 family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "qwen/",
        family: TokenizerFamily::Qwen,
        encoding: None,
        rationale: "Alibaba Qwen repository prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "deepseek-",
        family: TokenizerFamily::DeepSeek,
        encoding: None,
        rationale: "DeepSeek family prefix (calibrated heuristic)",
    },
    PrefixMapping {
        prefix: "deepseek/",
        family: TokenizerFamily::DeepSeek,
        encoding: None,
        rationale: "DeepSeek repository prefix (calibrated heuristic)",
    },
];

/// Centralized, deterministic resolution of a model identifier to its tokenizer mapping policy.
pub fn resolve_model_tokenizer(
    model_id: &str,
    registry: Option<&crate::registry::ModelRegistry>,
) -> ModelTokenizerResolution {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return ModelTokenizerResolution {
            normalized_model_id: String::new(),
            mapping_kind: MappingKind::Unknown,
            family: TokenizerFamily::Generic,
            bpe_encoding: None,
            is_exact_raw_text_supported: false,
            rationale: "Empty model identifier; default to generic heuristic",
        };
    }

    let lower = trimmed.to_ascii_lowercase();

    // Check candidate target keys: both raw lower and stripped of provider namespace prefix
    let stripped = if let Some(rest) = lower.strip_prefix("openai/") {
        rest
    } else if let Some(rest) = lower.strip_prefix("groq/") {
        rest
    } else if let Some(rest) = lower.strip_prefix("openrouter/") {
        rest
    } else {
        &lower
    };

    // 1. Exact verified model IDs (highest precedence)
    for exact in EXACT_VERIFIED_MODELS {
        if lower == exact.id || stripped == exact.id {
            return ModelTokenizerResolution {
                normalized_model_id: lower,
                mapping_kind: MappingKind::ExactVerified,
                family: exact.family,
                bpe_encoding: exact.encoding,
                is_exact_raw_text_supported: exact.encoding.is_some(),
                rationale: exact.rationale,
            };
        }
    }

    // 2. Explicit application reference aliases
    for alias in EXPLICIT_APPLICATION_ALIASES {
        if lower == alias.id || stripped == alias.id {
            return ModelTokenizerResolution {
                normalized_model_id: lower,
                mapping_kind: MappingKind::ExplicitAlias,
                family: alias.family,
                bpe_encoding: alias.encoding,
                is_exact_raw_text_supported: alias.encoding.is_some(),
                rationale: alias.rationale,
            };
        }
    }

    // 3. Verified family prefix mappings
    for pm in VERIFIED_PREFIX_MAPPINGS {
        if lower.starts_with(pm.prefix) || stripped.starts_with(pm.prefix) {
            return ModelTokenizerResolution {
                normalized_model_id: lower,
                mapping_kind: MappingKind::FamilyPrefixVerified,
                family: pm.family,
                bpe_encoding: pm.encoding,
                is_exact_raw_text_supported: pm.encoding.is_some(),
                rationale: pm.rationale,
            };
        }
    }

    // 4. Check registry if provided for registered unverified models
    if registry.is_some_and(|reg| reg.has_model(trimmed) || reg.has_model(&lower)) {
        return ModelTokenizerResolution {
            normalized_model_id: lower,
            mapping_kind: MappingKind::RegisteredUnverified,
            family: TokenizerFamily::Generic,
            bpe_encoding: None,
            is_exact_raw_text_supported: false,
            rationale: "Registered catalog model without verified native BPE tokenizer mapping",
        };
    }

    // 5. Unknown model identifier
    ModelTokenizerResolution {
        normalized_model_id: lower,
        mapping_kind: MappingKind::Unknown,
        family: TokenizerFamily::Generic,
        bpe_encoding: None,
        is_exact_raw_text_supported: false,
        rationale: "Unrecognized model ID; falling back to conservative generic heuristic",
    }
}

impl TokenizerFamily {
    /// Returns the capability tier of this tokenizer family.
    pub fn capability(&self) -> TokenizerCapability {
        match self {
            Self::Gpt => TokenizerCapability::Exact,
            _ => TokenizerCapability::Approximate,
        }
    }

    /// Detect tokenizer family from model identifier string using the centralized mapping policy.
    pub fn from_model_id(model_id: &str) -> Self {
        resolve_model_tokenizer(model_id, None).family
    }

    /// Framing overhead in tokens per message for this tokenizer family.
    pub fn per_message_framing_tokens(&self) -> usize {
        match self {
            Self::Llama => 4,    // <|start_header_id|>role<|end_header_id|>\n\n...<|eot_id|>
            Self::Mistral => 3,  // [INST] or framing tokens
            Self::Gemma => 3,    // <start_of_turn>role\n...<end_of_turn>
            Self::Qwen => 4,     // <|im_start|>role\n...<|im_end|>\n
            Self::DeepSeek => 4, // ChatML style delimiters
            Self::Gpt => 4,      // <|im_start|>role\n...<|im_end|>\n
            Self::Generic => 4,  // Conservative default
        }
    }

    /// Generation primer overhead in tokens for this tokenizer family.
    pub fn assistant_primer_tokens(&self) -> usize {
        match self {
            Self::Llama => 4,    // <|start_header_id|>assistant<|end_header_id|>\n\n
            Self::Mistral => 2,  // [/INST] boundary
            Self::Gemma => 3,    // <start_of_turn>model\n
            Self::Qwen => 3,     // <|im_start|>assistant\n
            Self::DeepSeek => 3, // <|im_start|>assistant\n
            Self::Gpt => 3,      // <|im_start|>assistant\n
            Self::Generic => 3,  // Standard primer
        }
    }
}

/// Provenance and accuracy category of a token calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstimationMethod {
    /// Exact count reported by upstream provider usage headers/payloads.
    ExactProviderReported,
    /// Exact raw-text BPE tokenization for a string slice with verified vocabulary.
    ExactTokenizer,
    /// Model-aware chat request estimate with verified BPE message tokenization and estimated framing.
    EstimatedChatFraming,
    /// Calibrated model-family-specific subword heuristic.
    CalibratedFamilyHeuristic,
    /// Conservative fallback heuristic for unknown models.
    ConservativeFallback,
}

/// Detailed token estimation result with provenance metadata and safety margins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenEstimate {
    pub estimated_tokens: usize,
    pub method: EstimationMethod,
    pub family: TokenizerFamily,
    pub is_exact: bool,
    pub safety_margin_applied: usize,
    pub confidence_note: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text_exact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpe_variant: Option<&'static str>,
}

/// Metrics detailing token savings and compaction outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCompactionMetrics {
    pub original_characters: usize,
    pub compacted_characters: usize,
    pub bytes_saved: usize,
    pub original_estimated_tokens: usize,
    pub optimized_estimated_tokens: usize,
    pub tokens_saved: usize,
    pub duplicate_system_messages_removed: usize,
    pub compaction_applied: bool,
    pub compaction_skipped_reason: Option<String>,
}

/// Exact composite key for collision-safe LRU caching.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    text: String,
    family: TokenizerFamily,
    bpe_variant: Option<&'static str>,
}

/// Thread-safe bounded LRU cache for prompt token estimates.
struct TokenCache {
    capacity: usize,
    entries: VecDeque<(CacheKey, usize)>,
}

impl TokenCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<usize> {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == key) {
            let entry = self.entries.remove(pos)?;
            self.entries.push_front(entry);
            Some(self.entries[0].1)
        } else {
            None
        }
    }

    fn put(&mut self, key: CacheKey, count: usize) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k == &key) {
            self.entries.remove(pos);
        } else if self.entries.len() >= self.capacity {
            self.entries.pop_back();
        }
        self.entries.push_front((key, count));
    }
}

/// Global static token estimation cache (512 entries).
static TOKEN_CACHE: std::sync::LazyLock<Mutex<TokenCache>> =
    std::sync::LazyLock::new(|| Mutex::new(TokenCache::new(512)));

/// High-performance deterministic token estimation and prompt compaction engine.
pub struct TokenEstimator;

impl TokenEstimator {
    /// Resolves the model-to-tokenizer mapping using the centralized mapping policy.
    pub fn resolve_model(model_id: &str) -> ModelTokenizerResolution {
        resolve_model_tokenizer(model_id, None)
    }

    /// Resolves the model-to-tokenizer mapping with awareness of the active model registry.
    pub fn resolve_model_with_registry(
        model_id: &str,
        registry: &crate::registry::ModelRegistry,
    ) -> ModelTokenizerResolution {
        resolve_model_tokenizer(model_id, Some(registry))
    }

    /// Resolves the exact CoreBPE instance and variant name if available for the given model ID.
    pub fn select_exact_bpe(model_id: Option<&str>) -> Option<(&'static str, &'static CoreBPE)> {
        let id = model_id?;
        let resolution = Self::resolve_model(id);
        let enc = resolution.bpe_encoding?;
        let bpe = enc.get_bpe()?;
        Some((enc.name(), bpe))
    }

    /// Resolves CoreBPE for a tokenizer family if available.
    ///
    /// Note: `TokenizerFamily::Gpt` models use specific encodings (`cl100k` or `o200k`).
    /// Prefer `select_exact_bpe(Some(model_id))` for exact model-level resolution.
    pub fn bpe_for_family(family: TokenizerFamily) -> Option<(&'static str, &'static CoreBPE)> {
        match family {
            TokenizerFamily::Gpt => CL100K_BPE.as_ref().map(|b| ("cl100k", b)),
            _ => None,
        }
    }

    /// Resolves CoreBPE directly for a verified BPE encoding variant.
    pub fn bpe_for_encoding(encoding: BpeEncoding) -> Option<(&'static str, &'static CoreBPE)> {
        encoding.get_bpe().map(|b| (encoding.name(), b))
    }

    /// Exact token calculation via CoreBPE with thread-safe LRU caching.
    pub fn estimate_text_with_bpe(text: &str, variant: &'static str, bpe: &CoreBPE) -> usize {
        if text.trim().is_empty() {
            return 0;
        }

        let key = CacheKey {
            text: text.to_string(),
            family: TokenizerFamily::Gpt,
            bpe_variant: Some(variant),
        };

        if let Some(cached) = TOKEN_CACHE.lock().ok().and_then(|mut c| c.get(&key)) {
            return cached;
        }

        let tokens = bpe.encode_ordinary(text).len();

        if let Ok(mut cache) = TOKEN_CACHE.lock() {
            cache.put(key, tokens);
        }

        tokens
    }

    /// Estimate token count for a raw string slice using generic heuristic with LRU caching.
    pub fn estimate_text(text: &str) -> usize {
        Self::estimate_text_for_family(text, TokenizerFamily::Generic)
    }

    /// Estimate token count for a raw string slice targeting a specific model identifier.
    pub fn estimate_text_for_model(text: &str, model_id: Option<&str>) -> usize {
        if let Some((variant, bpe)) = Self::select_exact_bpe(model_id) {
            return Self::estimate_text_with_bpe(text, variant, bpe);
        }
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        Self::estimate_text_for_family(text, family)
    }

    /// Estimate token count for a raw string slice targeting a specific tokenizer family.
    pub fn estimate_text_for_family(text: &str, family: TokenizerFamily) -> usize {
        if text.trim().is_empty() {
            return 0;
        }

        if let Some((variant, bpe)) = Self::bpe_for_family(family) {
            return Self::estimate_text_with_bpe(text, variant, bpe);
        }

        let key = CacheKey {
            text: text.to_string(),
            family,
            bpe_variant: None,
        };

        if let Some(cached) = TOKEN_CACHE.lock().ok().and_then(|mut c| c.get(&key)) {
            return cached;
        }

        let tokens = Self::compute_text_tokens(text, family);

        if let Ok(mut cache) = TOKEN_CACHE.lock() {
            cache.put(key, tokens);
        }

        tokens
    }

    /// Pure computation of token count for text without cache interaction.
    fn compute_text_tokens(text: &str, family: TokenizerFamily) -> usize {
        if text.trim().is_empty() {
            return 0;
        }

        let mut tokens = 0;
        let mut current_word_len = 0;
        let mut non_ascii_count: usize = 0;
        let mut consecutive_newlines = 0;
        let mut consecutive_spaces = 0;

        for c in text.chars() {
            if c.is_ascii() {
                if c == '\n' {
                    if current_word_len > 0 {
                        tokens += Self::word_tokens(current_word_len, family);
                        current_word_len = 0;
                    }
                    consecutive_spaces = 0;
                    consecutive_newlines += 1;
                    if consecutive_newlines > 1 {
                        tokens += 1; // Extra newlines in formatting take tokens
                    }
                } else if c.is_ascii_whitespace() {
                    if current_word_len > 0 {
                        tokens += Self::word_tokens(current_word_len, family);
                        current_word_len = 0;
                    }
                    consecutive_newlines = 0;
                    consecutive_spaces += 1;
                    if consecutive_spaces >= 4 {
                        tokens += 1; // Indentation block (~4 spaces per token)
                        consecutive_spaces = 0;
                    }
                } else if c.is_ascii_punctuation() {
                    if current_word_len > 0 {
                        tokens += Self::word_tokens(current_word_len, family);
                        current_word_len = 0;
                    }
                    consecutive_newlines = 0;
                    consecutive_spaces = 0;
                    tokens += 1;
                } else {
                    consecutive_newlines = 0;
                    consecutive_spaces = 0;
                    current_word_len += 1;
                }
            } else {
                if current_word_len > 0 {
                    tokens += Self::word_tokens(current_word_len, family);
                    current_word_len = 0;
                }
                consecutive_newlines = 0;
                consecutive_spaces = 0;
                non_ascii_count += 1;
            }
        }

        if current_word_len > 0 {
            tokens += Self::word_tokens(current_word_len, family);
        }

        // Multilingual & Unicode token calibration:
        // Distinguish scripts by vocabulary density:
        // Qwen and DeepSeek feature massive multilingual vocabularies (150k+ tokens).
        // Llama 3 has a 128k BPE vocabulary with good Urdu/Arabic/Devanagari morpheme compression.
        // Gemma 2 features a 256k vocabulary with high Unicode subword coverage.
        if non_ascii_count > 0 {
            let divisor = match family {
                TokenizerFamily::Qwen | TokenizerFamily::DeepSeek => 3,
                TokenizerFamily::Gemma => 3,
                _ => 2,
            };
            tokens += non_ascii_count.div_ceil(divisor);
        }

        tokens.max(1)
    }

    #[inline]
    fn word_tokens(len: usize, family: TokenizerFamily) -> usize {
        let max_single_len = match family {
            TokenizerFamily::Gemma => 7, // Gemma splits long tokens slightly more aggressively
            _ => 8,
        };

        if len <= max_single_len {
            1
        } else {
            1 + (len - max_single_len).div_ceil(4)
        }
    }

    /// Estimate token overhead for an individual ChatMessage including role framing.
    pub fn estimate_message(msg: &ChatMessage) -> usize {
        Self::estimate_message_for_family(msg, TokenizerFamily::Generic)
    }

    /// Estimate token overhead for an individual ChatMessage with a specific tokenizer family.
    pub fn estimate_message_for_family(msg: &ChatMessage, family: TokenizerFamily) -> usize {
        if let Some((variant, bpe)) = Self::bpe_for_family(family) {
            let role_tokens = Self::estimate_text_with_bpe(&msg.role, variant, bpe);
            let content_tokens = Self::estimate_text_with_bpe(&msg.content, variant, bpe);
            3 + role_tokens + content_tokens
        } else {
            let content_tokens = Self::estimate_text_for_family(&msg.content, family);
            let role_tokens = Self::estimate_text_for_family(&msg.role, family);
            content_tokens + role_tokens + family.per_message_framing_tokens()
        }
    }

    /// Estimate token overhead for an individual ChatMessage targeting a specific model ID.
    pub fn estimate_message_for_model(msg: &ChatMessage, model_id: Option<&str>) -> usize {
        if let Some((variant, bpe)) = Self::select_exact_bpe(model_id) {
            let role_tokens = Self::estimate_text_with_bpe(&msg.role, variant, bpe);
            let content_tokens = Self::estimate_text_with_bpe(&msg.content, variant, bpe);
            3 + role_tokens + content_tokens
        } else {
            let family = model_id
                .map(TokenizerFamily::from_model_id)
                .unwrap_or(TokenizerFamily::Generic);
            Self::estimate_message_for_family(msg, family)
        }
    }

    /// Estimate token count for a full chat conversation history including assistant response primer.
    pub fn estimate_chat(messages: &[ChatMessage]) -> usize {
        Self::estimate_chat_for_family(messages, TokenizerFamily::Generic)
    }

    /// Estimate token count for a full chat conversation history for a specific model ID.
    pub fn estimate_chat_for_model(messages: &[ChatMessage], model_id: Option<&str>) -> usize {
        if messages.is_empty() {
            return 0;
        }

        if let Some((variant, bpe)) = Self::select_exact_bpe(model_id) {
            let mut total = 0;
            for msg in messages {
                let role_tokens = Self::estimate_text_with_bpe(&msg.role, variant, bpe);
                let content_tokens = Self::estimate_text_with_bpe(&msg.content, variant, bpe);
                total += 3 + role_tokens + content_tokens;
            }
            total + 3 // OpenAI assistant response primer
        } else {
            let family = model_id
                .map(TokenizerFamily::from_model_id)
                .unwrap_or(TokenizerFamily::Generic);
            Self::estimate_chat_for_family(messages, family)
        }
    }

    /// Estimate token count for a full chat conversation history targeting a specific tokenizer family.
    pub fn estimate_chat_for_family(messages: &[ChatMessage], family: TokenizerFamily) -> usize {
        if messages.is_empty() {
            return 0;
        }

        if let Some((variant, bpe)) = Self::bpe_for_family(family) {
            let mut total = 0;
            for msg in messages {
                let role_tokens = Self::estimate_text_with_bpe(&msg.role, variant, bpe);
                let content_tokens = Self::estimate_text_with_bpe(&msg.content, variant, bpe);
                total += 3 + role_tokens + content_tokens;
            }
            return total + 3;
        }

        let mut total = 0;
        for msg in messages {
            total += Self::estimate_message_for_family(msg, family);
        }

        total + family.assistant_primer_tokens()
    }

    /// Estimate token count for a raw string slice with full provenance metadata.
    pub fn estimate_text_detailed(text: &str, model_id: Option<&str>) -> TokenEstimate {
        if let Some(id) = model_id {
            let res = Self::resolve_model(id);
            if let Some((enc, bpe)) = res.bpe_encoding.and_then(|e| e.get_bpe().map(|b| (e, b))) {
                let count = Self::estimate_text_with_bpe(text, enc.name(), bpe);
                return TokenEstimate {
                    estimated_tokens: count,
                    method: EstimationMethod::ExactTokenizer,
                    family: TokenizerFamily::Gpt,
                    is_exact: true,
                    safety_margin_applied: 0,
                    confidence_note: "Exact BPE tokenizer count using verified model vocabulary",
                    raw_text_exact: Some(true),
                    bpe_variant: Some(enc.name()),
                };
            }

            let family = res.family;
            let count = Self::estimate_text_for_family(text, family);
            let method = match family {
                TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
                _ => EstimationMethod::CalibratedFamilyHeuristic,
            };
            return TokenEstimate {
                estimated_tokens: count,
                method,
                family,
                is_exact: false,
                safety_margin_applied: 0,
                confidence_note: match method {
                    EstimationMethod::ConservativeFallback => {
                        "Conservative heuristic fallback for uncalibrated/generic model"
                    }
                    EstimationMethod::CalibratedFamilyHeuristic => {
                        "Calibrated BPE/SentencePiece vocabulary heuristic for model family"
                    }
                    _ => "Approximate token count",
                },
                raw_text_exact: Some(false),
                bpe_variant: None,
            };
        }

        let family = TokenizerFamily::Generic;
        let count = Self::estimate_text_for_family(text, family);
        TokenEstimate {
            estimated_tokens: count,
            method: EstimationMethod::ConservativeFallback,
            family,
            is_exact: false,
            safety_margin_applied: 0,
            confidence_note: "Conservative heuristic fallback for uncalibrated/generic model",
            raw_text_exact: Some(false),
            bpe_variant: None,
        }
    }

    /// Estimate token count for chat messages with full provenance metadata.
    pub fn estimate_chat_detailed(
        messages: &[ChatMessage],
        model_id: Option<&str>,
    ) -> TokenEstimate {
        if let Some(id) = model_id {
            let res = Self::resolve_model(id);
            if let Some((enc, bpe)) = res.bpe_encoding.and_then(|e| e.get_bpe().map(|b| (e, b))) {
                let count = if messages.is_empty() {
                    0
                } else {
                    let mut total = 0;
                    for msg in messages {
                        let role_tokens = Self::estimate_text_with_bpe(&msg.role, enc.name(), bpe);
                        let content_tokens =
                            Self::estimate_text_with_bpe(&msg.content, enc.name(), bpe);
                        total += 3 + role_tokens + content_tokens;
                    }
                    total + 3 // OpenAI assistant primer
                };
                return TokenEstimate {
                    estimated_tokens: count,
                    method: EstimationMethod::EstimatedChatFraming,
                    family: TokenizerFamily::Gpt,
                    is_exact: false, // Request-level framing is an estimate, not exact raw tokenization
                    safety_margin_applied: 0,
                    confidence_note: "Chat request estimate using verified BPE message tokenization with estimated framing overhead",
                    raw_text_exact: Some(true),
                    bpe_variant: Some(enc.name()),
                };
            }

            let family = res.family;
            let base_count = Self::estimate_chat_for_family(messages, family);
            let method = match family {
                TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
                _ => EstimationMethod::CalibratedFamilyHeuristic,
            };
            return TokenEstimate {
                estimated_tokens: base_count,
                method,
                family,
                is_exact: false,
                safety_margin_applied: 0,
                confidence_note: match method {
                    EstimationMethod::ConservativeFallback => {
                        "Conservative heuristic fallback for uncalibrated/generic model"
                    }
                    EstimationMethod::CalibratedFamilyHeuristic => {
                        "Calibrated BPE/SentencePiece vocabulary heuristic for model family"
                    }
                    _ => "Approximate token count",
                },
                raw_text_exact: Some(false),
                bpe_variant: None,
            };
        }

        let family = TokenizerFamily::Generic;
        let base_count = Self::estimate_chat_for_family(messages, family);
        TokenEstimate {
            estimated_tokens: base_count,
            method: EstimationMethod::ConservativeFallback,
            family,
            is_exact: false,
            safety_margin_applied: 0,
            confidence_note: "Conservative heuristic fallback for uncalibrated/generic model",
            raw_text_exact: Some(false),
            bpe_variant: None,
        }
    }

    /// Estimate token overhead for tool / function schema definitions targeting a specific family.
    pub fn estimate_tools_tokens(tools_json: &str, family: TokenizerFamily) -> usize {
        let trimmed = tools_json.trim();
        if trimmed.is_empty() {
            return 0;
        }
        let schema_tokens = Self::estimate_text_for_family(trimmed, family);
        schema_tokens + 12 // Schema framing overhead
    }

    /// Estimate token overhead for tool / function schema definitions targeting a specific model.
    pub fn estimate_tools_tokens_for_model(tools_json: &str, model_id: Option<&str>) -> usize {
        let trimmed = tools_json.trim();
        if trimmed.is_empty() {
            return 0;
        }
        let schema_tokens = Self::estimate_text_for_model(trimmed, model_id);
        schema_tokens + 12 // Schema framing overhead
    }

    /// Estimates full effective request context including messages, tool schemas, and safety buffer.
    pub fn estimate_request_context_safe(
        messages: &[ChatMessage],
        tools_json: Option<&str>,
        model_id: Option<&str>,
    ) -> TokenEstimate {
        if let Some(id) = model_id {
            let res = Self::resolve_model(id);
            if let Some((enc, bpe)) = res.bpe_encoding.and_then(|e| e.get_bpe().map(|b| (e, b))) {
                let chat_tokens = if messages.is_empty() {
                    0
                } else {
                    let mut total = 0;
                    for msg in messages {
                        let role_tokens = Self::estimate_text_with_bpe(&msg.role, enc.name(), bpe);
                        let content_tokens =
                            Self::estimate_text_with_bpe(&msg.content, enc.name(), bpe);
                        total += 3 + role_tokens + content_tokens;
                    }
                    total + 3
                };
                let tools_tokens = tools_json
                    .and_then(|t| {
                        let trimmed = t.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(Self::estimate_text_with_bpe(trimmed, enc.name(), bpe) + 12)
                        }
                    })
                    .unwrap_or(0);
                let exact_total = chat_tokens + tools_tokens;

                return TokenEstimate {
                    estimated_tokens: exact_total,
                    method: EstimationMethod::EstimatedChatFraming,
                    family: TokenizerFamily::Gpt,
                    is_exact: false, // Context budget calculation incorporating framing is an estimate
                    safety_margin_applied: 0,
                    confidence_note: "Context budget estimate using verified BPE tokenization with estimated message and schema framing",
                    raw_text_exact: Some(true),
                    bpe_variant: Some(enc.name()),
                };
            }

            let family = res.family;
            let chat_tokens = Self::estimate_chat_for_family(messages, family);
            let tools_tokens = tools_json
                .and_then(|t| {
                    let trimmed = t.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(Self::estimate_tools_tokens(trimmed, family))
                    }
                })
                .unwrap_or(0);
            let raw_total = chat_tokens + tools_tokens;

            // Apply conservative safety margin (15% or minimum 8 tokens) for heuristic estimators
            // Note: Heuristic estimation cannot provide a universal overflow guarantee for arbitrary high-entropy inputs.
            // This margin provides headroom against subword underestimation while avoiding premature context exhaustion.
            let safety_margin = (raw_total.saturating_mul(15) / 100).max(8);
            let safe_total = raw_total.saturating_add(safety_margin);

            let method = match family {
                TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
                _ => EstimationMethod::CalibratedFamilyHeuristic,
            };

            return TokenEstimate {
                estimated_tokens: safe_total,
                method,
                family,
                is_exact: false,
                safety_margin_applied: safety_margin,
                confidence_note: "Conservative context budget estimate with 15% safety headroom (approximate)",
                raw_text_exact: Some(false),
                bpe_variant: None,
            };
        }

        let family = TokenizerFamily::Generic;
        let chat_tokens = Self::estimate_chat_for_family(messages, family);
        let tools_tokens = tools_json
            .and_then(|t| {
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(Self::estimate_tools_tokens(trimmed, family))
                }
            })
            .unwrap_or(0);
        let raw_total = chat_tokens + tools_tokens;

        let safety_margin = (raw_total.saturating_mul(15) / 100).max(8);
        let safe_total = raw_total.saturating_add(safety_margin);

        TokenEstimate {
            estimated_tokens: safe_total,
            method: EstimationMethod::ConservativeFallback,
            family,
            is_exact: false,
            safety_margin_applied: safety_margin,
            confidence_note: "Conservative heuristic fallback for uncalibrated/generic model with 15% safety headroom",
            raw_text_exact: Some(false),
            bpe_variant: None,
        }
    }

    /// Compacts messages by deduplicating consecutive identical system messages
    /// and safely normalizing whitespace without altering code indentation,
    /// markdown code blocks, or structured data semantics.
    pub fn optimize_messages(
        messages: &[ChatMessage],
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        Self::optimize_messages_for_model(messages, None)
    }

    /// Compacts messages targeting a specific model identifier.
    pub fn optimize_messages_for_model(
        messages: &[ChatMessage],
        model_id: Option<&str>,
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        let original_tokens = Self::estimate_chat_for_model(messages, model_id);
        let original_characters: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        let original_bytes: usize = messages.iter().map(|m| m.content.len()).sum();

        if messages.is_empty() {
            return (
                Vec::new(),
                TokenCompactionMetrics {
                    original_characters: 0,
                    compacted_characters: 0,
                    bytes_saved: 0,
                    original_estimated_tokens: 0,
                    optimized_estimated_tokens: 0,
                    tokens_saved: 0,
                    duplicate_system_messages_removed: 0,
                    compaction_applied: false,
                    compaction_skipped_reason: Some("Empty messages array".to_string()),
                },
            );
        }

        let mut optimized: Vec<ChatMessage> = Vec::with_capacity(messages.len());
        let mut duplicate_system_count = 0;

        for msg in messages {
            // Check for consecutive identical system instructions
            let is_duplicate_system = msg.role.eq_ignore_ascii_case("system")
                && optimized.last().is_some_and(|last| {
                    last.role.eq_ignore_ascii_case("system")
                        && last.content.trim() == msg.content.trim()
                });

            if is_duplicate_system {
                duplicate_system_count += 1;
                continue;
            }

            // Perform conservative whitespace normalization:
            // Crucial: DO NOT strip leading horizontal whitespace from lines that could be code indentation!
            let compacted_content = Self::compact_content_conservative(&msg.content);

            optimized.push(ChatMessage {
                role: msg.role.clone(),
                content: compacted_content,
            });
        }

        let optimized_tokens = Self::estimate_chat_for_model(&optimized, model_id);
        let compacted_characters: usize = optimized.iter().map(|m| m.content.chars().count()).sum();
        let compacted_bytes: usize = optimized.iter().map(|m| m.content.len()).sum();

        let bytes_saved = original_bytes.saturating_sub(compacted_bytes);
        let tokens_saved = original_tokens.saturating_sub(optimized_tokens);
        let compaction_applied = duplicate_system_count > 0 || bytes_saved > 0;

        let compaction_skipped_reason = if !compaction_applied {
            Some("Content already optimal with no redundant whitespace or duplicates".to_string())
        } else {
            None
        };

        (
            optimized,
            TokenCompactionMetrics {
                original_characters,
                compacted_characters,
                bytes_saved,
                original_estimated_tokens: original_tokens,
                optimized_estimated_tokens: optimized_tokens,
                tokens_saved,
                duplicate_system_messages_removed: duplicate_system_count,
                compaction_applied,
                compaction_skipped_reason,
            },
        )
    }

    /// Compacts messages targeting a specific tokenizer family.
    pub fn optimize_messages_for_family(
        messages: &[ChatMessage],
        family: TokenizerFamily,
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        let model_id = match family {
            TokenizerFamily::Gpt => Some("gpt-4"),
            TokenizerFamily::Llama => Some("llama-3"),
            TokenizerFamily::Mistral => Some("mistral"),
            TokenizerFamily::Gemma => Some("gemma"),
            TokenizerFamily::Qwen => Some("qwen"),
            TokenizerFamily::DeepSeek => Some("deepseek"),
            TokenizerFamily::Generic => None,
        };
        Self::optimize_messages_for_model(messages, model_id)
    }

    /// Conservatively compacts text content while preserving code indentation,
    /// markdown fences, YAML/JSON structure, and significant whitespace.
    fn compact_content_conservative(content: &str) -> String {
        if content.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        let mut in_code_fence = false;
        let mut consecutive_blank_lines = 0;

        for raw_line in content.lines() {
            let trimmed_line = raw_line.trim_start();
            if trimmed_line.starts_with("```") || trimmed_line.starts_with("~~~") {
                in_code_fence = !in_code_fence;
            }

            if in_code_fence {
                // Inside code blocks, preserve line verbatim (including leading and trailing whitespace)
                lines.push(raw_line.to_string());
                consecutive_blank_lines = 0;
            } else if raw_line.trim().is_empty() {
                // Blank line outside code block: allow at most one consecutive blank line
                consecutive_blank_lines += 1;
                if consecutive_blank_lines <= 1 {
                    lines.push(String::new());
                }
            } else {
                consecutive_blank_lines = 0;
                // Preserve leading indentation (spaces/tabs), but safely strip trailing whitespace
                let trimmed_end = raw_line.trim_end();
                lines.push(trimmed_end.to_string());
            }
        }

        // Drop leading and trailing empty lines from the message
        while lines.first().is_some_and(|l| l.is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_text_empty_and_whitespace() {
        assert_eq!(TokenEstimator::estimate_text(""), 0);
        assert_eq!(TokenEstimator::estimate_text("   \n\t "), 0);
    }

    #[test]
    fn test_estimate_text_basic_english() {
        let tokens = TokenEstimator::estimate_text("Hello world");
        assert_eq!(tokens, 2);

        let code_tokens = TokenEstimator::estimate_text(
            "Write a high-performance memory allocator in Rust using std::alloc.",
        );
        assert!((10..=20).contains(&code_tokens));
    }

    #[test]
    fn test_estimate_text_multilingual_unicode() {
        let hindi_tokens = TokenEstimator::estimate_text("नमस्ते, आप कैसे हैं?");
        assert!(hindi_tokens >= 4);

        let chinese_tokens = TokenEstimator::estimate_text("这是一个测试消息");
        assert!(chinese_tokens >= 4);
    }

    #[test]
    fn test_estimate_chat_framing() {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Hi".to_string(),
            },
        ];
        let total = TokenEstimator::estimate_chat(&msgs);
        assert!(total > 10);
    }

    #[test]
    fn test_tokenizer_family_detection() {
        assert_eq!(
            TokenizerFamily::from_model_id("llama-3.3-70b-versatile"),
            TokenizerFamily::Llama
        );
        assert_eq!(
            TokenizerFamily::from_model_id("mixtral-8x7b-32768"),
            TokenizerFamily::Mistral
        );
        assert_eq!(
            TokenizerFamily::from_model_id("gemma2-9b-it"),
            TokenizerFamily::Gemma
        );
        assert_eq!(
            TokenizerFamily::from_model_id("qwen/qwen3.8-27b"),
            TokenizerFamily::Qwen
        );
        assert_eq!(
            TokenizerFamily::from_model_id("deepseek/deepseek-r1"),
            TokenizerFamily::DeepSeek
        );
        assert_eq!(
            TokenizerFamily::from_model_id("gpt-4o-mini"),
            TokenizerFamily::Gpt
        );
        assert_eq!(
            TokenizerFamily::from_model_id("unknown-custom-model"),
            TokenizerFamily::Generic
        );
    }

    #[test]
    fn test_conservative_compaction_preserves_code_indentation() {
        let python_code = "    def calculate_total(items):\n        total = 0   \n        for item in items:\n            total += item.price\n        return total\n";
        let compacted = TokenEstimator::compact_content_conservative(python_code);

        // Assert leading 4 spaces and 8 spaces are preserved
        assert!(compacted.starts_with("    def calculate_total(items):"));
        assert!(compacted.contains("        total = 0"));
        assert!(compacted.contains("            total += item.price"));
        // Assert trailing spaces on `total = 0   ` were stripped
        assert!(compacted.contains("        total = 0\n"));
    }

    #[test]
    fn test_conservative_compaction_preserves_code_blocks() {
        let markdown =
            "# Title\n\n```python\n    def foo():\n        return True\n```\n\n\n\nEnd text.";
        let compacted = TokenEstimator::compact_content_conservative(markdown);
        assert!(compacted.contains("```python\n    def foo():\n        return True\n```"));
        // 4 blank lines collapsed to at most one blank line (\n\n)
        assert!(compacted.contains("```\n\nEnd text."));
    }

    #[test]
    fn test_tiktoken_availability() {
        let text = "fn compute(x: i32) -> i32 { x * 2 + 1 }";
        let cl100k = tiktoken_rs::cl100k_base().expect("cl100k_base should load");
        assert_eq!(cl100k.encode_ordinary(text).len(), 19);

        let o200k = tiktoken_rs::o200k_base().expect("o200k_base should load");
        assert_eq!(o200k.encode_ordinary(text).len(), 19);

        // Verify select_exact_bpe
        let (v1, _) = TokenEstimator::select_exact_bpe(Some("gpt-4")).unwrap();
        assert_eq!(v1, "cl100k");
        let (v2, _) = TokenEstimator::select_exact_bpe(Some("gpt-4o")).unwrap();
        assert_eq!(v2, "o200k");
    }

    #[test]
    fn test_resolve_model_tokenizer_exact_and_next_gen() {
        // Next-generation and reasoning models mapped to o200k_base
        let cases = [
            ("o1", "o200k", TokenizerFamily::Gpt),
            ("o3", "o200k", TokenizerFamily::Gpt),
            ("o4-mini", "o200k", TokenizerFamily::Gpt),
            ("gpt-4o", "o200k", TokenizerFamily::Gpt),
            ("gpt-4o-mini", "o200k", TokenizerFamily::Gpt),
            ("gpt-4.1", "o200k", TokenizerFamily::Gpt),
            ("gpt-4.5", "o200k", TokenizerFamily::Gpt),
            ("gpt-5", "o200k", TokenizerFamily::Gpt),
            ("chatgpt-4o-latest", "o200k", TokenizerFamily::Gpt),
        ];

        for (id, expected_enc, expected_fam) in cases {
            let res = TokenEstimator::resolve_model(id);
            assert_eq!(res.mapping_kind, MappingKind::ExactVerified, "Model {id}");
            assert_eq!(res.family, expected_fam, "Model {id}");
            assert_eq!(
                res.bpe_encoding.map(|e| e.name()),
                Some(expected_enc),
                "Model {id}"
            );
            assert!(res.is_exact_raw_text_supported, "Model {id}");
        }

        // Prior-generation models mapped to cl100k_base
        let cl100k_cases = [
            ("gpt-4", "cl100k", TokenizerFamily::Gpt),
            ("gpt-4-turbo", "cl100k", TokenizerFamily::Gpt),
            ("gpt-4-32k", "cl100k", TokenizerFamily::Gpt),
            ("gpt-3.5-turbo", "cl100k", TokenizerFamily::Gpt),
            ("gpt-3.5", "cl100k", TokenizerFamily::Gpt),
            ("gpt-35-turbo", "cl100k", TokenizerFamily::Gpt),
        ];

        for (id, expected_enc, expected_fam) in cl100k_cases {
            let res = TokenEstimator::resolve_model(id);
            assert_eq!(res.mapping_kind, MappingKind::ExactVerified, "Model {id}");
            assert_eq!(res.family, expected_fam, "Model {id}");
            assert_eq!(
                res.bpe_encoding.map(|e| e.name()),
                Some(expected_enc),
                "Model {id}"
            );
            assert!(res.is_exact_raw_text_supported, "Model {id}");
        }
    }

    #[test]
    fn test_resolve_model_tokenizer_prefixes() {
        let prefix_cases = [
            ("gpt-4.1-mini", "o200k"),
            ("gpt-4.1-nano", "o200k"),
            ("gpt-4.5-preview-2025-02-27", "o200k"),
            ("gpt-5-mini", "o200k"),
            ("gpt-5-nano", "o200k"),
            ("o1-preview-2024-09-12", "o200k"),
            ("o1-mini-2024-09-12", "o200k"),
            ("o3-mini-2025-01-31", "o200k"),
            ("gpt-4o-2024-08-06", "o200k"),
            ("gpt-4o-mini-2024-07-18", "o200k"),
            ("gpt-4-0613", "cl100k"),
            ("gpt-4-turbo-2024-04-09", "cl100k"),
            ("gpt-3.5-turbo-0125", "cl100k"),
        ];

        for (id, expected_enc) in prefix_cases {
            let res = TokenEstimator::resolve_model(id);
            assert_eq!(
                res.mapping_kind,
                MappingKind::FamilyPrefixVerified,
                "Model {id}"
            );
            assert_eq!(res.family, TokenizerFamily::Gpt, "Model {id}");
            assert_eq!(
                res.bpe_encoding.map(|e| e.name()),
                Some(expected_enc),
                "Model {id}"
            );
            assert!(res.is_exact_raw_text_supported, "Model {id}");
        }
    }

    #[test]
    fn test_resolve_model_tokenizer_negative_lookalikes() {
        let negative_cases = [
            "auto1",
            "echo1",
            "audio3",
            "demo3",
            "my-custom-gpt",
            "custom-gpt-model",
            "bogus-o1-model",
            "unregistered-llm-v1",
            "",
            "   \t  ",
        ];

        for id in negative_cases {
            let res = TokenEstimator::resolve_model(id);
            assert_eq!(res.mapping_kind, MappingKind::Unknown, "Input '{id}'");
            assert_eq!(res.family, TokenizerFamily::Generic, "Input '{id}'");
            assert_eq!(res.bpe_encoding, None, "Input '{id}'");
            assert!(
                !res.is_exact_raw_text_supported,
                "Input '{id}' must not claim exact BPE"
            );
            assert!(
                TokenEstimator::select_exact_bpe(Some(id)).is_none(),
                "Input '{id}' must return None from select_exact_bpe"
            );
        }
    }

    #[test]
    fn test_resolve_model_tokenizer_provider_namespaces() {
        let res = TokenEstimator::resolve_model("openai/gpt-4o");
        assert_eq!(res.mapping_kind, MappingKind::ExactVerified);
        assert_eq!(res.bpe_encoding.map(|e| e.name()), Some("o200k"));

        let res = TokenEstimator::resolve_model("groq/fast-chat-v1");
        assert_eq!(res.mapping_kind, MappingKind::ExplicitAlias);
        assert_eq!(res.family, TokenizerFamily::Llama);

        let res = TokenEstimator::resolve_model("openrouter/deepseek/deepseek-r1");
        assert_eq!(res.mapping_kind, MappingKind::FamilyPrefixVerified);
        assert_eq!(res.family, TokenizerFamily::DeepSeek);
    }

    #[test]
    fn test_exact_raw_text_vs_estimated_chat_semantics() {
        let text = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let raw_est = TokenEstimator::estimate_text_detailed(text, Some("gpt-4o"));
        assert!(raw_est.is_exact);
        assert_eq!(raw_est.method, EstimationMethod::ExactTokenizer);
        assert_eq!(raw_est.raw_text_exact, Some(true));
        assert_eq!(raw_est.bpe_variant, Some("o200k"));

        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: text.to_string(),
        }];
        let chat_est = TokenEstimator::estimate_chat_detailed(&msgs, Some("gpt-4o"));
        assert!(
            !chat_est.is_exact,
            "Chat request framing must be classified as an estimate"
        );
        assert_eq!(chat_est.method, EstimationMethod::EstimatedChatFraming);
        assert_eq!(chat_est.raw_text_exact, Some(true));
        assert_eq!(chat_est.bpe_variant, Some("o200k"));

        let safe_est = TokenEstimator::estimate_request_context_safe(&msgs, None, Some("gpt-4o"));
        assert!(
            !safe_est.is_exact,
            "Context safe budget must be classified as an estimate"
        );
        assert_eq!(safe_est.method, EstimationMethod::EstimatedChatFraming);
        assert_eq!(safe_est.raw_text_exact, Some(true));
    }
}
