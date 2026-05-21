//! Model-specific pricing tiers and USD cost calculation.
//!
//! Mirrors `utils/modelCost.ts` from the upstream TS codebase.
//! Pricing source: https://platform.claude.com/docs/en/about-claude/pricing

use crate::UsageSummary;

// ── Pricing tiers ──────────────────────────────────────────────────────────

/// Per-model pricing in USD per million tokens.
#[derive(Debug, Clone, Copy)]
pub struct ModelCosts {
    /// USD per 1M input tokens.
    pub input_per_mtok: f64,
    /// USD per 1M output tokens.
    pub output_per_mtok: f64,
    /// USD per 1M prompt cache write tokens.
    pub cache_write_per_mtok: f64,
    /// USD per 1M prompt cache read tokens.
    pub cache_read_per_mtok: f64,
    /// USD per web search request.
    pub web_search_per_request: f64,
}

/// Sonnet-tier pricing: $3/$15 per Mtok.
pub const COST_TIER_3_15: ModelCosts = ModelCosts {
    input_per_mtok: 3.0,
    output_per_mtok: 15.0,
    cache_write_per_mtok: 3.75,
    cache_read_per_mtok: 0.3,
    web_search_per_request: 0.01,
};

/// Opus 4/4.1 pricing: $15/$75 per Mtok.
pub const COST_TIER_15_75: ModelCosts = ModelCosts {
    input_per_mtok: 15.0,
    output_per_mtok: 75.0,
    cache_write_per_mtok: 18.75,
    cache_read_per_mtok: 1.5,
    web_search_per_request: 0.01,
};

/// Opus 4.5/4.6 pricing: $5/$25 per Mtok.
pub const COST_TIER_5_25: ModelCosts = ModelCosts {
    input_per_mtok: 5.0,
    output_per_mtok: 25.0,
    cache_write_per_mtok: 6.25,
    cache_read_per_mtok: 0.5,
    web_search_per_request: 0.01,
};

/// Fast-mode Opus 4.7 pricing: $30/$150 per Mtok.
pub const COST_TIER_30_150: ModelCosts = ModelCosts {
    input_per_mtok: 30.0,
    output_per_mtok: 150.0,
    cache_write_per_mtok: 37.5,
    cache_read_per_mtok: 3.0,
    web_search_per_request: 0.01,
};

/// Haiku 3.5 pricing: $0.80/$4 per Mtok.
pub const COST_HAIKU_35: ModelCosts = ModelCosts {
    input_per_mtok: 0.8,
    output_per_mtok: 4.0,
    cache_write_per_mtok: 1.0,
    cache_read_per_mtok: 0.08,
    web_search_per_request: 0.01,
};

/// Haiku 4.5 pricing: $1/$5 per Mtok.
pub const COST_HAIKU_45: ModelCosts = ModelCosts {
    input_per_mtok: 1.0,
    output_per_mtok: 5.0,
    cache_write_per_mtok: 1.25,
    cache_read_per_mtok: 0.1,
    web_search_per_request: 0.01,
};

// Default cost for unknown models.
const DEFAULT_UNKNOWN_MODEL_COST: ModelCosts = COST_TIER_5_25;

// ── Cost resolution ────────────────────────────────────────────────────────

/// Resolve the pricing tier for a given model ID.
///
/// Matches the logic in `getModelCosts()` from `modelCost.ts`.
pub fn get_model_costs(model_id: &str) -> ModelCosts {
    let lower = model_id.to_lowercase();

    // Haiku models
    if lower.contains("claude-3-5-haiku") {
        return COST_HAIKU_35;
    }
    if lower.contains("claude-haiku-4") || lower.contains("claude-haiku-4-5") {
        return COST_HAIKU_45;
    }

    // Sonnet models
    if lower.contains("claude-3-5-sonnet")
        || lower.contains("claude-3-7-sonnet")
        || lower.contains("claude-sonnet-4")
        || lower.contains("claude-sonnet-4-5")
        || lower.contains("claude-sonnet-4-6")
    {
        return COST_TIER_3_15;
    }

    // Opus 4/4.1
    if lower.contains("claude-opus-4-1") || lower.contains("claude-opus-4-0") {
        return COST_TIER_15_75;
    }
    if lower.contains("claude-opus-4")
        && !lower.contains("opus-4-5")
        && !lower.contains("opus-4-6")
        && !lower.contains("opus-4-7")
    {
        return COST_TIER_15_75;
    }

    // Opus 4.5 / 4.6
    if lower.contains("claude-opus-4-5") || lower.contains("claude-opus-4-6") {
        return COST_TIER_5_25;
    }

    // Opus 4.7 — uses Opus 4.5/4.6 pricing in standard mode
    if lower.contains("claude-opus-4-7") {
        return COST_TIER_5_25;
    }

    DEFAULT_UNKNOWN_MODEL_COST
}

/// Resolve the pricing tier for a model, considering fast mode.
pub fn get_model_costs_with_fast_mode(model_id: &str, fast_mode: bool) -> ModelCosts {
    let lower = model_id.to_lowercase();
    if fast_mode && lower.contains("claude-opus-4-7") {
        return COST_TIER_30_150;
    }
    get_model_costs(model_id)
}

// ── USD calculation ────────────────────────────────────────────────────────

/// Calculate the USD cost for a single response.
///
/// Mirrors `tokensToUSDCost()` from `modelCost.ts`.
pub fn calculate_usd_cost(model_id: &str, usage: &UsageSummary) -> f64 {
    calculate_usd_cost_with_fast_mode(model_id, usage, false)
}

