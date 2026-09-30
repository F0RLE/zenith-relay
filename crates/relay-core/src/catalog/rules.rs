use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRules {
    pub allowed: BTreeSet<String>,
    pub excluded: BTreeSet<String>,
}

impl ModelRules {
    pub fn from_allow_deny(allowed: &[String], excluded: &[String]) -> Self {
        Self {
            allowed: allowed.iter().cloned().collect(),
            excluded: excluded.iter().cloned().collect(),
        }
    }

    pub fn allows(&self, model: &str) -> bool {
        if self.excluded.iter().any(|rule| matches(rule, model)) {
            return false;
        }
        self.allowed.is_empty()
            || self.allowed.iter().any(|rule| matches(rule, model))
            || self.exact_snapshot_includes_later_models()
    }

    /// A partial exact snapshot is the old editor's saved checkbox state.
    /// A model that was not known then stays on. A `*` rule remains a closed set,
    /// and an allow list without exclusions stays closed too.
    fn exact_snapshot_includes_later_models(&self) -> bool {
        !self.excluded.is_empty()
            && self
                .allowed
                .iter()
                .chain(&self.excluded)
                .all(|rule| !rule.contains('*'))
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

    #[test]
    fn allow_deny_lists_use_the_same_rules_as_routing() {
        let rules =
            ModelRules::from_allow_deny(&["gpt-*".to_string()], &["GPT-5-private".to_string()]);

        assert!(rules.allows("GPT-4.1"));
        assert!(!rules.allows("gpt-5-private"));
        assert!(!rules.allows("claude-3"));
    }

    #[test]
    fn exact_member_snapshot_enables_a_later_model() {
        let rules = ModelRules::from_allow_deny(&["gpt-5.4".to_string()], &["gpt-old".to_string()]);

        assert!(rules.allows("gpt-5.5"));
        assert!(rules.allows("GPT-5.4"));
        assert!(!rules.allows("gpt-old"));

        let allow_only = ModelRules::from_allow_deny(&["gpt-5.4".to_string()], &[]);
        assert!(allow_only.allows("gpt-5.4"));
        assert!(!allow_only.allows("gpt-5.5"));

        let wildcard = ModelRules::from_allow_deny(&["gpt-*".to_string()], &[]);
        assert!(wildcard.allows("gpt-5.5"));
        assert!(!wildcard.allows("claude-new"));

        let wildcard_with_exclusion =
            ModelRules::from_allow_deny(&["gpt-*".to_string()], &["gpt-old".to_string()]);
        assert!(wildcard_with_exclusion.allows("gpt-5.5"));
        assert!(!wildcard_with_exclusion.allows("gpt-old"));
        assert!(!wildcard_with_exclusion.allows("claude-new"));
    }
}
