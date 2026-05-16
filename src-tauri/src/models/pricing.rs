use serde::{Deserialize, Serialize};

use super::provider::ProviderKind;

/// W22: token usage as parsed from a provider response. `total_tokens`
/// equals `prompt + completion + reasoning` when reasoning is reported
/// separately; otherwise it equals what the provider sent verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageReport {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
    pub total_tokens: u32,
}

impl UsageReport {
    pub fn new(prompt: u32, completion: u32, reasoning: Option<u32>) -> Self {
        let total = prompt
            .saturating_add(completion)
            .saturating_add(reasoning.unwrap_or(0));
        Self {
            prompt_tokens: prompt,
            completion_tokens: completion,
            reasoning_tokens: reasoning,
            total_tokens: total,
        }
    }
}

/// W22: per-1M token unit pricing. Reasoning column is optional because
/// most providers bill reasoning at the completion rate; OpenAI o-series
/// and a handful of OpenRouter aliases bill it separately.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelPricing {
    pub input_usd_per_1m: f64,
    pub output_usd_per_1m: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_usd_per_1m: Option<f64>,
}

impl ModelPricing {
    pub fn cost_for(&self, usage: &UsageReport) -> f64 {
        let input = (usage.prompt_tokens as f64) * self.input_usd_per_1m / 1_000_000.0;
        let output = (usage.completion_tokens as f64) * self.output_usd_per_1m / 1_000_000.0;
        let reasoning = usage
            .reasoning_tokens
            .map(|count| {
                let rate = self.reasoning_usd_per_1m.unwrap_or(self.output_usd_per_1m);
                (count as f64) * rate / 1_000_000.0
            })
            .unwrap_or(0.0);
        input + output + reasoning
    }
}

/// W22: user-editable override entry stored in `pricing_overrides.json`.
/// `model_pattern` is a case-insensitive substring matched against the
/// resolved model id; the first matching entry wins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricingOverride {
    pub model_pattern: String,
    #[serde(default)]
    pub provider_kind: Option<ProviderKind>,
    pub input_usd_per_1m: f64,
    pub output_usd_per_1m: f64,
    #[serde(default)]
    pub reasoning_usd_per_1m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PricingOverridesFile {
    #[serde(default)]
    pub overrides: Vec<ModelPricingOverride>,
}