/// Calculate USD cost with optional fast-mode pricing.
pub fn calculate_usd_cost_with_fast_mode(
    model_id: &str,
    usage: &UsageSummary,
    fast_mode: bool,
) -> f64 {
    let costs = get_model_costs_with_fast_mode(model_id, fast_mode);
    tokens_to_usd(&costs, usage)
}

/// Core token-to-USD conversion.
fn tokens_to_usd(costs: &ModelCosts, usage: &UsageSummary) -> f64 {
    (usage.input_tokens as f64 / 1_000_000.0) * costs.input_per_mtok
        + (usage.output_tokens as f64 / 1_000_000.0) * costs.output_per_mtok
        + (usage.cache_read_input_tokens as f64 / 1_000_000.0) * costs.cache_read_per_mtok
        + (usage.cache_creation_input_tokens as f64 / 1_000_000.0) * costs.cache_write_per_mtok
        + (usage.server_tool_use_web_search_requests as f64) * costs.web_search_per_request
}

// ── Formatting ─────────────────────────────────────────────────────────────

/// Format a price value: integers without decimals, others with 2 decimal places.
pub fn format_price(price: f64) -> String {
    if price.fract() == 0.0 {
        format!("${price:.0}")
    } else {
        format!("${price:.2}")
    }
}

/// Format model costs as a pricing string (e.g., "$3/$15 per Mtok").
pub fn format_model_pricing(costs: &ModelCosts) -> String {
    format!(
        "{}/{} per Mtok",
        format_price(costs.input_per_mtok),
        format_price(costs.output_per_mtok)
    )
}

/// Get formatted pricing string for a model ID.
pub fn get_model_pricing_string(model_id: &str) -> Option<String> {
    let lower = model_id.to_lowercase();
    if !lower.contains("claude-") {
        return None;
    }
    let costs = get_model_costs(model_id);
    Some(format_model_pricing(&costs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonnet_4_6_uses_tier_3_15() {
        let costs = get_model_costs("claude-sonnet-4-6");
        assert_eq!(costs.input_per_mtok, 3.0);
        assert_eq!(costs.output_per_mtok, 15.0);
    }

    #[test]
    fn opus_4_uses_tier_15_75() {
        let costs = get_model_costs("claude-opus-4");
        assert_eq!(costs.input_per_mtok, 15.0);
        assert_eq!(costs.output_per_mtok, 75.0);
    }

    #[test]
    fn opus_4_5_uses_tier_5_25() {
        let costs = get_model_costs("claude-opus-4-5");
        assert_eq!(costs.input_per_mtok, 5.0);
        assert_eq!(costs.output_per_mtok, 25.0);
    }

    #[test]
    fn opus_4_7_uses_tier_5_25_standard() {
        let costs = get_model_costs("claude-opus-4-7");
        assert_eq!(costs.input_per_mtok, 5.0);
    }

    #[test]
    fn opus_4_7_fast_uses_tier_30_150() {
        let costs = get_model_costs_with_fast_mode("claude-opus-4-7", true);
        assert_eq!(costs.input_per_mtok, 30.0);
        assert_eq!(costs.output_per_mtok, 150.0);
    }

    #[test]
    fn haiku_3_5_pricing() {
        let costs = get_model_costs("claude-3-5-haiku");
        assert_eq!(costs.input_per_mtok, 0.8);
        assert_eq!(costs.output_per_mtok, 4.0);
    }

    #[test]
    fn haiku_4_5_pricing() {
        let costs = get_model_costs("claude-haiku-4-5");
        assert_eq!(costs.input_per_mtok, 1.0);
        assert_eq!(costs.output_per_mtok, 5.0);
    }

    #[test]
    fn unknown_model_uses_default() {
        let costs = get_model_costs("some-other-model");
        assert_eq!(
            costs.input_per_mtok,
            DEFAULT_UNKNOWN_MODEL_COST.input_per_mtok
        );
    }

    #[test]
    fn calculate_usd_cost_basic() {
        let usage = UsageSummary {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            ..Default::default()
        };
        let cost = calculate_usd_cost("claude-sonnet-4-6", &usage);
        assert!((cost - 18.0).abs() < 1e-9, "cost was {cost}");
    }

    #[test]
    fn calculate_usd_cost_with_cache() {
        let usage = UsageSummary {
            input_tokens: 500_000,
            output_tokens: 200_000,
            cache_read_input_tokens: 300_000,
            cache_creation_input_tokens: 100_000,
            server_tool_use_web_search_requests: 5,
            ..Default::default()
        };
        let cost = calculate_usd_cost("claude-sonnet-4-6", &usage);
        let expected = (500_000.0 / 1_000_000.0) * 3.0
            + (200_000.0 / 1_000_000.0) * 15.0
            + (300_000.0 / 1_000_000.0) * 0.3
            + (100_000.0 / 1_000_000.0) * 3.75
            + 5.0 * 0.01;
        assert!(
            (cost - expected).abs() < 1e-9,
            "cost was {cost}, expected {expected}"
        );
    }

    #[test]
    fn format_price_integer() {
        assert_eq!(format_price(3.0), "$3");
    }

    #[test]
    fn format_price_decimal() {
        assert_eq!(format_price(0.8), "$0.80");
    }

    #[test]
    fn format_model_pricing_string() {
        let s = format_model_pricing(&COST_TIER_3_15);
        assert_eq!(s, "$3/$15 per Mtok");
    }
}
