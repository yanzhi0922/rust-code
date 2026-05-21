//! Cost tracking for API usage.
//!
//! Pricing sources (per million tokens, USD):
//!
//! - OpenAI:    <https://platform.openai.com/pricing> (2025-01, 2026-04)
//! - Anthropic: <https://docs.anthropic.com/pricing> (2025-01)
//! - 智谱 AI:   <https://open.bigmodel.cn/pricing> (2025-01, 2026-04)
//! - MiniMax:   <https://www.minimaxi.com/pricing> (2025-06, 2026-04)
//! - DeepSeek:  <https://platform.deepseek.com/pricing> (2025-01)
//! - Qwen:      <https://help.aliyun.com> (2025-01)
//! - Google:    <https://ai.google.dev/pricing> (2025-01)
//! - Moonshot:  <https://platform.moonshot.cn/pricing> (2025-01)
//! - 百度 ERNIE: <https://cloud.baidu.com> (2025-01)

use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt::Write as _;

/// Per-model cost breakdown.
#[derive(Debug, Clone, Default)]
pub struct ModelCost {
    /// Total input tokens for this model.
    pub input_tokens: u64,
    /// Total output tokens for this model.
    pub output_tokens: u64,
    /// Estimated cost in USD for this model.
    pub cost_usd: f64,
}

/// Thread-safe cost tracker that accumulates token usage and estimated costs.
#[derive(Debug)]
pub struct CostTracker {
    inner: Mutex<CostTrackerInner>,
}

#[derive(Debug, Clone, Default)]
struct CostTrackerInner {
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_creation_tokens: u64,
    estimated_cost_usd: f64,
    per_model: HashMap<String, ModelCost>,
}