/// Static seed table — covers what Datrina actually ships against. Adding
/// a model here is preferable to relying on overrides for everyone.
/// Rates are USD per 1M tokens as of 2026-05. Update when providers
/// publish new pricing; the user can also override via the JSON file
/// without a rebuild.
const SEED_PRICING: &[(&str, ModelPricing)] = &[
    // Moonshot / Kimi (via OpenRouter)
    (
        "kimi-k2",
        ModelPricing {
            input_usd_per_1m: 0.60,
            output_usd_per_1m: 2.50,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "moonshot",
        ModelPricing {
            input_usd_per_1m: 0.60,
            output_usd_per_1m: 2.50,
            reasoning_usd_per_1m: None,
        },
    ),
    // Anthropic
    (
        "claude-opus-4",
        ModelPricing {
            input_usd_per_1m: 15.0,
            output_usd_per_1m: 75.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "claude-sonnet-4",
        ModelPricing {
            input_usd_per_1m: 3.0,
            output_usd_per_1m: 15.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "claude-haiku-4",
        ModelPricing {
            input_usd_per_1m: 0.80,
            output_usd_per_1m: 4.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "claude-3.5-sonnet",
        ModelPricing {
            input_usd_per_1m: 3.0,
            output_usd_per_1m: 15.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "claude-3-haiku",
        ModelPricing {
            input_usd_per_1m: 0.25,
            output_usd_per_1m: 1.25,
            reasoning_usd_per_1m: None,
        },
    ),
    // OpenAI
    (
        "gpt-4o-mini",
        ModelPricing {
            input_usd_per_1m: 0.15,
            output_usd_per_1m: 0.60,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "gpt-4o",
        ModelPricing {
            input_usd_per_1m: 2.50,
            output_usd_per_1m: 10.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "gpt-4.1",
        ModelPricing {
            input_usd_per_1m: 2.0,
            output_usd_per_1m: 8.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "o1-mini",
        ModelPricing {
            input_usd_per_1m: 3.0,
            output_usd_per_1m: 12.0,
            reasoning_usd_per_1m: Some(12.0),
        },
    ),
    (
        "o1",
        ModelPricing {
            input_usd_per_1m: 15.0,
            output_usd_per_1m: 60.0,
            reasoning_usd_per_1m: Some(60.0),
        },
    ),
    // Google
    (
        "gemini-1.5-pro",
        ModelPricing {
            input_usd_per_1m: 1.25,
            output_usd_per_1m: 5.0,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "gemini-1.5-flash",
        ModelPricing {
            input_usd_per_1m: 0.075,
            output_usd_per_1m: 0.30,
            reasoning_usd_per_1m: None,
        },
    ),
    // Mistral / DeepSeek (cheap workhorses people throw at autonomous runs)
    (
        "deepseek",
        ModelPricing {
            input_usd_per_1m: 0.27,
            output_usd_per_1m: 1.10,
            reasoning_usd_per_1m: None,
        },
    ),
    (
        "mistral-large",
        ModelPricing {
            input_usd_per_1m: 2.0,
            output_usd_per_1m: 6.0,
            reasoning_usd_per_1m: None,
        },
    ),
];

/// Lookup pricing for a provider+model pair. `overrides` is the parsed
/// `pricing_overrides.json`; entries there win over seed entries. The
/// match is a case-insensitive substring on the model id so OpenRouter
/// aliases like `moonshotai/kimi-k2.6-instruct` match `kimi-k2`.
pub fn pricing_for(
    provider_kind: ProviderKind,
    model: &str,
    overrides: &[ModelPricingOverride],
) -> Option<ModelPricing> {
    let model_lc = model.to_ascii_lowercase();

    for entry in overrides {
        if let Some(kind) = entry.provider_kind {
            if kind != provider_kind {
                continue;
            }
        }
        if entry.model_pattern.is_empty() {
            continue;
        }
        if model_lc.contains(&entry.model_pattern.to_ascii_lowercase()) {
            return Some(ModelPricing {
                input_usd_per_1m: entry.input_usd_per_1m,
                output_usd_per_1m: entry.output_usd_per_1m,
                reasoning_usd_per_1m: entry.reasoning_usd_per_1m,
            });
        }
    }

    if matches!(
        provider_kind,
        ProviderKind::LocalMock | ProviderKind::Ollama
    ) {
        return None;
    }

    SEED_PRICING
        .iter()
        .find(|(pattern, _)| model_lc.contains(*pattern))
        .map(|(_, pricing)| *pricing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_for_kimi_matches_alias() {
        let pricing = pricing_for(
            ProviderKind::Openrouter,
            "moonshotai/kimi-k2.6-instruct",
            &[],
        )
        .unwrap();
        let usage = UsageReport::new(1_000_000, 1_000_000, None);
        assert!((pricing.cost_for(&usage) - 3.10).abs() < 1e-9);
    }

    #[test]
    fn cost_uses_reasoning_when_provided() {
        let pricing = pricing_for(ProviderKind::Openrouter, "o1-mini", &[]).unwrap();
        let usage = UsageReport::new(1_000_000, 1_000_000, Some(1_000_000));
        // 3 + 12 + 12 = 27
        assert!((pricing.cost_for(&usage) - 27.0).abs() < 1e-9);
    }

    #[test]
    fn local_mock_has_no_price() {
        assert!(pricing_for(ProviderKind::LocalMock, "anything", &[]).is_none());
    }

    #[test]
    fn override_wins_over_seed() {
        let overrides = vec![ModelPricingOverride {
            model_pattern: "kimi-k2".to_string(),
            provider_kind: None,
            input_usd_per_1m: 0.10,
            output_usd_per_1m: 0.20,
            reasoning_usd_per_1m: None,
        }];
        let pricing = pricing_for(ProviderKind::Openrouter, "kimi-k2.6", &overrides).unwrap();
        assert_eq!(pricing.input_usd_per_1m, 0.10);
        assert_eq!(pricing.output_usd_per_1m, 0.20);
    }
}
