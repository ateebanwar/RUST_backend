//! Token estimation, caching, and prompt optimization engine.
//!
//! Conforms to MASTER_SPEC.md §10, ARCHITECTURE.md §2, and API_SPEC.md §5 & §6.
//!
//! Provides deterministic token approximation, bounded LRU token caching,
//! consecutive duplicate system prompt removal, and message whitespace compaction.

use crate::models::ChatMessage;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Metrics detailing token savings and compaction outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCompactionMetrics {
    pub original_estimated_tokens: usize,
    pub optimized_estimated_tokens: usize,
    pub tokens_saved: usize,
    pub duplicate_system_messages_removed: usize,
}

/// Thread-safe bounded LRU cache for prompt token estimates.
struct TokenCache {
    capacity: usize,
    entries: VecDeque<(u64, usize)>,
}

impl TokenCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&mut self, hash: u64) -> Option<usize> {
        if let Some(pos) = self.entries.iter().position(|(h, _)| *h == hash) {
            let entry = self.entries.remove(pos)?;
            self.entries.push_front(entry);
            Some(entry.1)
        } else {
            None
        }
    }

    fn put(&mut self, hash: u64, count: usize) {
        if let Some(pos) = self.entries.iter().position(|(h, _)| *h == hash) {
            self.entries.remove(pos);
        } else if self.entries.len() >= self.capacity {
            self.entries.pop_back();
        }
        self.entries.push_front((hash, count));
    }
}

/// Global static token estimation cache (512 entries).
static TOKEN_CACHE: std::sync::LazyLock<Mutex<TokenCache>> =
    std::sync::LazyLock::new(|| Mutex::new(TokenCache::new(512)));

/// Fast 64-bit FNV-1a string hashing for token cache keys.
#[inline]
fn fnv1a_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// High-performance deterministic token estimation and prompt compaction engine.
pub struct TokenEstimator;

impl TokenEstimator {
    /// Estimate token count for a raw string slice using BPE heuristic approximation with LRU caching.
    pub fn estimate_text(text: &str) -> usize {
        if text.trim().is_empty() {
            return 0;
        }

        let hash = fnv1a_hash(text);
        if let Some(cached) = TOKEN_CACHE.lock().ok().and_then(|mut c| c.get(hash)) {
            return cached;
        }

        let tokens = Self::compute_text_tokens(text);

        if let Ok(mut cache) = TOKEN_CACHE.lock() {
            cache.put(hash, tokens);
        }

        tokens
    }

