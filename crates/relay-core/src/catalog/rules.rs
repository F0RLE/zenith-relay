use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRules {
    pub allowed: BTreeSet<String>,
    pub excluded: BTreeSet<String>,
}

impl ModelRules {
    pub fn allows(&self, model: &str) -> bool {
        !self.excluded.iter().any(|rule| matches(rule, model))
            && (self.allowed.is_empty() || self.allowed.iter().any(|rule| matches(rule, model)))
    }
}

fn matches(rule: &str, model: &str) -> bool {
    let rule = rule.trim();
    if rule == "*" {
        return true;
    }
    rule.strip_suffix('*').map_or_else(
        || rule.eq_ignore_ascii_case(model),
        |prefix| {
            model
                .get(..prefix.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusions_override_case_insensitive_allow_rules() {
        let rules = ModelRules {
            allowed: ["gpt-*".to_string()].into(),
            excluded: ["GPT-5-private".to_string()].into(),
        };

        assert!(rules.allows("GPT-4.1"));
        assert!(!rules.allows("gpt-5-private"));
        assert!(!rules.allows("claude-3"));
    }
}
