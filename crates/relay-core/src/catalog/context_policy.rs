use crate::pricing::TokenPrice;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// An explicit application preference. It never selects an economic tariff.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationContextPolicy {
    #[default]
    Client,
    Short,
    Extended,
    Custom {
        tokens: u64,
    },
    Configured {
        context: ContextWindowPolicy,
        compaction: CompactionPolicy,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextWindowPolicy {
    #[default]
    Client,
    Short,
    Extended,
    Custom {
        tokens: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompactionPolicy {
    #[default]
    Client,
    Early,
    Custom {
        tokens: u64,
    },
}

impl ApplicationContextPolicy {
    pub fn manages_window(self) -> bool {
        match self {
            Self::Client => false,
            Self::Configured { context, .. } => context != ContextWindowPolicy::Client,
            _ => true,
        }
    }

    pub fn manages_compaction(self) -> bool {
        match self {
            Self::Client => false,
            Self::Configured { compaction, .. } => compaction != CompactionPolicy::Client,
            _ => true,
        }
    }

    pub fn validate(self) -> Result<(), &'static str> {
        let (context, compaction) = match self {
            Self::Configured {
                context,
                compaction,
            } => (
                match context {
                    ContextWindowPolicy::Custom { tokens } => Some(tokens),
                    _ => None,
                },
                match compaction {
                    CompactionPolicy::Custom { tokens } => Some(tokens),
                    _ => None,
                },
            ),
            Self::Custom { tokens } => (Some(tokens), None),
            _ => (None, None),
        };
        for tokens in [context, compaction].into_iter().flatten() {
            if !(1_024..=2_000_000).contains(&tokens) {
                return Err("custom context must be between 1024 and 2000000 tokens");
            }
        }
        Ok(())
    }

    /// Missing route capacity never becomes an invented million-token window.
    /// Native cards supply their own ceiling; reference limits can only reduce it.
    pub fn apply(
        self,
        entry: &mut Value,
        context_limit: Option<u64>,
        input_limit: Option<u64>,
        output_limit: Option<u64>,
        _price: Option<TokenPrice>,
    ) {
        if !self.manages_window() && !self.manages_compaction() {
            return;
        }
        let (context, compaction) = match self {
            Self::Client => (ContextWindowPolicy::Client, CompactionPolicy::Client),
            Self::Short => (ContextWindowPolicy::Short, CompactionPolicy::Early),
            Self::Extended => (ContextWindowPolicy::Extended, CompactionPolicy::Early),
            Self::Custom { tokens } => (
                ContextWindowPolicy::Custom { tokens },
                CompactionPolicy::Early,
            ),
            Self::Configured {
                context,
                compaction,
            } => (context, compaction),
        };
        let positive = |value: Option<u64>| value.filter(|number| *number > 0);
        let default = positive(entry.get("context_window").and_then(Value::as_u64));
        let native_max = positive(entry.get("max_context_window").and_then(Value::as_u64));
        let maximum = [native_max, positive(context_limit)]
            .into_iter()
            .flatten()
            .min()
            .or(default);
        let Some(maximum) = maximum else { return };
        let target = match context {
            ContextWindowPolicy::Client => default.unwrap_or(maximum),
            ContextWindowPolicy::Short => default.unwrap_or(272_000).min(272_000),
            ContextWindowPolicy::Extended => 1_000_000,
            ContextWindowPolicy::Custom { tokens } => tokens,
        };
        let window = target.min(maximum);
        let effective_window = if self.manages_window() {
            window
        } else {
            entry
                .get("effective_context_window_percent")
                .and_then(Value::as_u64)
                .filter(|percent| (1..=100).contains(percent))
                .map(|percent| window.saturating_mul(percent) / 100)
                .unwrap_or(window)
        };
        // Protect the route's hard context ceiling using its known output limit.
        let hard_output_reserve = positive(output_limit).unwrap_or(8_192).min(maximum / 4);
        // A short application window also keeps room for ordinary responses.
        let output_reserve = hard_output_reserve.min(32_768).min(window / 4);
        let input_budget = positive(input_limit)
            .unwrap_or(effective_window)
            .min(effective_window)
            .min(window.saturating_sub(output_reserve))
            .min(maximum.saturating_sub(hard_output_reserve));
        // Leave room for the next tool result/message before client compaction starts.
        let margin = (input_budget / 20).max(1_024).min(input_budget / 4);
        let compact = input_budget.saturating_sub(margin).max(1);
        if self.manages_window() {
            entry["context_window"] = json!(window);
            entry["max_context_window"] = json!(maximum);
        }
        match compaction {
            CompactionPolicy::Client => {
                if self.manages_window() {
                    if let Some(original) = entry
                        .get("auto_compact_token_limit")
                        .and_then(Value::as_u64)
                    {
                        entry["auto_compact_token_limit"] = json!(original.min(compact));
                    }
                }
            }
            CompactionPolicy::Early | CompactionPolicy::Custom { .. } => {
                let threshold = match compaction {
                    CompactionPolicy::Custom { tokens } => tokens.min(compact),
                    _ => compact,
                };
                entry["auto_compact_token_limit"] = json!(threshold);
                // Keep the application's native effective window in default context mode.
                if self.manages_window() {
                    entry["effective_context_window_percent"] = json!(100);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::{LongContextRates, TokenRateSet};

    #[test]
    fn client_policy_preserves_all_native_fields() {
        let mut entry = json!({"context_window": 272000, "auto_compact_token_limit": 240000});
        let original = entry.clone();
        ApplicationContextPolicy::Client.apply(&mut entry, Some(1050000), None, None, None);
        assert_eq!(entry, original);
    }

    #[test]
    fn custom_budget_clamps_to_route_and_compacts_before_input_and_output_limits() {
        let mut entry = json!({"context_window": 64000, "max_context_window": 128000});
        ApplicationContextPolicy::Custom { tokens: 1_000_000 }.apply(
            &mut entry,
            Some(200000),
            Some(100000),
            Some(32000),
            None,
        );
        assert_eq!(entry["context_window"], 128000);
        assert_eq!(entry["max_context_window"], 128000);
        assert_eq!(entry["auto_compact_token_limit"], 91200);
    }

    #[test]
    fn extended_window_reserves_a_large_output_against_the_hard_ceiling() {
        let mut entry = json!({"context_window": 272000, "max_context_window": 1050000});
        ApplicationContextPolicy::Extended.apply(&mut entry, None, None, Some(128000), None);
        assert_eq!(entry["context_window"], 1000000);
        assert_eq!(entry["auto_compact_token_limit"], 875900);
        assert!(entry["auto_compact_token_limit"].as_u64().unwrap() + 128000 < 1050000);
        let mut smaller = json!({"context_window": 272000, "max_context_window": 872000});
        ApplicationContextPolicy::Extended.apply(&mut smaller, None, None, None, None);
        assert_eq!(smaller["context_window"], 872000);
    }

    #[test]
    fn numeric_presets_ignore_price_boundaries_and_unknown_capacity_stays_unknown() {
        let price = TokenPrice {
            input: 1,
            output: 2,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            flex: TokenRateSet::EMPTY,
            priority: TokenRateSet::EMPTY,
            above_200k: LongContextRates {
                standard: TokenRateSet {
                    input: Some(2),
                    ..Default::default()
                },
                ..Default::default()
            },
            above_272k: LongContextRates::EMPTY,
            ..Default::default()
        };
        let mut entry = json!({"context_window": 272000, "max_context_window": 1000000});
        ApplicationContextPolicy::Short.apply(&mut entry, None, None, Some(32000), Some(price));
        assert_eq!(entry["context_window"], 272000);
        assert!(entry["auto_compact_token_limit"].as_u64().unwrap() < 272000);
        ApplicationContextPolicy::Short.apply(&mut entry, Some(128000), None, None, None);
        assert_eq!(entry["context_window"], 128000);
        ApplicationContextPolicy::Extended.apply(&mut entry, Some(128000), None, None, None);
        assert_eq!(entry["context_window"], 128000);
        let mut unknown = json!({"slug": "unknown"});
        ApplicationContextPolicy::Extended.apply(&mut unknown, None, None, None, Some(price));
        assert!(unknown.get("context_window").is_none());
    }

    #[test]
    fn compaction_is_independent_and_preserves_the_native_window() {
        let mut entry = json!({"context_window": 272000, "max_context_window": 1050000, "effective_context_window_percent": 95});
        ApplicationContextPolicy::Configured {
            context: ContextWindowPolicy::Client,
            compaction: CompactionPolicy::Custom { tokens: 150000 },
        }
        .apply(&mut entry, None, None, Some(32000), None);
        assert_eq!(entry["context_window"], 272000);
        assert_eq!(entry["max_context_window"], 1050000);
        assert_eq!(entry["auto_compact_token_limit"], 150000);
        assert_eq!(entry["effective_context_window_percent"], 95);
    }

    #[test]
    fn default_compaction_keeps_native_fields_and_custom_threshold_is_clamped() {
        let original = json!({"context_window": 272000, "max_context_window": 1050000, "auto_compact_token_limit": 230000});
        let mut entry = original.clone();
        ApplicationContextPolicy::Configured {
            context: ContextWindowPolicy::Client,
            compaction: CompactionPolicy::Client,
        }
        .apply(&mut entry, None, None, None, None);
        assert_eq!(entry, original);
        ApplicationContextPolicy::Configured {
            context: ContextWindowPolicy::Custom { tokens: 128000 },
            compaction: CompactionPolicy::Custom { tokens: 1000000 },
        }
        .apply(&mut entry, None, None, Some(32000), None);
        assert_eq!(entry["context_window"], 128000);
        assert_eq!(entry["auto_compact_token_limit"], 91200);
        assert!(ApplicationContextPolicy::Configured {
            context: ContextWindowPolicy::Client,
            compaction: CompactionPolicy::Custom { tokens: 1 },
        }
        .validate()
        .is_err());
    }
}
