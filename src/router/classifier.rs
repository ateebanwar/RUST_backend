//! Specification-driven Task Classification Engine.
//!
//! Conforms strictly to MASTER_SPEC.md §6.1, §6.2, §6.3, and ARCHITECTURE.md §2.
//!
//! Categorizes inbound requests across modalities and estimates task complexity
//! using deterministic, bounded, zero-network heuristics.

use crate::errors::AppError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum allowed prompt length in characters per API_SPEC.md §4.1.
pub const MAX_PROMPT_LENGTH: usize = 100_000;

/// Normalized task categories defined by MASTER_SPEC.md §6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    /// Simple questions and quick conversational responses.
    SimpleQuestions,
    /// General multi-turn conversation and chit-chat.
    GeneralConversation,
    /// Complex multi-step reasoning, logical deductions, and theoretical analysis.
    ComplexAnalysis,
    /// Fast factual lookups and quick reasoning queries.
    FastReasoning,
    /// Code generation, syntax analysis, refactoring, and debugging.
    CodingAndDebugging,
    /// Systems architecture, distributed systems, and software engineering design.
    ArchitectureAndDesign,
    /// Mathematics, algebra, calculus, and technical computation.
    MathematicsAndCalculation,
    /// Web search queries and recent information retrieval.
    WebSearchAndResearch,
    /// Image generation from text descriptions.
    ImageGeneration,
    /// Image manipulation, cropping, inpainting, and editing.
    ImageEditing,
    /// Motion and video generation.
    VideoGeneration,
    /// Audio transcription, synthesis, and processing.
    AudioProcessing,
    /// Long-form structured document and report creation.
    DocumentCreation,
    /// Slide deck and presentation generation.
    PowerPointGeneration,
    /// Information extraction into structured formats (JSON, CSV, tables).
    DataExtraction,
}

impl TaskCategory {
    /// Canonical string representation matching API_SPEC.md §4.1 and MASTER_SPEC.md §6.2.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SimpleQuestions => "simple_questions",
            Self::GeneralConversation => "general_conversation",
            Self::ComplexAnalysis => "complex_analysis",
            Self::FastReasoning => "fast_reasoning",
            Self::CodingAndDebugging => "coding_and_debugging",
            Self::ArchitectureAndDesign => "architecture_and_design",
            Self::MathematicsAndCalculation => "mathematics_and_calculation",
            Self::WebSearchAndResearch => "web_search_and_research",
            Self::ImageGeneration => "image_generation",
            Self::ImageEditing => "image_editing",
            Self::VideoGeneration => "video_generation",
            Self::AudioProcessing => "audio_processing",
            Self::DocumentCreation => "document_creation",
            Self::PowerPointGeneration => "powerpoint_generation",
            Self::DataExtraction => "data_extraction_and_structuring",
        }
    }

    /// Whether this task typically requires external tooling or retrieval augmentation.
    pub fn requires_tools(&self) -> bool {
        matches!(
            self,
            Self::WebSearchAndResearch
                | Self::ImageGeneration
                | Self::ImageEditing
                | Self::VideoGeneration
                | Self::AudioProcessing
        )
    }

    /// The default expected output modality for this task category.
    pub fn default_output_modality(&self) -> TaskModality {
        match self {
            Self::CodingAndDebugging => TaskModality::Code,
            Self::ImageGeneration | Self::ImageEditing => TaskModality::Image,
            Self::VideoGeneration => TaskModality::Video,
            Self::AudioProcessing => TaskModality::Audio,
            Self::DocumentCreation | Self::PowerPointGeneration => TaskModality::Document,
            _ => TaskModality::Text,
        }
    }
}

impl fmt::Display for TaskCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Task complexity tier defined by MASTER_SPEC.md §6.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskComplexity {
    /// Fast, lightweight requests requiring minimal compute.
    Low,
    /// Standard tasks, moderate analysis, or multi-function coding.
    Medium,
    /// Difficult reasoning, concurrency, formal proofs, or high-dimensional problems.
    High,
}

