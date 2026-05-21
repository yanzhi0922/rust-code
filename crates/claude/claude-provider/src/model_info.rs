//! Model context-window information database.
//!
//! Provides [`ModelInfo`], [`ModelCapability`], and [`get_model_info`] for
//! looking up the maximum context window, output token limits, multimodal
//! support, and capability tags of supported LLM models.  The lookup uses
//! fuzzy, case-insensitive matching so that versioned model names (e.g.
//! `gpt-4o-2024-05-13`) are resolved correctly.
//!
//! # Information sources
//!
//! ## Standard API Providers
//!
//! - 智谱 AI (GLM):  <https://open.bigmodel.cn> (2026-04)
//! - MiniMax:        <https://www.minimaxi.com> (2026-04)
//! - OpenAI:         <https://platform.openai.com> (2026-04)
//! - Anthropic:      <https://docs.anthropic.com> (2026-04)
//! - DeepSeek:       <https://platform.deepseek.com> (2026-04)
//! - Qwen (阿里云):  <https://help.aliyun.com> (2026-04)
//! - Google Gemini:  <https://ai.google.dev> (2026-04)
//! - Moonshot/Kimi:  <https://platform.moonshot.cn> (2026-04)
//! - 百度 ERNIE:     <https://cloud.baidu.com> (2026-04)
//! - 腾讯混元:       <https://cloud.tencent.com> (2026-04)
//! - 火山引擎/豆包:  <https://www.volcengine.com> (2026-04)
//!
//! ## Coding Plan Providers
//!
//! - 智谱 GLM Coding Plan:  <https://docs.bigmodel.cn/cn/coding-plan/overview> (2026-04)
//! - MiniMax Token Plan:    <https://platform.minimaxi.com/docs/token-plan/intro> (2026-04)
//! - 阿里云百炼 Coding Plan: <https://help.aliyun.com/zh/model-studio/coding-plan> (2026-04)
//! - 腾讯云 Coding Plan:    <https://cloud.tencent.com/document/product/1823/130092> (2026-04)
//! - 百度千帆 Coding Plan:  <https://cloud.baidu.com/doc/qianfan/s/imlg0beiu> (2026-04)
//! - 火山引擎 Coding Plan:  <https://www.volcengine.com/docs/82379/1925114> (2026-04)
//! - Kimi Code Plan:        <https://www.kimi.com/code/docs/> (2026-04)
//!
//! # Context Window Notes
//!
//! - GLM-5 series: 200K context (all variants including GLM-5.1, GLM-5-Turbo)
//! - GPT-5.4: 258K context (user-specified)
//! - Qwen3.6-Plus: 200K context (阿里云百炼 Pro 套餐专属)
//! - Hunyuan 2.0: 200K context (腾讯混元最新)
//! - Most 2025+ models: 128K-200K context

// ---------------------------------------------------------------------------
// ModelCapability
// ---------------------------------------------------------------------------

/// A single capability tag that a model may advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCapability {
    /// Plain text generation / chat completion.
    Text,
    /// Image understanding (vision).
    Vision,
    /// Video understanding.
    Video,
    /// Audio understanding (speech-to-text or audio analysis).
    Audio,
    /// Function / tool calling.
    ToolUse,
    /// Extended reasoning (o1 / o3 / R1 style chain-of-thought).
    Reasoning,
    /// Code generation optimised for programming tasks.
    Code,
    /// Image generation (DALL·E, CogView, etc.).
    ImageGeneration,
}

// ---------------------------------------------------------------------------
// ModelInfo
// ---------------------------------------------------------------------------

/// Context-window metadata and capability flags for a single model variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Maximum input context length in tokens.
    pub max_context: u64,
    /// Maximum output tokens the model can generate in a single response.
    pub max_output: u64,
    /// Model family identifier (e.g. `"glm"`, `"openai"`, `"anthropic"`).
    pub family: &'static str,
    /// Whether the model accepts multimodal input (images / video / audio).
    pub multimodal: bool,
    /// Fine-grained capability tags.
    pub capabilities: &'static [ModelCapability],
}

