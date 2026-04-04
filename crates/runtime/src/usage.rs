use crate::session::TokenUsage;
use std::sync::atomic::{AtomicU64, Ordering};

const MODEL_PRICING: &[(&str, f64, f64)] = &[
    ("claude-sonnet-4-20250514", 3.0, 15.0),
    ("claude-3-5-sonnet-20241022", 3.0, 15.0),
    ("claude-3-5-haiku-20241022", 0.80, 4.0),
    ("claude-3-opus-20240229", 15.0, 75.0),
    ("claude-3-haiku-20240307", 0.25, 1.25),
];

fn get_pricing(model: &str) -> (f64, f64) {
    for &(name, input, output) in MODEL_PRICING {
        if model == name {
            return (input, output);
        }
    }
    (3.0, 15.0)
}

#[derive(Debug, Default)]
pub struct UsageTracker {
    pub input_tokens: AtomicU64,
    pub output_tokens: AtomicU64,
    pub cache_creation_tokens: AtomicU64,
    pub cache_read_tokens: AtomicU64,
    model: String,
}

impl UsageTracker {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Default::default()
        }
    }

    pub fn record(&self, usage: &TokenUsage) {
        self.input_tokens
            .fetch_add(usage.input_tokens as u64, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(usage.output_tokens as u64, Ordering::Relaxed);
        self.cache_creation_tokens
            .fetch_add(usage.cache_creation_input_tokens as u64, Ordering::Relaxed);
        self.cache_read_tokens
            .fetch_add(usage.cache_read_input_tokens as u64, Ordering::Relaxed);
    }

    pub fn total_input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
            + self.cache_creation_tokens.load(Ordering::Relaxed)
            + self.cache_read_tokens.load(Ordering::Relaxed)
    }

    pub fn total_output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    pub fn estimate_cost_usd(&self) -> f64 {
        let (input_price, output_price) = get_pricing(&self.model);
        let input_cost = self.total_input_tokens() as f64 / 1_000_000.0 * input_price;
        let output_cost = self.total_output_tokens() as f64 / 1_000_000.0 * output_price;
        input_cost + output_cost
    }

    pub fn snapshot(&self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens.load(Ordering::Relaxed) as u32,
            output_tokens: self.output_tokens.load(Ordering::Relaxed) as u32,
            cache_creation_input_tokens: self.cache_creation_tokens.load(Ordering::Relaxed) as u32,
            cache_read_input_tokens: self.cache_read_tokens.load(Ordering::Relaxed) as u32,
        }
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    (text.len() as f64 / 3.5).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        let long = "a".repeat(35);
        assert_eq!(estimate_tokens(&long), 10);
    }

    #[test]
    fn test_usage_tracker_record() {
        let tracker = UsageTracker::new("claude-sonnet-4-20250514");
        assert_eq!(tracker.total_input_tokens(), 0);
        assert_eq!(tracker.total_output_tokens(), 0);

        let u1 = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
        };
        tracker.record(&u1);

        let u2 = TokenUsage {
            input_tokens: 200,
            output_tokens: 100,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
        };
        tracker.record(&u2);

        assert_eq!(tracker.total_input_tokens(), 345);
        assert_eq!(tracker.total_output_tokens(), 150);

        let snap = tracker.snapshot();
        assert_eq!(snap.input_tokens, 300);
        assert_eq!(snap.output_tokens, 150);
    }

    #[test]
    fn test_usage_tracker_cost() {
        let tracker = UsageTracker::new("claude-3-haiku-20240307");
        let u = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        tracker.record(&u);

        let cost = tracker.estimate_cost_usd();
        let expected = 1_000_000.0 / 1_000_000.0 * 0.25 + 500_000.0 / 1_000_000.0 * 1.25;
        assert!((cost - expected).abs() < 0.001);
    }
}
