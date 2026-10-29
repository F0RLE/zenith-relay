use sha2::{Digest, Sha256};

pub(crate) const CLAUDE_DESKTOP_ALIAS_PREFIX: &str = "claude-sonnet-5-relay-";
const CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX: &str = "claude-fable-5-relay-";
const LEGACY_ALIAS_PREFIX: &str = "claude-zenith-";
const LEGACY_REASONING_ALIAS_PREFIX: &str = "claude-fable-5-zenith-";

/// A client-facing alias for the canonical upstream model.
/// Desktop rejects other vendors' names even inside a `claude-*` route. Hash
/// canonical model identity into a compact 64-bit suffix. Four-character hex
/// groups avoid vendor words and terminal snapshot dates without exposing the
/// upstream name or depending on the model's position in the pool.
pub fn claude_desktop_model_alias(model: &str) -> String {
    model_alias(CLAUDE_DESKTOP_ALIAS_PREFIX, model)
}

/// A client-only route that opts into Desktop's built-in effort picker.
/// The alias does not establish support for every value in that picker;
/// unsupported requests are rejected.
/// Never infer upstream capabilities, context, prices or effort from this name.
/// Desktop's Fable effort metadata matches a prefix; suffixed Sonnet/Opus
/// names do not inherit their exact-ID controls. The opaque suffix preserves
/// each model's distinct route.
pub fn claude_desktop_reasoning_model_alias(model: &str) -> String {
    model_alias(CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX, model)
}

pub(crate) fn is_claude_desktop_model_alias(model: &str) -> bool {
    alias_kind(model).is_some()
}

#[cfg(test)]
fn is_claude_desktop_reasoning_model_alias(model: &str) -> bool {
    alias_kind(model).is_some_and(|(prefix, _)| {
        matches!(
            prefix,
            CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX | LEGACY_REASONING_ALIAS_PREFIX
        )
    })
}

pub(crate) fn claude_desktop_model_alias_matches(alias: &str, model: &str) -> bool {
    let Some((prefix, legacy)) = alias_kind(alias) else {
        return false;
    };
    let digest = Sha256::digest(super::model_id_key(model).as_bytes());
    alias[prefix.len()..].eq_ignore_ascii_case(&alias_suffix(&digest, legacy))
}

fn alias_kind(model: &str) -> Option<(&'static str, bool)> {
    [
        (CLAUDE_DESKTOP_ALIAS_PREFIX, false),
        (CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX, false),
        (LEGACY_ALIAS_PREFIX, true),
        (LEGACY_REASONING_ALIAS_PREFIX, true),
    ]
    .into_iter()
    .find(|(prefix, _)| {
        model
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    })
}

fn model_alias(prefix: &str, model: &str) -> String {
    let digest = Sha256::digest(super::model_id_key(model).as_bytes());
    format!("{prefix}{}", alias_suffix(&digest, false))
}

fn alias_suffix(digest: &[u8], legacy: bool) -> String {
    if legacy {
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("-")
    } else {
        digest[..8]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| format!("{:02x}{:02x}", pair[0], pair[1]))
            .collect::<Vec<_>>()
            .join("-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_desktop_aliases_are_stable_bounded_and_vendor_neutral() {
        for model in [
            "gpt-6-astra".to_string(),
            "vendor/gpt-6.1-sol".to_string(),
            "vendor/模型".to_string(),
            "x".repeat(256),
        ] {
            for (prefix, alias_fn) in [
                (
                    CLAUDE_DESKTOP_ALIAS_PREFIX,
                    claude_desktop_model_alias as fn(&str) -> String,
                ),
                (
                    CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX,
                    claude_desktop_reasoning_model_alias,
                ),
            ] {
                let alias = alias_fn(&model);
                assert!(super::super::is_valid_model_token(&alias));
                assert!(is_claude_desktop_model_alias(&alias.to_uppercase()));
                assert_eq!(
                    is_claude_desktop_reasoning_model_alias(&alias.to_uppercase()),
                    prefix == CLAUDE_DESKTOP_REASONING_ALIAS_PREFIX
                );
                let suffix = alias.strip_prefix(prefix).unwrap();
                assert_eq!(suffix.split('-').count(), 4);
                assert!(suffix.split('-').all(|group| {
                    group.len() == 4 && group.bytes().all(|character| character.is_ascii_hexdigit())
                }));
                assert!(alias.len() <= 41);
                assert!(claude_desktop_model_alias_matches(&alias, &model));
                assert_eq!(alias, alias_fn(&model.to_ascii_uppercase()));
                assert_ne!(alias, alias_fn(&format!("other/{model}")));
            }
        }
        assert!(!is_claude_desktop_model_alias("claude-fable-5"));
        assert!(!is_claude_desktop_model_alias("claude-opus-5"));
    }

    #[test]
    fn legacy_desktop_routes_remain_valid_without_generating_them() {
        let model = "vendor/model";
        let digest = Sha256::digest(super::super::model_id_key(model).as_bytes());
        for prefix in [LEGACY_ALIAS_PREFIX, LEGACY_REASONING_ALIAS_PREFIX] {
            let alias = format!("{prefix}{}", alias_suffix(&digest, true));
            assert!(is_claude_desktop_model_alias(&alias));
            assert_eq!(
                is_claude_desktop_reasoning_model_alias(&alias),
                prefix == LEGACY_REASONING_ALIAS_PREFIX
            );
            assert!(claude_desktop_model_alias_matches(
                &alias.to_uppercase(),
                model
            ));
            assert!(!claude_desktop_model_alias_matches(&alias, "other/model"));
            assert!(!claude_desktop_model_alias_matches(
                &format!("{alias}-effort-high"),
                model
            ));
        }
    }
}