    /// Pure computation of token count for text without cache interaction.
    fn compute_text_tokens(text: &str) -> usize {
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
                        tokens += Self::word_tokens(current_word_len);
                        current_word_len = 0;
                    }
                    consecutive_spaces = 0;
                    consecutive_newlines += 1;
                    if consecutive_newlines > 1 {
                        tokens += 1; // Extra newlines in formatting take tokens
                    }
                } else if c.is_ascii_whitespace() {
                    if current_word_len > 0 {
                        tokens += Self::word_tokens(current_word_len);
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
                        tokens += Self::word_tokens(current_word_len);
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
                    tokens += Self::word_tokens(current_word_len);
                    current_word_len = 0;
                }
                consecutive_newlines = 0;
                consecutive_spaces = 0;
                non_ascii_count += 1;
            }
        }

        if current_word_len > 0 {
            tokens += Self::word_tokens(current_word_len);
        }

        // Multilingual code points (e.g., Devanagari, Arabic, CJK) average ~1 token per 1-2 chars
        if non_ascii_count > 0 {
            tokens += non_ascii_count.div_ceil(2);
        }

        tokens.max(1)
    }

    #[inline]
    fn word_tokens(len: usize) -> usize {
        if len <= 8 {
            1
        } else {
            1 + (len - 8).div_ceil(4)
        }
    }

    /// Estimate token overhead for an individual ChatMessage including role framing.
    pub fn estimate_message(msg: &ChatMessage) -> usize {
        // Standard ChatML framing: <|im_start|>{role}\n{content}<|im_end|>\n (~3 tokens overhead)
        let content_tokens = Self::estimate_text(&msg.content);
        let role_tokens = Self::estimate_text(&msg.role);
        content_tokens + role_tokens + 3
    }

    /// Estimate token count for a full chat conversation history including assistant response primer.
    pub fn estimate_chat(messages: &[ChatMessage]) -> usize {
        if messages.is_empty() {
            return 0;
        }

        let mut total = 0;
        for msg in messages {
            total += Self::estimate_message(msg);
        }

        // 3 tokens for assistant primer: <|im_start|>assistant\n
        total + 3
    }

    /// Compacts messages by deduplicating consecutive identical system messages
    /// and trimming outer padding, returning the optimized messages and savings metrics.
    ///
    /// Preserves strict conversational turn ordering, user/assistant dialogue,
    /// code block indentation, and semantic instructions.
    pub fn optimize_messages(
        messages: &[ChatMessage],
    ) -> (Vec<ChatMessage>, TokenCompactionMetrics) {
        let original_tokens = Self::estimate_chat(messages);

        if messages.is_empty() {
            return (
                Vec::new(),
                TokenCompactionMetrics {
                    original_estimated_tokens: 0,
                    optimized_estimated_tokens: 0,
                    tokens_saved: 0,
                    duplicate_system_messages_removed: 0,
                },
            );
        }

        let mut optimized: Vec<ChatMessage> = Vec::with_capacity(messages.len());
        let mut duplicate_system_count = 0;

        for msg in messages {
            let trimmed_content = msg.content.trim();

            // Detect consecutive duplicate system instructions
            let is_duplicate_system = msg.role.eq_ignore_ascii_case("system")
                && optimized.last().is_some_and(|last| {
                    last.role.eq_ignore_ascii_case("system")
                        && last.content.trim() == trimmed_content
                });

            if is_duplicate_system {
                duplicate_system_count += 1;
                continue;
            }

            // Trim outer message whitespace while preserving internal indentation (crucial for code)
            let final_content = if trimmed_content.len() < msg.content.len() {
                trimmed_content.to_string()
            } else {
                msg.content.clone()
            };

            optimized.push(ChatMessage {
                role: msg.role.clone(),
                content: final_content,
            });
        }

        let optimized_tokens = Self::estimate_chat(&optimized);
        let tokens_saved = original_tokens.saturating_sub(optimized_tokens);

        (
            optimized,
            TokenCompactionMetrics {
                original_estimated_tokens: original_tokens,
                optimized_estimated_tokens: optimized_tokens,
                tokens_saved,
                duplicate_system_messages_removed: duplicate_system_count,
            },
        )
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
                content: "Hello!".to_string(),
            },
        ];

        let tokens = TokenEstimator::estimate_chat(&msgs);
        assert!((15..=25).contains(&tokens));
    }

    #[test]
    fn test_optimize_messages_deduplicates_consecutive_system_prompts() {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant.".to_string(),
            },
            ChatMessage {
                role: "system".to_string(),
                content: "You are a helpful assistant.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "Can you help me?".to_string(),
            },
        ];

        let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
        assert_eq!(optimized.len(), 2);
        assert_eq!(optimized[0].role, "system");
        assert_eq!(optimized[0].content, "You are a helpful assistant.");
        assert_eq!(optimized[1].role, "user");
        assert_eq!(metrics.duplicate_system_messages_removed, 1);
        assert!(metrics.tokens_saved > 0);
        assert!(metrics.optimized_estimated_tokens < metrics.original_estimated_tokens);
    }

    #[test]
    fn test_optimize_messages_preserves_distinct_system_prompts() {
        let msgs = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "Instruction 1: Be concise.".to_string(),
            },
            ChatMessage {
                role: "system".to_string(),
                content: "Instruction 2: Output JSON.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "List items.".to_string(),
            },
        ];

        let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
        assert_eq!(optimized.len(), 3);
        assert_eq!(metrics.duplicate_system_messages_removed, 0);
        assert_eq!(metrics.tokens_saved, 0);
    }

    #[test]
    fn test_optimize_messages_trims_outer_whitespace_preserves_inner_code() {
        let code_content = "\n\n\n        def hello():\n    print(\"Hello world\")\n\n\n        ";
        let msgs = vec![ChatMessage {
            role: "user".to_string(),
            content: code_content.to_string(),
        }];

        let (optimized, metrics) = TokenEstimator::optimize_messages(&msgs);
        assert_eq!(optimized.len(), 1);
        assert_eq!(
            optimized[0].content,
            "def hello():\n    print(\"Hello world\")"
        );
        assert!(metrics.tokens_saved > 0);
    }

    #[test]
    fn test_token_lru_cache() {
        let prompt = "A unique test prompt for cache validation";
        let first = TokenEstimator::estimate_text(prompt);
        let second = TokenEstimator::estimate_text(prompt);
        assert_eq!(first, second);
    }
}
