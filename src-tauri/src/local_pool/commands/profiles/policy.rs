use zenith_relay_core::CandidateQuota;

use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GatewayOAuthBindingRequest<'a> {
    Disabled,
    Account(&'a str),
}

pub(super) fn gateway_oauth_binding_request(
    disabled: bool,
    requested_account_id: Option<&str>,
) -> LocalResult<GatewayOAuthBindingRequest<'_>> {
    let requested_account_id = requested_account_id
        .map(str::trim)
        .filter(|account_id| !account_id.is_empty());
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
        GatewayOAuthBindingRequest::Disabled
    })
}

pub(super) fn profile_quota_rank(quota: CandidateQuota, allow_quota_wait: bool) -> Option<u64> {
    match quota {
        CandidateQuota::Available(remaining) => Some(remaining),
        CandidateQuota::Unknown | CandidateQuota::Stale | CandidateQuota::CreditFallback => Some(0),
        CandidateQuota::Exhausted if allow_quota_wait => Some(0),
        CandidateQuota::Exhausted => None,
    }
}

pub(super) fn prioritize_account_candidates(
    candidates: &mut Vec<(String, u64)>,
    preferred: Option<&str>,
) {
    candidates.sort_by(|left, right| {
        let left_preferred = Some(left.0.as_str()) == preferred;
        let right_preferred = Some(right.0.as_str()) == preferred;
        right_preferred
            .cmp(&left_preferred)
            .then_with(|| right.1.cmp(&left.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    candidates.dedup_by(|left, right| left.0 == right.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_quota_is_only_allowed_for_explicit_binding() {
        assert_eq!(
            profile_quota_rank(CandidateQuota::Available(1), false),
            Some(1)
        );
        assert_eq!(profile_quota_rank(CandidateQuota::Unknown, false), Some(0));
        assert_eq!(profile_quota_rank(CandidateQuota::Stale, false), Some(0));
        assert_eq!(
            profile_quota_rank(CandidateQuota::CreditFallback, false),
            Some(0)
        );
        assert_eq!(profile_quota_rank(CandidateQuota::Exhausted, false), None);
        assert_eq!(profile_quota_rank(CandidateQuota::Exhausted, true), Some(0));
    }

    #[test]
    fn manual_binding_keeps_the_explicit_account_first() {
        let mut candidates = vec![("selected".into(), 100), ("highest".into(), 9_000)];

        prioritize_account_candidates(&mut candidates, Some("selected"));

        assert_eq!(candidates[0], ("selected".to_string(), 100));
    }

    #[test]
    fn binding_request_distinguishes_disabled_and_manual() {
        assert_eq!(
            gateway_oauth_binding_request(true, None).unwrap(),
            GatewayOAuthBindingRequest::Disabled
        );
        assert_eq!(
            gateway_oauth_binding_request(false, None).unwrap(),
            GatewayOAuthBindingRequest::Disabled
        );
        assert_eq!(
            gateway_oauth_binding_request(false, Some(" account-a ")).unwrap(),
            GatewayOAuthBindingRequest::Account("account-a")
        );
        assert!(gateway_oauth_binding_request(true, Some("account-a")).is_err());
    }
}
