use zenith_relay_core::CandidateQuota;

use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GatewayOAuthBindingRequest<'a> {
    Disabled,
    Automatic,
    Account(&'a str),
}

pub(super) fn gateway_oauth_binding_request(
    disabled: bool,
    requested_account_id: Option<&str>,
) -> LocalResult<GatewayOAuthBindingRequest<'_>> {
    let requested_account_id = requested_account_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if disabled && requested_account_id.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth binding cannot be disabled and assigned to an account together",
        ));
    }
    Ok(if disabled {
        GatewayOAuthBindingRequest::Disabled
    } else if let Some(account_id) = requested_account_id {
        GatewayOAuthBindingRequest::Account(account_id)
    } else {
        GatewayOAuthBindingRequest::Automatic
    })
}

pub(super) fn profile_quota_rank(quota: CandidateQuota, allow_quota_wait: bool) -> Option<u64> {
    match quota {
        CandidateQuota::Available(remaining) => Some(remaining),
        CandidateQuota::Unknown | CandidateQuota::Stale => Some(0),
        CandidateQuota::Exhausted if allow_quota_wait => Some(0),
        CandidateQuota::Exhausted => None,
    }
}

pub(super) fn prioritize_account_candidates(
    candidates: &mut Vec<(String, u64)>,
    preferred: Option<&str>,
    automatic: bool,
) {
    candidates.sort_by(|left, right| {
        let left_preferred = Some(left.0.as_str()) == preferred;
        let right_preferred = Some(right.0.as_str()) == preferred;
        if automatic {
            right
                .1
                .cmp(&left.1)
                .then_with(|| right_preferred.cmp(&left_preferred))
                .then_with(|| left.0.cmp(&right.0))
        } else {
            right_preferred
                .cmp(&left_preferred)
                .then_with(|| right.1.cmp(&left.1))
                .then_with(|| left.0.cmp(&right.0))
        }
    });
    candidates.dedup_by(|left, right| left.0 == right.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_binding_prefers_highest_quota() {
        let mut candidates = vec![
            ("account-z".into(), 9_000),
            ("account-a".into(), 8_000),
            ("account-m".into(), 1_000),
        ];

        prioritize_account_candidates(&mut candidates, Some("account-m"), true);

        assert_eq!(
            candidates,
            [
                ("account-z".to_string(), 9_000),
                ("account-a".to_string(), 8_000),
                ("account-m".to_string(), 1_000),
            ]
        );
    }

    #[test]
    fn automatic_binding_keeps_low_quota_fallbacks() {
        let mut candidates = vec![
            ("preferred".into(), 100),
            ("highest".into(), 9_000),
            ("available".into(), 5_000),
            ("unknown".into(), 0),
        ];

        prioritize_account_candidates(&mut candidates, Some("preferred"), true);

        assert_eq!(
            candidates,
            [
                ("highest".to_string(), 9_000),
                ("available".to_string(), 5_000),
                ("preferred".to_string(), 100),
                ("unknown".to_string(), 0),
            ]
        );
    }

    #[test]
    fn exhausted_quota_is_only_allowed_for_explicit_binding() {
        assert_eq!(
            profile_quota_rank(CandidateQuota::Available(1), false),
            Some(1)
        );
        assert_eq!(profile_quota_rank(CandidateQuota::Unknown, false), Some(0));
        assert_eq!(profile_quota_rank(CandidateQuota::Stale, false), Some(0));
        assert_eq!(profile_quota_rank(CandidateQuota::Exhausted, false), None);
        assert_eq!(profile_quota_rank(CandidateQuota::Exhausted, true), Some(0));
    }

    #[test]
    fn manual_binding_keeps_the_explicit_account_first() {
        let mut candidates = vec![("selected".into(), 100), ("highest".into(), 9_000)];

        prioritize_account_candidates(&mut candidates, Some("selected"), false);

        assert_eq!(candidates[0], ("selected".to_string(), 100));
    }

    #[test]
    fn binding_request_distinguishes_disabled_automatic_and_manual() {
        assert_eq!(
            gateway_oauth_binding_request(true, None).unwrap(),
            GatewayOAuthBindingRequest::Disabled
        );
        assert_eq!(
            gateway_oauth_binding_request(false, None).unwrap(),
            GatewayOAuthBindingRequest::Automatic
        );
        assert_eq!(
            gateway_oauth_binding_request(false, Some(" account-a ")).unwrap(),
            GatewayOAuthBindingRequest::Account("account-a")
        );
        assert!(gateway_oauth_binding_request(true, Some("account-a")).is_err());
    }
}