impl TaskComplexity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl fmt::Display for TaskComplexity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Supported modalities per API_SPEC.md §3.1 & §4.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskModality {
    Text,
    Code,
    Image,
    Video,
    Audio,
    Document,
}

impl TaskModality {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Code => "code",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
        }
    }

    /// Parse modality from a string with normalization and validation.
    pub fn from_str_normalized(s: &str) -> Result<Self, AppError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" | "" => Ok(Self::Text),
            "code" => Ok(Self::Code),
            "image" => Ok(Self::Image),
            "video" => Ok(Self::Video),
            "audio" => Ok(Self::Audio),
            "document" => Ok(Self::Document),
            other => Err(AppError::InvalidRequest(format!(
                "Unsupported modality '{other}'. Valid options: text, code, image, video, audio, document"
            ))),
        }
    }
}

impl fmt::Display for TaskModality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Comprehensive, typed classification decision with explainable metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassificationResult {
    /// Detected task category from the taxonomy.
    pub category: TaskCategory,
    /// Estimated complexity tier for model tier matching.
    pub complexity: TaskComplexity,
    /// Primary input modality.
    pub input_modality: TaskModality,
    /// Target output modality.
    pub output_modality: TaskModality,
    /// Whether the task requires tool calling or search.
    pub requires_tools: bool,
    /// Deterministic confidence score in range 0.0 to 1.0.
    pub confidence: f32,
    /// Short human-readable summary of the detected intent.
    pub detected_intent: String,
    /// Detailed rationale explaining the classification decision.
    pub rationale: String,
}

/// Deterministic, zero-dependency task classifier.
pub struct TaskClassifier;