// ---------------------------------------------------------------------------
// Convenience constructors (keep call-sites concise)
// ---------------------------------------------------------------------------

impl ModelInfo {
    /// Text-only model shorthand.
    const fn text(cx: u64, out: u64, fam: &'static str) -> Self {
        Self {
            max_context: cx,
            max_output: out,
            family: fam,
            multimodal: false,
            capabilities: &[ModelCapability::Text, ModelCapability::ToolUse],
        }
    }

    /// Multimodal model shorthand (implies Vision + ToolUse).
    const fn multi(cx: u64, out: u64, fam: &'static str) -> Self {
        Self {
            max_context: cx,
            max_output: out,
            family: fam,
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::ToolUse,
            ],
        }
    }

    /// Reasoning model shorthand (o1 / o3 / R1 style).
    const fn reasoning(cx: u64, out: u64, fam: &'static str) -> Self {
        Self {
            max_context: cx,
            max_output: out,
            family: fam,
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Reasoning,
                ModelCapability::Code,
            ],
        }
    }

    /// Multimodal + reasoning model shorthand.
    const fn multi_reasoning(cx: u64, out: u64, fam: &'static str) -> Self {
        Self {
            max_context: cx,
            max_output: out,
            family: fam,
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::Reasoning,
                ModelCapability::Code,
                ModelCapability::ToolUse,
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Public lookup
// ---------------------------------------------------------------------------

/// Return the [`ModelInfo`] for the given model name.
///
/// Matching is **fuzzy** and case-insensitive: the function checks for known
/// substrings in the lowercased model name.  More specific patterns are tested
/// first so that, for example, `"glm-5v-turbo"` is not accidentally matched by
/// the broader `"glm-5"` rule.
///
/// # Fallback
///
/// If no known pattern matches, a conservative default of **128 K** context
/// and **4 K** output is returned with `family = "unknown"`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn get_model_info(model: &str) -> ModelInfo {
    let lower = model.to_lowercase();

    // --- GLM-5 series (智谱 AI, 2025+) — most specific first ----------------
    //
    // All GLM-5 series models have 200K context window.
    // Source: https://docs.bigmodel.cn/cn/coding-plan/overview — 2026-04
    // GLM Coding Plan supports: GLM-5.1, GLM-5-Turbo, GLM-4.7, GLM-4.5-Air

    // GLM-5v-turbo: multimodal vision model (supports images)
    // Source: https://open.bigmodel.cn — 2026-04
    if lower.contains("glm-5v") || lower.contains("glm5v") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // GLM-5.1: flagship text model (200K context)
    // Source: https://open.bigmodel.cn — 2026-04
    if lower.contains("glm-5.1") || lower.contains("glm51") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // GLM-5 series catch-all (includes GLM-5-Turbo, GLM-5)
    if lower.contains("glm-5") || lower.contains("glm5") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // --- GLM-4.7 / GLM-4.6 / GLM-4.5 series — still active -----------------
    //
    // These are part of the GLM Coding Plan supported models.
    // Source: https://docs.bigmodel.cn/cn/coding-plan/overview — 2026-04

    // GLM-4.6V: vision model (200K context, supports images)
    if lower.contains("glm-4.6v") || lower.contains("glm-46v") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // GLM-4.7: text model (200K context)
    if lower.contains("glm-4.7") || lower.contains("glm-47") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // GLM-4.6: text model (200K context)
    if lower.contains("glm-4.6") || lower.contains("glm-46") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // GLM-4.5-Air: lightweight model (200K context)
    if lower.contains("glm-4.5") || lower.contains("glm-45") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "glm",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // --- GLM-4 series (legacy but still functional) -------------------------

    if lower.contains("glm-4-long") {
        return ModelInfo::text(1_000_000, 4_096, "glm");
    }
    // Catch-all for glm-4, glm-4-plus, glm-4-air, glm-4-flash, glm-4-flashx
    if lower.contains("glm-4") || lower.contains("glm4") {
        return ModelInfo::text(200_000, 4_096, "glm");
    }

    // --- MiniMax series — https://www.minimaxi.com --------------------------
    //
    // Source: https://platform.minimaxi.com/docs/token-plan/intro — 2026-04

    // MiniMax-M2.7-highspeed: ~100 TPS speed variant
    if lower.contains("m2.7-highspeed")
        || (lower.contains("highspeed") && lower.contains("minimax"))
    {
        return ModelInfo {
            max_context: 1_000_000,
            max_output: 8_192,
            family: "minimax",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // MiniMax-M2.7: latest text model, 1M context
    if lower.contains("minimax-m2.7") || (lower.contains("m2.7") && lower.contains("minimax")) {
        return ModelInfo {
            max_context: 1_000_000,
            max_output: 8_192,
            family: "minimax",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // MiniMax-M2.5: previous generation (200K context)
    // Supported in: Tencent Cloud, Baidu Qianfan, Aliyun Coding Plans
    if lower.contains("minimax-m2.5") || (lower.contains("m2.5") && lower.contains("minimax")) {
        return ModelInfo::text(200_000, 8_192, "minimax");
    }

    // MiniMax-M1: flagship text model, 1M context
    if lower.contains("minimax-m1") || (lower.contains("m1") && lower.contains("minimax")) {
        return ModelInfo::text(1_000_000, 8_192, "minimax");
    }

    // --- 腾讯混元 Hunyuan series — https://cloud.tencent.com ----------------
    //
    // Source: https://cloud.tencent.com/document/product/1823/130092 — 2026-04

    // Hunyuan 2.0 Thinking: reasoning model
    if lower.contains("hunyuan-2.0-thinking") || lower.contains("hunyuan-2.0-think") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "hunyuan",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Reasoning,
                ModelCapability::Code,
                ModelCapability::ToolUse,
            ],
        };
    }

    // Hunyuan 2.0 Instruct: instruction-following model
    if lower.contains("hunyuan-2.0-instruct") || lower.contains("hunyuan-2.0") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "hunyuan",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // Hunyuan-T1 / TurboS: legacy models (即将下线 per Tencent docs)
    if lower.contains("hunyuan-t1") || lower.contains("hunyuan-turbos") {
        return ModelInfo::text(128_000, 4_096, "hunyuan");
    }

    // tc-code-latest: Tencent Cloud Coding Plan auto-select model
    if lower.contains("tc-code") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "hunyuan",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // --- OpenAI series -------------------------------------------------------
    //
    // Source: https://platform.openai.com — 2025-2026

    // GPT-5.4: latest flagship model with 258K context
    if lower.contains("gpt-5") || lower.contains("gpt5") {
        return ModelInfo {
            max_context: 258_000,
            max_output: 32_768,
            family: "openai",
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::ToolUse,
                ModelCapability::Code,
                ModelCapability::Reasoning,
            ],
        };
    }

    // o3: reasoning model with extended context
    if lower == "o3" || (lower.contains("o3-") && !lower.contains("o3-mini")) {
        return ModelInfo::multi_reasoning(200_000, 100_000, "openai");
    }

    // o4-mini: lightweight reasoning model
    if lower.contains("o4-mini") {
        return ModelInfo::reasoning(200_000, 100_000, "openai");
    }

    // o3-mini, o1, o1-preview all share 200K / 100K
    if lower.contains("o3-mini") || lower.contains("o1-preview") || lower == "o1" {
        return ModelInfo::reasoning(200_000, 100_000, "openai");
    }
    if lower.contains("o1-mini") {
        return ModelInfo::reasoning(128_000, 65_536, "openai");
    }

    // gpt-4o (including dated snapshots like gpt-4o-2024-05-13) and gpt-4-turbo
    if lower.contains("gpt-4o") || lower.contains("gpt-4-turbo") {
        return ModelInfo::multi(200_000, 16_384, "openai");
    }

    // --- Anthropic series ----------------------------------------------------
    //
    // Source: https://docs.anthropic.com — 2026-04

    // Claude 4 / Claude 3.7 Sonnet — latest generation (200K context)
    if lower.contains("claude-4") || lower.contains("claude4") {
        return ModelInfo::multi(200_000, 16_384, "anthropic");
    }
    if lower.contains("claude-3.7") || lower.contains("claude-3-7") {
        return ModelInfo::multi(200_000, 16_384, "anthropic");
    }

    // claude-3.5-sonnet, claude-3-5-sonnet, claude-3.5-haiku → 200K / 8 192 output
    if lower.contains("claude-3.5")
        || lower.contains("claude-3-5-sonnet")
        || lower.contains("claude-3-5-haiku")
    {
        return ModelInfo::multi(200_000, 8_192, "anthropic");
    }
    // claude-3-opus, claude-3-sonnet, claude-3-haiku → 200K / 4 096 output
    if lower.contains("claude-3") {
        return ModelInfo::multi(200_000, 4_096, "anthropic");
    }

    // --- DeepSeek series -----------------------------------------------------
    //
    // Source: https://platform.deepseek.com — 2026-04

    // DeepSeek-R1: reasoning model
    if lower.contains("deepseek-r1") {
        return ModelInfo::reasoning(128_000, 8_192, "deepseek");
    }

    // DeepSeek-V3.2: latest flagship model (200K context)
    if lower.contains("deepseek-v3.2") || lower.contains("deepseek-v32") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "deepseek",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // DeepSeek-V3: previous flagship
    if lower.contains("deepseek-v3") {
        return ModelInfo {
            max_context: 128_000,
            max_output: 8_192,
            family: "deepseek",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // DeepSeek catch-all
    if lower.contains("deepseek") {
        return ModelInfo {
            max_context: 128_000,
            max_output: 8_192,
            family: "deepseek",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // --- Qwen series (通义千问) — https://help.aliyun.com --------------------
    //
    // Source: https://help.aliyun.com/zh/model-studio/coding-plan — 2026-04

    // Qwen3.6-Plus: latest flagship (Pro 套餐专属, multimodal)
    if lower.contains("qwen3.6") || lower.contains("qwen-3.6") || lower.contains("qwen-36") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "qwen",
            multimodal: true,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::Vision,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // Qwen3-Coder series: coding-optimized models
    if lower.contains("qwen3-coder") || lower.contains("qwen-3-coder") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "qwen",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // Qwen3.5-Plus: previous generation flagship
    if lower.contains("qwen3.5") || lower.contains("qwen-3.5") {
        return ModelInfo::multi(200_000, 8_192, "qwen");
    }

    // Qwen-VL-Max: multimodal vision-language model
    if lower.contains("qwen-vl") {
        return ModelInfo::multi(32_768, 8_192, "qwen");
    }

    // Qwen-Long: long-context text model (1M context)
    if lower.contains("qwen-long") {
        return ModelInfo::text(1_000_000, 8_192, "qwen");
    }

    // Qwen-Max: flagship model (200K context)
    if lower.contains("qwen-max") {
        return ModelInfo::multi(200_000, 8_192, "qwen");
    }

    // Qwen-Plus and Qwen-Turbo: standard models (200K context)
    if lower.contains("qwen-plus") || lower.contains("qwen-turbo") {
        return ModelInfo::multi(200_000, 8_192, "qwen");
    }

    // Catch-all for qwen
    if lower.contains("qwen") {
        return ModelInfo::multi(131_072, 8_192, "qwen");
    }

    // --- Google Gemini series — https://ai.google.dev -----------------------

    // Gemini 2.5 Pro: latest flagship, 1M context
    if lower.contains("gemini-2.5") || lower.contains("gemini-25") {
        return ModelInfo::multi_reasoning(1_000_000, 8_192, "gemini");
    }

    // Gemini 2.0 Pro: high-capability multimodal, 2M context
    if lower.contains("gemini-2.0-pro") || lower.contains("gemini-20-pro") {
        return ModelInfo::multi(2_000_000, 8_192, "gemini");
    }

    // Gemini 2.0 Flash: fast multimodal, 1M context
    if lower.contains("gemini-2.0") || lower.contains("gemini-20") {
        return ModelInfo::multi(1_000_000, 8_192, "gemini");
    }

    // Gemini 1.5 Pro: 2M context multimodal
    if lower.contains("gemini-1.5-pro") || lower.contains("gemini-15-pro") {
        return ModelInfo::multi(2_000_000, 8_192, "gemini");
    }

    // Gemini 1.5 Flash: 1M context multimodal
    if lower.contains("gemini-1.5") || lower.contains("gemini-15") {
        return ModelInfo::multi(1_000_000, 8_192, "gemini");
    }

    // Catch-all for any other Gemini variants
    if lower.contains("gemini") {
        return ModelInfo::multi(1_000_000, 8_192, "gemini");
    }

    // --- Moonshot / Kimi (月之暗面) — https://platform.moonshot.cn -----------
    //
    // Source: https://platform.moonshot.cn — 2026-04

    // Kimi-K2.5: latest flagship coding model (200K context)
    // Supported in: Tencent Cloud, Baidu Qianfan, Aliyun Coding Plans
    if lower.contains("kimi-k2") || lower.contains("kimi_k2") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "moonshot",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // moonshot-v1-128k and variants
    if lower.contains("moonshot-v1-128k") || lower.contains("moonshot-128k") {
        return ModelInfo::text(128_000, 4_096, "moonshot");
    }
    if lower.contains("moonshot-v1-32k") || lower.contains("moonshot-32k") {
        return ModelInfo::text(32_768, 4_096, "moonshot");
    }
    if lower.contains("moonshot-v1-8k") || lower.contains("moonshot-8k") {
        return ModelInfo::text(8_192, 4_096, "moonshot");
    }
    // Catch-all for moonshot / kimi
    if lower.contains("moonshot") || lower.contains("kimi") {
        return ModelInfo::text(128_000, 4_096, "moonshot");
    }

    // --- ERNIE / 百度文心 — https://cloud.baidu.com --------------------------
    //
    // Source: https://cloud.baidu.com/doc/qianfan/s/imlg0beiu — 2026-04

    // ERNIE-4.5-Turbo: latest model (200K context)
    if lower.contains("ernie-4.5") || lower.contains("ernie-45") {
        return ModelInfo::text(200_000, 4_096, "ernie");
    }

    // ERNIE-4.0 series with 128K context
    if lower.contains("ernie-4.0-turbo")
        || lower.contains("ernie-4-turbo")
        || lower.contains("ernie-4.0-128k")
    {
        return ModelInfo::text(128_000, 4_096, "ernie");
    }
    if lower.contains("ernie-4.0") || lower.contains("ernie-4") || lower.contains("ernie4") {
        return ModelInfo::text(128_000, 4_096, "ernie");
    }
    // Catch-all for ERNIE
    if lower.contains("ernie") {
        return ModelInfo::text(128_000, 4_096, "ernie");
    }

    // --- 火山引擎/豆包 series — https://www.volcengine.com --------------------
    //
    // Source: https://www.volcengine.com/docs/82379/1925114 — 2026-04

    // Doubao Seed 1.5 / 1.6 series
    if lower.contains("doubao-seed") || lower.contains("doubao") {
        return ModelInfo {
            max_context: 128_000,
            max_output: 8_192,
            family: "doubao",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // qianfan-code-latest: Baidu Qianfan Coding Plan auto-select model
    if lower.contains("qianfan-code") {
        return ModelInfo {
            max_context: 200_000,
            max_output: 8_192,
            family: "ernie",
            multimodal: false,
            capabilities: &[
                ModelCapability::Text,
                ModelCapability::ToolUse,
                ModelCapability::Code,
            ],
        };
    }

    // --- Default fallback ----------------------------------------------------

    ModelInfo {
        max_context: 128_000,
        max_output: 4_096,
        family: "unknown",
        multimodal: false,
        capabilities: &[ModelCapability::Text],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- GLM-5 series --------------------------------------------------------

    #[test]
    fn test_glm5_models() {
        // GLM-5.1: 200K context
        let info = get_model_info("glm-5.1");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 8_192);
        assert_eq!(info.family, "glm");
        assert!(!info.multimodal);
        assert!(info.capabilities.contains(&ModelCapability::Text));
        assert!(info.capabilities.contains(&ModelCapability::Code));

        let info = get_model_info("GLM-5.1");
        assert_eq!(info.family, "glm");
        assert_eq!(info.max_context, 200_000);

        // GLM-5-Turbo: 200K context
        let info = get_model_info("glm-5-turbo");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "glm");

        // GLM-5v-turbo: 200K context (multimodal vision model)
        let info = get_model_info("glm-5v-turbo");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "glm");
        assert!(info.multimodal);
        assert!(info.capabilities.contains(&ModelCapability::Vision));
    }

    // -- GLM-4.7 / 4.6 / 4.5 series -----------------------------------------

    #[test]
    fn test_glm4x_models() {
        // GLM-4.7: 200K context (Coding Plan supported)
        let info = get_model_info("glm-4.7");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "glm");

        // GLM-4.6: 200K context
        let info = get_model_info("glm-4.6");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "glm");

        // GLM-4.6V: 200K context (vision)
        let info = get_model_info("glm-4.6v");
        assert_eq!(info.max_context, 200_000);
        assert!(info.multimodal);

        // GLM-4.5-Air: 200K context
        let info = get_model_info("glm-4.5-air");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "glm");
    }

    // -- GLM-4 series (legacy) -----------------------------------------------

    #[test]
    fn test_glm4_legacy_models() {
        let info = get_model_info("glm-4-plus");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 4_096);
        assert_eq!(info.family, "glm");

        let info = get_model_info("glm-4-long");
        assert_eq!(info.max_context, 1_000_000);

        let info = get_model_info("glm-4-air");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("glm-4-flash");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("glm-4");
        assert_eq!(info.max_context, 200_000);
    }

    // -- MiniMax series ------------------------------------------------------

    #[test]
    fn test_minimax_models() {
        let info = get_model_info("MiniMax-M1");
        assert_eq!(info.max_context, 1_000_000);
        assert_eq!(info.max_output, 8_192);
        assert_eq!(info.family, "minimax");

        let info = get_model_info("minimax-m2.7");
        assert_eq!(info.max_context, 1_000_000);
        assert_eq!(info.max_output, 8_192);
        assert!(info.capabilities.contains(&ModelCapability::Code));

        let info = get_model_info("minimax-m2.7-highspeed");
        assert_eq!(info.max_context, 1_000_000);
        assert_eq!(info.family, "minimax");

        let info = get_model_info("minimax-m2.5");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "minimax");
    }

    // -- Hunyuan / Tencent series --------------------------------------------

    #[test]
    fn test_hunyuan_models() {
        let info = get_model_info("hunyuan-2.0-instruct");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "hunyuan");
        assert!(info.capabilities.contains(&ModelCapability::Code));

        let info = get_model_info("hunyuan-2.0-thinking");
        assert_eq!(info.max_context, 200_000);
        assert!(info.capabilities.contains(&ModelCapability::Reasoning));

        let info = get_model_info("tc-code-latest");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "hunyuan");
    }

    // -- OpenAI series -------------------------------------------------------

    #[test]
    fn test_openai_models() {
        // GPT-5.4: 258K context
        let info = get_model_info("gpt-5.4");
        assert_eq!(info.max_context, 258_000);
        assert_eq!(info.max_output, 32_768);
        assert_eq!(info.family, "openai");
        assert!(info.multimodal);

        // GPT-4o: 200K context
        let info = get_model_info("gpt-4o");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 16_384);
        assert!(info.multimodal);

        let info = get_model_info("gpt-4o-2024-05-13");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("gpt-4-turbo");
        assert_eq!(info.max_context, 200_000);

        // o-series reasoning models
        let info = get_model_info("o1");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 100_000);
        assert!(info.capabilities.contains(&ModelCapability::Reasoning));

        let info = get_model_info("o1-mini");
        assert_eq!(info.max_context, 128_000);

        let info = get_model_info("o3");
        assert_eq!(info.max_context, 200_000);
        assert!(info.multimodal);

        let info = get_model_info("o3-mini");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("o4-mini");
        assert_eq!(info.max_context, 200_000);
    }

    // -- Anthropic series ----------------------------------------------------

    #[test]
    fn test_anthropic_models() {
        let info = get_model_info("claude-3-5-sonnet-20241022");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 8_192);
        assert_eq!(info.family, "anthropic");
        assert!(info.multimodal);

        let info = get_model_info("claude-3.5-haiku");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("claude-3-opus");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 4_096);

        let info = get_model_info("claude-4-opus");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 16_384);

        let info = get_model_info("claude-3.7-sonnet");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 16_384);
    }

    // -- DeepSeek series -----------------------------------------------------

    #[test]
    fn test_deepseek_models() {
        let info = get_model_info("deepseek-v3.2");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.max_output, 8_192);
        assert_eq!(info.family, "deepseek");
        assert!(info.capabilities.contains(&ModelCapability::Code));

        let info = get_model_info("deepseek-v3");
        assert_eq!(info.max_context, 128_000);

        let info = get_model_info("deepseek-r1");
        assert_eq!(info.max_context, 128_000);
        assert!(info.capabilities.contains(&ModelCapability::Reasoning));
    }

    // -- Qwen series ---------------------------------------------------------

    #[test]
    fn test_qwen_models() {
        // Qwen3.6-Plus: 200K context (Pro 套餐专属)
        let info = get_model_info("qwen3.6-plus");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "qwen");
        assert!(info.multimodal);

        // Qwen3-Coder: 200K context
        let info = get_model_info("qwen3-coder-next");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "qwen");

        // Qwen-Max: 200K context
        let info = get_model_info("qwen-max");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("qwen-plus");
        assert_eq!(info.max_context, 200_000);

        let info = get_model_info("qwen-vl-max");
        assert_eq!(info.max_context, 32_768);
        assert!(info.multimodal);

        let info = get_model_info("qwen-long");
        assert_eq!(info.max_context, 1_000_000);
    }

    // -- Gemini series -------------------------------------------------------

    #[test]
    fn test_gemini_models() {
        let info = get_model_info("gemini-2.5-pro");
        assert_eq!(info.max_context, 1_000_000);
        assert!(info.capabilities.contains(&ModelCapability::Reasoning));

        let info = get_model_info("gemini-2.0-flash");
        assert_eq!(info.max_context, 1_000_000);

        let info = get_model_info("gemini-2.0-pro");
        assert_eq!(info.max_context, 2_000_000);

        let info = get_model_info("gemini-1.5-pro");
        assert_eq!(info.max_context, 2_000_000);

        let info = get_model_info("gemini-1.5-flash");
        assert_eq!(info.max_context, 1_000_000);

        let info = get_model_info("gemini-exp");
        assert_eq!(info.family, "gemini");
    }

    // -- Moonshot / Kimi series -----------------------------------------------

    #[test]
    fn test_moonshot_models() {
        let info = get_model_info("moonshot-v1-128k");
        assert_eq!(info.max_context, 128_000);
        assert_eq!(info.family, "moonshot");

        let info = get_model_info("moonshot-v1-32k");
        assert_eq!(info.max_context, 32_768);

        let info = get_model_info("moonshot-v1-8k");
        assert_eq!(info.max_context, 8_192);

        // Kimi-K2.5
        let info = get_model_info("kimi-k2.5");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "moonshot");
        assert!(info.capabilities.contains(&ModelCapability::Code));

        // Catch-all
        let info = get_model_info("kimi-latest");
        assert_eq!(info.family, "moonshot");
    }

    // -- ERNIE series --------------------------------------------------------

    #[test]
    fn test_ernie_models() {
        let info = get_model_info("ernie-4.5-turbo");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "ernie");

        let info = get_model_info("ernie-4.0-turbo");
        assert_eq!(info.max_context, 128_000);

        let info = get_model_info("ernie-4.0-128k");
        assert_eq!(info.max_context, 128_000);

        let info = get_model_info("ernie-4.0");
        assert_eq!(info.max_context, 128_000);

        // Catch-all
        let info = get_model_info("ernie-speed");
        assert_eq!(info.family, "ernie");
    }

    // -- Doubao / Volcengine series ------------------------------------------

    #[test]
    fn test_doubao_models() {
        let info = get_model_info("doubao-seed-1-5");
        assert_eq!(info.max_context, 128_000);
        assert_eq!(info.family, "doubao");
        assert!(info.capabilities.contains(&ModelCapability::Code));

        let info = get_model_info("qianfan-code-latest");
        assert_eq!(info.max_context, 200_000);
        assert_eq!(info.family, "ernie");
    }

    // -- Multimodal flag verification ----------------------------------------

    #[test]
    fn test_multimodal_flag() {
        // Models that SHOULD be multimodal
        let multimodal_models = [
            "gpt-4o",
            "gpt-4-turbo",
            "claude-3-5-sonnet-20241022",
            "claude-3-opus",
            "claude-4-sonnet",
            "glm-5v-turbo",
            "glm-4.6v",
            "qwen-max",
            "qwen-plus",
            "qwen3.6-plus",
            "qwen-vl-max",
            "gemini-2.0-flash",
            "gemini-1.5-pro",
            "o3",
            "gpt-5.4",
        ];
        for model in multimodal_models {
            let info = get_model_info(model);
            assert!(info.multimodal, "{model} should be multimodal");
            assert!(
                info.capabilities.contains(&ModelCapability::Vision),
                "{model} should have Vision capability"
            );
        }

        // Models that should NOT be multimodal
        let text_only_models = [
            "glm-4-plus",
            "glm-4-long",
            "glm-4-flash",
            "glm-5.1",
            "glm-4.7",
            "glm-4.5-air",
            "minimax-m1",
            "minimax-m2.7",
            "hunyuan-2.0-instruct",
            "o1",
            "o1-mini",
            "o3-mini",
            "o4-mini",
            "deepseek-v3",
            "deepseek-r1",
            "kimi-k2.5",
            "ernie-4.0",
            "qwen-long",
            "doubao-seed-1-5",
        ];
        for model in text_only_models {
            let info = get_model_info(model);
            assert!(!info.multimodal, "{model} should NOT be multimodal");
        }
    }

    // -- Reasoning capability verification -----------------------------------

    #[test]
    fn test_reasoning_capability() {
        let reasoning_models = [
            "o1",
            "o1-mini",
            "o1-preview",
            "o3",
            "o3-mini",
            "o4-mini",
            "deepseek-r1",
            "hunyuan-2.0-thinking",
        ];
        for model in reasoning_models {
            let info = get_model_info(model);
            assert!(
                info.capabilities.contains(&ModelCapability::Reasoning),
                "{model} should have Reasoning capability"
            );
        }

        // Non-reasoning models
        let non_reasoning = [
            "gpt-4o",
            "glm-4-plus",
            "deepseek-v3",
            "qwen-max",
            "hunyuan-2.0-instruct",
        ];
        for model in non_reasoning {
            let info = get_model_info(model);
            assert!(
                !info.capabilities.contains(&ModelCapability::Reasoning),
                "{model} should NOT have Reasoning capability"
            );
        }
    }

    // -- Unknown model fallback ----------------------------------------------

    #[test]
    fn test_unknown_model_fallback() {
        let info = get_model_info("some-unknown-model");
        assert_eq!(info.max_context, 128_000);
        assert_eq!(info.max_output, 4_096);
        assert_eq!(info.family, "unknown");
        assert!(!info.multimodal);

        let info = get_model_info("");
        assert_eq!(info.family, "unknown");
    }
}