impl CostTracker {
    /// Create a new, empty cost tracker.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CostTrackerInner::default()),
        }
    }

    /// Record a single API call's token usage.
    pub fn record(&self, model: &str, input_tokens: u64, output_tokens: u64) {
        let cost = estimate_cost(model, input_tokens, output_tokens);
        let mut inner = self.inner.lock();
        inner.total_input_tokens += input_tokens;
        inner.total_output_tokens += output_tokens;
        inner.estimated_cost_usd += cost;

        let entry = inner.per_model.entry(model.to_owned()).or_default();
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;
        entry.cost_usd += cost;
    }

    /// Record cache-related token usage.
    pub fn record_cache(&self, cache_read_tokens: u64, cache_creation_tokens: u64) {
        let mut inner = self.inner.lock();
        inner.total_cache_read_tokens += cache_read_tokens;
        inner.total_cache_creation_tokens += cache_creation_tokens;
    }

    /// Get the total estimated cost in USD.
    pub fn total_cost_usd(&self) -> f64 {
        self.inner.lock().estimated_cost_usd
    }

    /// Get the total input tokens across all models.
    pub fn total_input_tokens(&self) -> u64 {
        self.inner.lock().total_input_tokens
    }

    /// Get the total output tokens across all models.
    pub fn total_output_tokens(&self) -> u64 {
        self.inner.lock().total_output_tokens
    }

    /// Generate a human-readable summary of accumulated costs.
    pub fn summary(&self) -> String {
        let inner = self.inner.lock();
        let mut out = String::new();

        let _ = writeln!(
            out,
            "=== Cost Summary ===\nTotal input tokens:  {}\nTotal output tokens: {}\nCache read tokens:   {}\nCache creation tokens: {}\nEstimated cost:      ${:.6} USD",
            inner.total_input_tokens,
            inner.total_output_tokens,
            inner.total_cache_read_tokens,
            inner.total_cache_creation_tokens,
            inner.estimated_cost_usd,
        );

        if !inner.per_model.is_empty() {
            let _ = writeln!(out, "\nPer-model breakdown:");
            let mut models: Vec<_> = inner.per_model.iter().collect();
            models.sort_by(|a, b| {
                b.1.cost_usd
                    .partial_cmp(&a.1.cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (model, cost) in models {
                let _ = writeln!(
                    out,
                    "  {}: {} in / {} out → ${:.6}",
                    model, cost.input_tokens, cost.output_tokens, cost.cost_usd
                );
            }
        }

        out
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Estimate the cost in USD for a single API call.
///
/// Pricing is per million tokens and uses publicly available rates for
/// well-known models. Unknown models default to GPT-4o-mini pricing.
pub fn estimate_cost(model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    let (input_per_m, output_per_m) = pricing_for_model(model);
    let input_cost = (input_tokens as f64 / 1_000_000.0) * input_per_m;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * output_per_m;
    input_cost + output_cost
}

/// Return `(input_price_per_million, output_price_per_million)` for a model.
///
/// All prices are in **USD per million tokens**.  For Chinese providers that
/// publish CNY pricing, an approximate conversion rate of ¥1 ≈ $0.14 is used.
#[allow(clippy::too_many_lines)]
fn pricing_for_model(model: &str) -> (f64, f64) {
    let lower = model.to_ascii_lowercase();

    // ---- OpenAI family -----------------------------------------------------

    // o3: $2.00 / $8.00 per million (reasoning)
    // Source: https://platform.openai.com/pricing — 2025-04
    if lower == "o3" {
        return (2.00, 8.00);
    }

    // o4-mini: $1.10 / $4.40 per million (reasoning)
    // Source: https://platform.openai.com/pricing — 2025-04
    if lower.contains("o4-mini") {
        return (1.10, 4.40);
    }

    // o3-mini: $1.10 / $4.40 per million
    if lower.contains("o3-mini") {
        return (1.10, 4.40);
    }

    // o1: $15.00 / $60.00 per million
    // o1-preview: same tier
    if lower == "o1" || lower.contains("o1-preview") {
        return (15.00, 60.00);
    }

    // o1-mini: $1.50 / $6.00 per million
    if lower.contains("o1-mini") {
        return (1.50, 6.00);
    }

    // GPT-4.5: $30.00 / $120.00 per million (conservative estimate)
    // Source: https://platform.openai.com/pricing — 2025-04
    if lower.contains("gpt-4.5") || lower.contains("gpt-45") {
        return (30.00, 120.00);
    }

    // GPT-4o family (non-mini)
    if lower.contains("gpt-4o") && !lower.contains("mini") {
        // $2.50 / $10.00 per million tokens
        return (2.50, 10.00);
    }
    if lower.contains("gpt-4o-mini") {
        // $0.15 / $0.60 per million tokens
        return (0.15, 0.60);
    }

    // GPT-4 Turbo
    if lower.contains("gpt-4-turbo") || lower.contains("gpt-4-0") {
        return (10.00, 30.00);
    }

    // GPT-4 (base)
    if lower.contains("gpt-4") {
        return (30.00, 60.00);
    }

    // GPT-3.5 Turbo
    if lower.contains("gpt-3.5") {
        return (0.50, 1.50);
    }

    // ---- Anthropic family --------------------------------------------------

    // Claude 4: conservative estimate based on Opus tier
    // Source: https://docs.anthropic.com/pricing — 2026-04
    if lower.contains("claude-4") || lower.contains("claude4") {
        return (15.00, 75.00);
    }

    // Claude 3.7 Sonnet
    if lower.contains("claude-3.7") || lower.contains("claude-3-7") {
        return (3.00, 15.00);
    }

    // Claude 3.5 Sonnet
    if lower.contains("claude-3-5-sonnet") || lower.contains("claude-3.5-sonnet") {
        return (3.00, 15.00);
    }

    // Claude 3.5 Haiku
    if lower.contains("claude-3-5-haiku") || lower.contains("claude-3.5-haiku") {
        return (0.80, 4.00);
    }

    // Claude 3 Opus
    if lower.contains("claude-3-opus") || lower.contains("claude.3-opus") {
        return (15.00, 75.00);
    }

    // Claude 3 Haiku
    if lower.contains("claude-3-haiku") || lower.contains("claude.3-haiku") {
        return (0.25, 1.25);
    }

    // Claude 3 Sonnet (original)
    if lower.contains("claude-3") {
        return (3.00, 15.00);
    }

    // ---- GLM family (智谱 AI) ----------------------------------------------

    // GLM-5.1: ¥0.05/千token in, ¥0.05/千token out ≈ $7.0/M tokens
    // Source: https://open.bigmodel.cn/pricing — 2026-04
    if lower.contains("glm-5") || lower.contains("glm5") {
        return (7.00, 7.00);
    }

    // GLM-4-Plus: ¥0.05/千token ≈ $7.0/M tokens
    if lower.contains("glm-4-plus") {
        return (7.00, 7.00);
    }

    // GLM-4-Long: ¥0.001/千token ≈ $0.14/M tokens (very cheap for long context)
    if lower.contains("glm-4-long") {
        return (0.14, 0.14);
    }

    // GLM-4V / GLM-4V-Plus: ¥0.01/千token ≈ $1.4/M tokens
    if lower.contains("glm-4v") {
        return (1.40, 1.40);
    }

    // GLM-4-Air: ¥0.001/千token ≈ $0.14/M tokens
    if lower.contains("glm-4-air") {
        return (0.14, 0.14);
    }

    // GLM-4-Flash / FlashX: free tier, minimal cost estimate
    if lower.contains("glm-4-flash") {
        return (0.01, 0.01);
    }

    // GLM-4 (base) catch-all
    if lower.contains("glm-4") || lower.contains("glm4") {
        return (7.00, 7.00);
    }

    // ---- MiniMax family ----------------------------------------------------

    // MiniMax-M1: ¥0.01/千token ≈ $1.4/M tokens
    // Source: https://www.minimaxi.com/pricing — 2025-06
    if lower.contains("minimax-m1") {
        return (1.40, 1.40);
    }

    // MiniMax-M2.7: ¥0.01/千token ≈ $1.4/M tokens
    // Source: https://www.minimaxi.com/pricing — 2026-04
    if lower.contains("minimax-m2") {
        return (1.40, 1.40);
    }

    // abab-7: ¥0.005/千token ≈ $0.7/M tokens
    if lower.contains("abab-7") || lower.contains("abab7") {
        return (0.70, 0.70);
    }

    // abab-6.5s: ¥0.002/千token ≈ $0.28/M tokens
    if lower.contains("abab-6") || lower.contains("abab6") {
        return (0.28, 0.28);
    }

    // ---- DeepSeek family ---------------------------------------------------

    // DeepSeek-R1: $0.55 / $2.19 per million (reasoning)
    // Source: https://platform.deepseek.com/pricing — 2025-01
    if lower.contains("deepseek-r1") {
        return (0.55, 2.19);
    }

    // DeepSeek-V3 / V2.5: $0.27 / $1.10 per million
    if lower.contains("deepseek") {
        return (0.27, 1.10);
    }

    // ---- Qwen family (通义千问) ---------------------------------------------

    // Qwen-Max: ¥0.02/千token ≈ $2.8/M tokens
    // Source: https://help.aliyun.com — 2025-01
    if lower.contains("qwen-max") {
        return (2.80, 2.80);
    }

    // Qwen-VL-Max: ¥0.02/千token ≈ $2.8/M tokens
    if lower.contains("qwen-vl") {
        return (2.80, 2.80);
    }

    // Qwen-Long: ¥0.0005/千token ≈ $0.07/M tokens
    if lower.contains("qwen-long") {
        return (0.07, 0.07);
    }

    // Qwen-Plus / Qwen-Turbo: ¥0.004/千token ≈ $0.56/M tokens
    if lower.contains("qwen") {
        return (0.56, 0.56);
    }

    // ---- Google Gemini family -----------------------------------------------

    // Gemini 2.5 Pro: $1.25 / $10.00 per million
    // Source: https://ai.google.dev/pricing — 2025-03
    if lower.contains("gemini-2.5") || lower.contains("gemini-25") {
        return (1.25, 10.00);
    }

    // Gemini 2.0 Pro: $1.25 / $5.00 per million
    if lower.contains("gemini-2.0-pro") || lower.contains("gemini-20-pro") {
        return (1.25, 5.00);
    }

    // Gemini 2.0 Flash: $0.10 / $0.40 per million
    if lower.contains("gemini-2.0") || lower.contains("gemini-20") {
        return (0.10, 0.40);
    }

    // Gemini 1.5 Pro: $1.25 / $5.00 per million
    if lower.contains("gemini-1.5-pro") || lower.contains("gemini-15-pro") {
        return (1.25, 5.00);
    }

    // Gemini 1.5 Flash: $0.075 / $0.30 per million
    if lower.contains("gemini-1.5") || lower.contains("gemini-15") {
        return (0.075, 0.30);
    }

    // Gemini catch-all
    if lower.contains("gemini") {
        return (0.10, 0.40);
    }

    // ---- Moonshot / Kimi family ---------------------------------------------

    // moonshot-v1: ¥0.012/千token in, ¥0.012/千token out ≈ $1.68/M tokens
    // Source: https://platform.moonshot.cn/pricing — 2025-01
    if lower.contains("moonshot") || lower.contains("kimi") {
        return (1.68, 1.68);
    }

    // ---- ERNIE / 百度文心 family --------------------------------------------

    // ERNIE-4.0: ¥0.12/千token ≈ $16.8/M tokens
    // Source: https://cloud.baidu.com — 2025-01
    if lower.contains("ernie-4") {
        return (16.80, 16.80);
    }

    // ERNIE-3.5: ¥0.012/千token ≈ $1.68/M tokens
    if lower.contains("ernie-3") {
        return (1.68, 1.68);
    }

    // ERNIE catch-all
    if lower.contains("ernie") {
        return (1.68, 1.68);
    }

    // ---- Default: use GPT-4o-mini pricing as a safe fallback ---------------
    (0.15, 0.60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_gpt4o() {
        let cost = estimate_cost("gpt-4o", 1_000_000, 1_000_000);
        assert!((cost - 12.50).abs() < 0.01, "expected ~12.50, got {cost}");
    }

    #[test]
    fn estimate_cost_claude_sonnet() {
        let cost = estimate_cost("claude-3-5-sonnet-20241022", 1_000_000, 1_000_000);
        assert!((cost - 18.00).abs() < 0.01, "expected ~18.00, got {cost}");
    }

    #[test]
    fn estimate_cost_unknown_model_uses_default() {
        let cost = estimate_cost("unknown-model", 1_000_000, 1_000_000);
        assert!((cost - 0.75).abs() < 0.01, "expected ~0.75, got {cost}");
    }

    #[test]
    fn estimate_cost_deepseek_v3() {
        let cost = estimate_cost("deepseek-v3", 1_000_000, 1_000_000);
        assert!((cost - 1.37).abs() < 0.01, "expected ~1.37, got {cost}");
    }

    #[test]
    fn estimate_cost_deepseek_r1() {
        let cost = estimate_cost("deepseek-r1", 1_000_000, 1_000_000);
        assert!((cost - 2.74).abs() < 0.01, "expected ~2.74, got {cost}");
    }

    #[test]
    fn estimate_cost_gemini_flash() {
        let cost = estimate_cost("gemini-2.0-flash", 1_000_000, 1_000_000);
        assert!((cost - 0.50).abs() < 0.01, "expected ~0.50, got {cost}");
    }

    #[test]
    fn estimate_cost_moonshot() {
        let cost = estimate_cost("moonshot-v1-128k", 1_000_000, 1_000_000);
        assert!((cost - 3.36).abs() < 0.01, "expected ~3.36, got {cost}");
    }

    #[test]
    fn estimate_cost_minimax_m1() {
        let cost = estimate_cost("minimax-m1", 1_000_000, 1_000_000);
        assert!((cost - 2.80).abs() < 0.01, "expected ~2.80, got {cost}");
    }

    #[test]
    fn estimate_cost_glm5() {
        let cost = estimate_cost("glm-5.1", 1_000_000, 1_000_000);
        assert!((cost - 14.00).abs() < 0.01, "expected ~14.00, got {cost}");
    }

    #[test]
    fn estimate_cost_ernie_4() {
        let cost = estimate_cost("ernie-4.0-8k", 1_000_000, 1_000_000);
        assert!((cost - 33.60).abs() < 0.01, "expected ~33.60, got {cost}");
    }

    #[test]
    fn estimate_cost_o3() {
        let cost = estimate_cost("o3", 1_000_000, 1_000_000);
        assert!((cost - 10.00).abs() < 0.01, "expected ~10.00, got {cost}");
    }

    #[test]
    fn estimate_cost_o4_mini() {
        let cost = estimate_cost("o4-mini", 1_000_000, 1_000_000);
        assert!((cost - 5.50).abs() < 0.01, "expected ~5.50, got {cost}");
    }

    #[test]
    fn tracker_records_multiple_models() {
        let tracker = CostTracker::new();
        tracker.record("gpt-4o", 1000, 500);
        tracker.record("gpt-4o-mini", 2000, 1000);

        assert_eq!(tracker.total_input_tokens(), 3000);
        assert_eq!(tracker.total_output_tokens(), 1500);
        assert!(tracker.total_cost_usd() > 0.0);

        let summary = tracker.summary();
        assert!(summary.contains("gpt-4o"));
        assert!(summary.contains("gpt-4o-mini"));
    }

    #[test]
    fn tracker_record_cache() {
        let tracker = CostTracker::new();
        tracker.record_cache(500, 200);

        let summary = tracker.summary();
        assert!(summary.contains("500"));
        assert!(summary.contains("200"));
    }

    #[test]
    fn default_trait_works() {
        let tracker = CostTracker::default();
        assert_eq!(tracker.total_cost_usd(), 0.0);
    }
}