impl TaskClassifier {
    /// Validate input prompt against size bounds and empty content rules.
    pub fn validate_prompt(prompt: &str) -> Result<&str, AppError> {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidRequest(
                "Prompt cannot be empty".to_string(),
            ));
        }
        let char_count = trimmed.chars().count();
        if char_count > MAX_PROMPT_LENGTH {
            return Err(AppError::InvalidRequest(format!(
                "Prompt exceeds {MAX_PROMPT_LENGTH} characters limit"
            )));
        }
        Ok(trimmed)
    }

    /// Backward-compatible classification interface returning `(detected_task, estimated_complexity)`.
    pub fn classify(prompt: &str, modality: &str) -> (String, String) {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return (
                TaskCategory::GeneralConversation.as_str().to_string(),
                TaskComplexity::Low.as_str().to_string(),
            );
        }

        let parsed_modality =
            TaskModality::from_str_normalized(modality).unwrap_or(TaskModality::Text);
        let result = Self::classify_internal(trimmed, parsed_modality);
        (
            result.category.as_str().to_string(),
            result.complexity.as_str().to_string(),
        )
    }

    /// Structured classification with strict validation, explainability, and full typing.
    pub fn classify_structured(
        prompt: &str,
        requested_modality: &str,
    ) -> Result<ClassificationResult, AppError> {
        let validated_prompt = Self::validate_prompt(prompt)?;
        let modality = TaskModality::from_str_normalized(requested_modality)?;
        Ok(Self::classify_internal(validated_prompt, modality))
    }

    /// Internal deterministic classification pipeline.
    fn classify_internal(prompt: &str, modality: TaskModality) -> ClassificationResult {
        let lower = prompt.to_ascii_lowercase();

        // --------------------------------------------------------------------
        // Stage 1: Explicit Non-Text Modality Overrides
        // --------------------------------------------------------------------
        match modality {
            TaskModality::Image => {
                let is_editing = [
                    "edit",
                    "crop",
                    "upscale",
                    "modify",
                    "filter",
                    "remove background",
                    "inpaint",
                    "adjust",
                    "resize",
                    "retouch",
                ]
                .iter()
                .any(|&kw| lower.contains(kw));

                let category = if is_editing {
                    TaskCategory::ImageEditing
                } else {
                    TaskCategory::ImageGeneration
                };

                let complexity = if lower.contains("4k")
                    || lower.contains("photorealistic")
                    || lower.contains("hyper-detailed")
                    || lower.contains("composition")
                {
                    TaskComplexity::High
                } else {
                    TaskComplexity::Medium
                };

                return ClassificationResult {
                    category,
                    complexity,
                    input_modality: modality,
                    output_modality: TaskModality::Image,
                    requires_tools: true,
                    confidence: 0.95,
                    detected_intent: if is_editing {
                        "Image editing".to_string()
                    } else {
                        "Image generation".to_string()
                    },
                    rationale: format!(
                        "Explicit image modality requested; intent classified as {category}."
                    ),
                };
            }
            TaskModality::Video => {
                return ClassificationResult {
                    category: TaskCategory::VideoGeneration,
                    complexity: TaskComplexity::High,
                    input_modality: modality,
                    output_modality: TaskModality::Video,
                    requires_tools: true,
                    confidence: 0.95,
                    detected_intent: "Video generation".to_string(),
                    rationale:
                        "Explicit video modality requested with motion synthesis requirements."
                            .to_string(),
                };
            }
            TaskModality::Audio => {
                let complexity = if lower.contains("transcribe") || lower.contains("speech to text")
                {
                    TaskComplexity::Medium
                } else {
                    TaskComplexity::High
                };
                return ClassificationResult {
                    category: TaskCategory::AudioProcessing,
                    complexity,
                    input_modality: modality,
                    output_modality: TaskModality::Audio,
                    requires_tools: true,
                    confidence: 0.95,
                    detected_intent: "Audio processing".to_string(),
                    rationale: "Explicit audio modality requested.".to_string(),
                };
            }
            TaskModality::Document => {
                let is_slides = [
                    "slide",
                    "presentation",
                    "powerpoint",
                    "ppt",
                    "keynote",
                    "deck",
                ]
                .iter()
                .any(|&kw| lower.contains(kw));

                let category = if is_slides {
                    TaskCategory::PowerPointGeneration
                } else {
                    TaskCategory::DocumentCreation
                };

                return ClassificationResult {
                    category,
                    complexity: TaskComplexity::Medium,
                    input_modality: modality,
                    output_modality: TaskModality::Document,
                    requires_tools: false,
                    confidence: 0.90,
                    detected_intent: if is_slides {
                        "Presentation slides".to_string()
                    } else {
                        "Structured document".to_string()
                    },
                    rationale: format!("Explicit document modality requested for {category}."),
                };
            }
            TaskModality::Code => {
                return Self::evaluate_code_task(&lower, prompt, 0.95);
            }
            TaskModality::Text => {
                // Fallthrough to text pattern analysis
            }
        }

        // --------------------------------------------------------------------
        // Stage 2: Media Intent Detection within Text Prompts
        // --------------------------------------------------------------------
        // PowerPoint & Presentation creation
        if [
            "powerpoint",
            "slide deck",
            "presentation on",
            "create slides",
            "generate slides",
            "ppt presentation",
            "slides for",
            "slides bana",
            "presentation bana",
            "ppt bana",
            "slides create",
            "slides generate",
            "deck bana",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
        {
            return ClassificationResult {
                category: TaskCategory::PowerPointGeneration,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Document,
                requires_tools: false,
                confidence: 0.90,
                detected_intent: "Presentation slides creation".to_string(),
                rationale: "Prompt requests slide deck / PowerPoint presentation generation."
                    .to_string(),
            };
        }

        // Structured Document & Report creation
        let has_doc_cue = [
            "draft a contract",
            "write a formal policy",
            "generate report",
            "executive summary document",
            "format as formal document",
            "create a technical whitepaper",
            "document bana",
            "report bana",
            "contract draft",
            "policy draft",
            "formal document",
            "agreement bana",
            "whitepaper bana",
            "whitepaper likho",
            "draft karo",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
            || ((lower.contains("document")
                || lower.contains("contract")
                || lower.contains("policy")
                || lower.contains("report"))
                && (lower.contains("draft") || lower.contains("bana") || lower.contains("format")));

        if has_doc_cue {
            return ClassificationResult {
                category: TaskCategory::DocumentCreation,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Document,
                requires_tools: false,
                confidence: 0.85,
                detected_intent: "Document authoring".to_string(),
                rationale: "Prompt requests long-form formal document authoring.".to_string(),
            };
        }

        // Video generation from text
        if [
            "generate a video",
            "create an animation of",
            "render a video of",
            "generate video of",
            "create a short clip of",
            "video generate",
            "video bana",
            "animation bana",
            "video clip bana",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
        {
            return ClassificationResult {
                category: TaskCategory::VideoGeneration,
                complexity: TaskComplexity::High,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Video,
                requires_tools: true,
                confidence: 0.90,
                detected_intent: "Video generation".to_string(),
                rationale: "Text prompt explicitly requests video generation.".to_string(),
            };
        }

        // Image editing cues from text
        if [
            "remove background",
            "background remove",
            "background hata",
            "inpaint",
            "crop photo",
            "crop image",
            "photo edit",
            "image edit",
            "retouch photo",
            "retouch image",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
        {
            return ClassificationResult {
                category: TaskCategory::ImageEditing,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Image,
                requires_tools: true,
                confidence: 0.90,
                detected_intent: "Image editing".to_string(),
                rationale:
                    "Text prompt explicitly requests image editing or background manipulation."
                        .to_string(),
            };
        }

        // Image generation from text
        if [
            "generate an image",
            "draw a picture of",
            "create an illustration",
            "photorealistic image",
            "render an image of",
            "generate photo of",
            "image generate",
            "image bana",
            "photo generate",
            "photo bana",
            "tasveer bana",
            "picture bana",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
        {
            return ClassificationResult {
                category: TaskCategory::ImageGeneration,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Image,
                requires_tools: true,
                confidence: 0.90,
                detected_intent: "Image generation".to_string(),
                rationale: "Text prompt explicitly requests image generation.".to_string(),
            };
        }

        // Audio processing from text
        if [
            "transcribe audio",
            "speech to text",
            "text to speech",
            "synthesize voice",
            "convert text to audio",
            "audio transcribe",
            "audio sunke",
            "awaz ko text",
            "audio convert",
            "audio text me",
            "transcription karo",
        ]
        .iter()
        .any(|&kw| lower.contains(kw))
        {
            return ClassificationResult {
                category: TaskCategory::AudioProcessing,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Audio,
                requires_tools: true,
                confidence: 0.90,
                detected_intent: "Audio processing".to_string(),
                rationale: "Text prompt requests speech/audio synthesis or transcription."
                    .to_string(),
            };
        }

        // Fast factual definition lookups (e.g., "what is Tokio?", "Rust kya hai?")
        let is_fast_factual = prompt.len() < 100
            && !prompt.contains("```")
            && !lower.contains("search")
            && !lower.contains("dhoondo")
            && !lower.contains("latest")
            && !lower.contains("taza")
            && !lower.contains("price")
            && !lower.contains("news")
            && !lower.contains("khabar")
            && !lower.contains("calculate")
            && !lower.contains("hisaab")
            && !lower.contains("solve")
            && !lower.contains("integral")
            && !lower.contains("derivative")
            && !lower.contains("extract")
            && !lower.contains("convert")
            && !lower.contains("json")
            && !lower.contains("csv")
            && !lower.contains("table")
            && !lower.contains("bug")
            && !lower.contains("fix")
            && !lower.contains("error")
            && !lower.contains("debug")
            && (lower.contains("what is")
                || lower.contains("how do i")
                || lower.contains("define ")
                || lower.contains("who is")
                || lower.contains("kya hai")
                || lower.contains("kya hota hai")
                || lower.contains("kaise kaam karta hai"));

        if is_fast_factual {
            return ClassificationResult {
                category: TaskCategory::FastReasoning,
                complexity: TaskComplexity::Low,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.85,
                detected_intent: "Fast reasoning lookup".to_string(),
                rationale: "Short factual question suitable for fast inference.".to_string(),
            };
        }

        // --------------------------------------------------------------------
        // Stage 3: Technical Domains (Systems, Code, Architecture, Math)
        // --------------------------------------------------------------------
        // Architecture and software engineering
        let arch_keywords = [
            "system architecture",
            "distributed system",
            "microservices architecture",
            "database schema design",
            "high availability design",
            "event-driven architecture",
            "scalability design",
            "cap theorem",
            "domain-driven design",
            "architectural trade-offs",
            "fault tolerance design",
            "database sharding",
            "database replication",
            "distributed architecture",
            "system design",
            "architecture design",
            "architecture bana",
            "system design karo",
        ];
        if arch_keywords.iter().any(|&kw| lower.contains(kw)) {
            return ClassificationResult {
                category: TaskCategory::ArchitectureAndDesign,
                complexity: TaskComplexity::High,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.92,
                detected_intent: "Software architecture and engineering".to_string(),
                rationale: "Prompt requests systems architecture and engineering design analysis."
                    .to_string(),
            };
        }

        // Code and debugging detection
        let code_keywords = [
            "fn ",
            "def ",
            "class ",
            "struct ",
            "impl ",
            "interface ",
            "pub fn",
            "async fn",
            "import ",
            "select *",
            "sql",
            "bug",
            "compile",
            "syntax",
            "panic",
            "error[e",
            "git",
            "bash",
            "rust",
            "python",
            "async",
            "unsafe",
            "memory",
            "code",
            "debugger",
            "segfault",
            "stack trace",
            "nullpointer",
            "refactor",
            "unit test",
            "pointer",
            "function ",
            "script ",
            "algorithm",
            "program ",
            "code likho",
            "code bana",
            "bug fix",
            "error fix",
            "script likho",
        ];
        let has_code_block = prompt.contains("```");
        let has_code_cue = code_keywords.iter().any(|&k| lower.contains(k));

        if has_code_block || has_code_cue {
            return Self::evaluate_code_task(
                &lower,
                prompt,
                if has_code_block { 0.95 } else { 0.88 },
            );
        }

        // Mathematics and technical computation
        let math_keywords = [
            "calculate",
            "integral",
            "derivative",
            "matrix multiplication",
            "eigenvalue",
            "differential equation",
            "linear algebra",
            "bayes theorem",
            "probability distribution",
            "solve for x",
            "mathematical proof",
            "prove by induction",
            "trigonometric",
            "binomial theorem",
            "calculate karo",
            "hisaab karo",
            "solve karo",
            "math problem",
            "equation solve",
        ];
        if math_keywords.iter().any(|&kw| lower.contains(kw)) {
            let is_high = lower.contains("prove")
                || lower.contains("eigenvalue")
                || lower.contains("differential")
                || lower.contains("induction")
                || lower.contains("matrix");

            let complexity = if is_high {
                TaskComplexity::High
            } else {
                TaskComplexity::Medium
            };

            return ClassificationResult {
                category: TaskCategory::MathematicsAndCalculation,
                complexity,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.88,
                detected_intent: "Mathematics and calculation".to_string(),
                rationale: "Prompt involves mathematical computation or formal proof.".to_string(),
            };
        }

        // Web search and research
        let search_keywords = [
            "search the web",
            "latest news",
            "current price of",
            "search for recent",
            "find recent articles",
            "what happened in 2026",
            "lookup online",
            "latest developments in",
            "search karo",
            "internet pe search",
            "online search",
            "online dhoondo",
            "current price",
            "taza khabar",
            "latest khabar",
            "google search",
        ];
        if search_keywords.iter().any(|&kw| lower.contains(kw)) {
            return ClassificationResult {
                category: TaskCategory::WebSearchAndResearch,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: true,
                confidence: 0.90,
                detected_intent: "Web search and research".to_string(),
                rationale: "Prompt requests real-time web search or recent information retrieval."
                    .to_string(),
            };
        }

        // Data extraction and structured parsing
        let extraction_keywords = [
            "extract into json",
            "parse csv",
            "extract fields",
            "extract structured data",
            "convert to json",
            "convert to xml",
            "regex to extract",
            "extract table",
            "json me convert",
            "csv parse",
            "table banao",
            "data extract",
            "fields extract",
            "json format me",
            "csv se json",
        ];
        if extraction_keywords.iter().any(|&kw| lower.contains(kw)) {
            return ClassificationResult {
                category: TaskCategory::DataExtraction,
                complexity: TaskComplexity::Medium,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.88,
                detected_intent: "Data extraction and structuring".to_string(),
                rationale: "Prompt requests parsing and extracting structured data.".to_string(),
            };
        }

        // --------------------------------------------------------------------
        // Stage 4: Analytical Reasoning vs Conversational Fallback
        // --------------------------------------------------------------------
        let reasoning_keywords = [
            "prove",
            "derive",
            "analyze",
            "why does",
            "step by step",
            "explain why",
            "compare and contrast",
            "implications",
            "first principles",
            "logical fallacy",
            "evaluate trade-offs",
            "step by step explain",
            "analyze karo",
            "deep analysis",
            "kyu hota hai explain",
        ];
        if reasoning_keywords.iter().any(|&k| lower.contains(k)) {
            return ClassificationResult {
                category: TaskCategory::ComplexAnalysis,
                complexity: TaskComplexity::High,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.85,
                detected_intent: "Complex analysis".to_string(),
                rationale: "Prompt requires multi-step deductive reasoning and analytical depth."
                    .to_string(),
            };
        }

        // Fast simple questions (factual lookups)
        if prompt.len() < 100
            && (lower.contains("what is")
                || lower.contains("how do i")
                || lower.contains("define")
                || lower.contains("who is")
                || lower.contains("kya hai")
                || lower.contains("kya hota hai")
                || lower.contains("kaise kaam karta hai"))
        {
            return ClassificationResult {
                category: TaskCategory::FastReasoning,
                complexity: TaskComplexity::Low,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.80,
                detected_intent: "Fast reasoning lookup".to_string(),
                rationale: "Short factual question suitable for fast inference.".to_string(),
            };
        }

        // Simple questions (conversational or general questions ending in ? or Arabic ؟)
        let trimmed_end = prompt.trim();
        if (trimmed_end.ends_with('?') || trimmed_end.ends_with('؟')) && prompt.len() < 120 {
            return ClassificationResult {
                category: TaskCategory::SimpleQuestions,
                complexity: TaskComplexity::Low,
                input_modality: TaskModality::Text,
                output_modality: TaskModality::Text,
                requires_tools: false,
                confidence: 0.80,
                detected_intent: "Simple question".to_string(),
                rationale: "Short question suitable for lightweight conversational answering."
                    .to_string(),
            };
        }

        // Stage 5: Default General Conversation
        ClassificationResult {
            category: TaskCategory::GeneralConversation,
            complexity: TaskComplexity::Low,
            input_modality: TaskModality::Text,
            output_modality: TaskModality::Text,
            requires_tools: false,
            confidence: 0.75,
            detected_intent: "General conversation".to_string(),
            rationale: "General conversational input with no specialized domain signals."
                .to_string(),
        }
    }

    /// Helper for evaluating coding task complexity and signals.
    fn evaluate_code_task(
        lower: &str,
        _prompt: &str,
        base_confidence: f32,
    ) -> ClassificationResult {
        let is_high = [
            "async",
            "lifetime",
            "unsafe",
            "concurrency",
            "deadlock",
            "mutex",
            "memory",
            "performance",
            "refactor",
            "race condition",
            "architecture",
            "coroutine",
            "atomic",
        ]
        .iter()
        .any(|&kw| lower.contains(kw));

        let complexity = if is_high {
            TaskComplexity::High
        } else {
            TaskComplexity::Medium
        };

        ClassificationResult {
            category: TaskCategory::CodingAndDebugging,
            complexity,
            input_modality: TaskModality::Code,
            output_modality: TaskModality::Code,
            requires_tools: false,
            confidence: base_confidence,
            detected_intent: "Coding and debugging".to_string(),
            rationale: format!("Code signals identified; complexity assessed as {complexity}."),
        }
    }
}
