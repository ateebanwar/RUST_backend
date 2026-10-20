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
    /// OpenAI GPT-4 / GPT-4o / o1 family (100k-200k tiktoken vocabulary).
    Gpt,
    /// Fallback conservative generic tokenizer heuristic.
    Generic,
}

impl TokenizerFamily {
    /// Detect tokenizer family from model identifier string.
    pub fn from_model_id(model_id: &str) -> Self {
        let lower = model_id.to_ascii_lowercase();
        if lower.contains("llama") {
            Self::Llama
        } else if lower.contains("mistral") || lower.contains("mixtral") {
            Self::Mistral
        } else if lower.contains("gemma") {
            Self::Gemma
        } else if lower.contains("qwen") {
            Self::Qwen
        } else if lower.contains("deepseek") || lower.contains("deep-reasoner") {
            Self::DeepSeek
        } else if lower.contains("gpt") || lower.contains("o1") || lower.contains("o3") {
            Self::Gpt
        } else {
            Self::Generic
        }
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
    /// Exact tokenizer matching.
    ExactTokenizer,
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
    /// Estimate token count for a raw string slice using generic heuristic with LRU caching.
    pub fn estimate_text(text: &str) -> usize {
        Self::estimate_text_for_family(text, TokenizerFamily::Generic)
    }

    /// Estimate token count for a raw string slice targeting a specific model identifier.
    pub fn estimate_text_for_model(text: &str, model_id: Option<&str>) -> usize {
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

        let key = CacheKey {
            text: text.to_string(),
            family,
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
        let content_tokens = Self::estimate_text_for_family(&msg.content, family);
        let role_tokens = Self::estimate_text_for_family(&msg.role, family);
        content_tokens + role_tokens + family.per_message_framing_tokens()
    }

    /// Estimate token count for a full chat conversation history including assistant response primer.
    pub fn estimate_chat(messages: &[ChatMessage]) -> usize {
        Self::estimate_chat_for_family(messages, TokenizerFamily::Generic)
    }

    /// Estimate token count for a full chat conversation history for a specific model ID.
    pub fn estimate_chat_for_model(messages: &[ChatMessage], model_id: Option<&str>) -> usize {
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        Self::estimate_chat_for_family(messages, family)
    }

    /// Estimate token count for a full chat conversation history targeting a specific tokenizer family.
    pub fn estimate_chat_for_family(messages: &[ChatMessage], family: TokenizerFamily) -> usize {
        if messages.is_empty() {
            return 0;
        }

        let mut total = 0;
        for msg in messages {
            total += Self::estimate_message_for_family(msg, family);
        }

        total + family.assistant_primer_tokens()
    }

    /// Estimate token count for a raw string slice with full provenance metadata.
    pub fn estimate_text_detailed(text: &str, model_id: Option<&str>) -> TokenEstimate {
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        let count = Self::estimate_text_for_family(text, family);
        let method = match family {
            TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
            _ => EstimationMethod::CalibratedFamilyHeuristic,
        };
        TokenEstimate {
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
                _ => "Exact token count",
            },
        }
    }

    /// Estimate token count for chat messages with full provenance metadata.
    pub fn estimate_chat_detailed(
        messages: &[ChatMessage],
        model_id: Option<&str>,
    ) -> TokenEstimate {
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        let base_count = Self::estimate_chat_for_family(messages, family);
        let method = match family {
            TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
            _ => EstimationMethod::CalibratedFamilyHeuristic,
        };
        TokenEstimate {
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
                _ => "Exact token count",
            },
        }
    }

    /// Estimate token overhead for tool / function schema definitions.
    pub fn estimate_tools_tokens(tools_json: &str, family: TokenizerFamily) -> usize {
        if tools_json.trim().is_empty() {
            return 0;
        }
        let schema_tokens = Self::estimate_text_for_family(tools_json, family);
        schema_tokens + 12 // Schema framing overhead
    }

    /// Estimates full effective request context including messages, tool schemas, and safety buffer.
    pub fn estimate_request_context_safe(
        messages: &[ChatMessage],
        tools_json: Option<&str>,
        model_id: Option<&str>,
    ) -> TokenEstimate {
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        let chat_tokens = Self::estimate_chat_for_family(messages, family);
        let tools_tokens = tools_json
            .map(|t| Self::estimate_tools_tokens(t, family))
            .unwrap_or(0);
        let raw_total = chat_tokens + tools_tokens;

        // Apply conservative safety margin (5% or minimum 4 tokens) for heuristic estimators
        let safety_margin = (raw_total / 20).max(4);
        let safe_total = raw_total.saturating_add(safety_margin);

        let method = match family {
            TokenizerFamily::Generic => EstimationMethod::ConservativeFallback,
            _ => EstimationMethod::CalibratedFamilyHeuristic,
        };

        TokenEstimate {
            estimated_tokens: safe_total,
            method,
            family,
            is_exact: false,
            safety_margin_applied: safety_margin,
            confidence_note: "Safe context budget estimate with conservative margin",
        }
    }

    /// Compacts messages by deduplicating consecutive identical system messages
    /// and safely normalizing whitespace without altering code indentation,
    /// markdown code blocks, or structured data semantics.
    pub fn optimize_messages(
        messages: &[ChatMessage],
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        Self::optimize_messages_for_family(messages, TokenizerFamily::Generic)
    }

    /// Compacts messages targeting a specific model identifier.
    pub fn optimize_messages_for_model(
        messages: &[ChatMessage],
        model_id: Option<&str>,
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        let family = model_id
            .map(TokenizerFamily::from_model_id)
            .unwrap_or(TokenizerFamily::Generic);
        Self::optimize_messages_for_family(messages, family)
    }

    /// Compacts messages targeting a specific tokenizer family.
    pub fn optimize_messages_for_family(
        messages: &[ChatMessage],
        family: TokenizerFamily,
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        let original_tokens = Self::estimate_chat_for_family(messages, family);
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

        let optimized_tokens = Self::estimate_chat_for_family(&optimized, family);
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
}
