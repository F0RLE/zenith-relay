use super::*;

#[test]
fn namespaced_api_model_never_falls_back_to_a_bare_oauth_model() {
    let mut scheduler = PoolScheduler::new();
    let mut api = candidate("api-slot");
    api.models = ["cpa/gpt-5.5".to_string()].into();
    api.cooldowns.insert("*".to_string(), 200);
    scheduler.upsert(api);

    let mut oauth = oauth_candidate("oauth-slot");
    oauth.models = ["gpt-5.5".to_string()].into();
    scheduler.upsert(oauth);

    let namespaced = scheduler.select(SelectionRequest {
        model: "cpa/gpt-5.5",
        allowed_protocols: &[WireApi::Responses],
        scope: &CandidateScope::default(),
        tried: &HashSet::new(),
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    });
    assert!(namespaced.is_none());

    let bare = scheduler
        .select(SelectionRequest {
            model: "gpt-5.5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .unwrap();
    assert_eq!(bare.candidate_id, "oauth-slot");
}
#[test]
fn availability_updates_take_effect_while_candidate_is_in_flight() {
    let mut scheduler = PoolScheduler::new();
    let first = oauth_candidate("first");
    let second = oauth_candidate("second");
    scheduler.upsert(first);
    scheduler.upsert(second);
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "first"
    );
    assert!(scheduler.reserve("first"));

    assert!(scheduler.update_candidate_availability(
        "first",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Exhausted,
    ));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "second"
    );
    assert!(scheduler.release("first"));
    assert!(!scheduler.update_candidate_availability(
        "missing",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Unknown,
    ));
    assert!(scheduler.set_candidate_health("second", CandidateHealth::Unhealthy));
    assert!(!scheduler.set_candidate_health("missing", CandidateHealth::Healthy));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
}
#[test]
fn stale_oauth_quota_stays_probeable_unless_it_protects_the_chatgpt_reserve() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("account");
    account.quota = CandidateQuota::Available(5_000);
    account.quota_updated_at_ms = Some(100);
    scheduler.upsert(account);
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = |now_ms| SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms,
    };

    assert!(scheduler
        .select(request(100 + QUOTA_STALE_AFTER_MS))
        .is_some());
    assert!(scheduler
        .select(request(101 + QUOTA_STALE_AFTER_MS))
        .is_some());
    assert!(scheduler.set_protected_candidate(Some("account"), 100));
    assert!(scheduler
        .select(request(101 + QUOTA_STALE_AFTER_MS))
        .is_none());
}
#[test]
fn hard_filters_reject_every_ineligible_candidate_state() {
    let mut candidates = Vec::new();

    let mut disabled = candidate("disabled");
    disabled.enabled = false;
    candidates.push(disabled);
    let mut draining = candidate("draining");
    draining.draining = true;
    candidates.push(draining);
    let mut no_secret = candidate("no-secret");
    no_secret.secret_available = false;
    candidates.push(no_secret);
    let mut wrong_model = candidate("wrong-model");
    wrong_model.models = ["other".to_string()].into();
    candidates.push(wrong_model);
    let mut excluded_model = candidate("excluded-model");
    excluded_model.model_rules.excluded = ["gpt-*".to_string()].into();
    candidates.push(excluded_model);
    let mut unhealthy = candidate("unhealthy");
    unhealthy.health = CandidateHealth::Unhealthy;
    candidates.push(unhealthy);
    for (id, health) in [
        ("reauth", CandidateHealth::ReauthRequired),
        ("checkpoint", CandidateHealth::Checkpoint),
        ("captcha", CandidateHealth::Captcha),
        ("blocked", CandidateHealth::Blocked),
        ("expired", CandidateHealth::Expired),
    ] {
        let mut blocked = candidate(id);
        blocked.health = health;
        candidates.push(blocked);
    }
    let mut exhausted = candidate("exhausted");
    exhausted.quota = CandidateQuota::Exhausted;
    candidates.push(exhausted);
    let mut zero_quota = candidate("zero-quota");
    zero_quota.quota = CandidateQuota::Available(0);
    candidates.push(zero_quota);
    let mut cooled = candidate("cooled");
    cooled.cooldowns.insert("gpt-5".to_string(), 101);
    candidates.push(cooled);
    let mut wrong_protocol = candidate("wrong-protocol");
    wrong_protocol.protocol = WireApi::Messages;
    candidates.push(wrong_protocol);

    let mut scheduler = PoolScheduler::new();
    for candidate in candidates {
        scheduler.upsert(candidate);
    }
    assert_eq!(select(&mut scheduler, &HashSet::new()), None);

    scheduler.upsert(candidate("ready"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "ready"
    );
    let scope = CandidateScope {
        source_ids: Some(["different-source".to_string()].into()),
        ..CandidateScope::default()
    };
    assert_eq!(
        scheduler.select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        None
    );

    let scope = CandidateScope {
        model_rules: ModelRules {
            excluded: ["gpt-*".to_string()].into(),
            ..ModelRules::default()
        },
        ..CandidateScope::default()
    };
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .is_none());
}
#[test]
fn cooldown_expires_and_success_clears_it_and_updates_last_used_timestamp() {
    let mut scheduler = PoolScheduler::new();
    let mut candidate = candidate("candidate");
    candidate.cooldowns.insert("gpt-5".to_string(), 101);
    candidate.cooldowns.insert("*".to_string(), 101);
    scheduler.upsert(candidate);
    assert_eq!(select(&mut scheduler, &HashSet::new()), None);

    assert!(!scheduler.record_success("candidate", "GPT-5", 90));
    assert_eq!(
        scheduler.candidate("candidate").unwrap().last_used_at,
        Some(90)
    );
    assert_eq!(
        scheduler
            .candidate("candidate")
            .unwrap()
            .cooldowns
            .get("gpt-5"),
        Some(&101)
    );
    assert!(scheduler.record_success("candidate", "GPT-5", 102));
    assert!(scheduler
        .candidate("candidate")
        .unwrap()
        .cooldowns
        .is_empty());
    assert!(select(&mut scheduler, &HashSet::new()).is_some());

    scheduler.set_cooldown("candidate", "gpt-5", 101);
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 101,
        })
        .is_some());

    assert!(scheduler.record_success("candidate", "gpt-5", 102));
}
#[test]
fn cooldown_updates_never_shorten_an_existing_retry_window() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("candidate"));
    assert!(scheduler.set_cooldown("candidate", "gpt-5", 10_000));
    assert!(scheduler.set_cooldown("candidate", "GPT-5", 2_000));
    assert_eq!(
        scheduler
            .candidate("candidate")
            .unwrap()
            .cooldowns
            .get("gpt-5"),
        Some(&10_000)
    );
}
#[test]
fn mandatory_cooldown_dominates_aggregate_reason() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("rate-limited"));
    scheduler.upsert(candidate("mandatory"));
    assert!(scheduler.set_cooldown_with_reason(
        "rate-limited",
        "gpt-5",
        20_000,
        CooldownReason::RateLimit,
    ));
    assert!(scheduler.set_cooldown_with_reason(
        "mandatory",
        "gpt-5",
        10_000,
        CooldownReason::Mandatory,
    ));

    let scope = CandidateScope::default();
    let allowed_protocols = [WireApi::Responses];
    assert_eq!(
        scheduler.all_applicable_cooldown(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &allowed_protocols,
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        Some((10_000, CooldownReason::Mandatory))
    );
}
#[test]
fn earliest_retry_ignores_candidates_blocked_for_non_cooldown_reasons() {
    let mut scheduler = PoolScheduler::new();
    let mut later = candidate("later");
    later.cooldowns.insert("gpt-5".to_string(), 300);
    scheduler.upsert(later);
    let mut sooner = candidate("sooner");
    sooner.cooldowns.insert("gpt-5".to_string(), 200);
    scheduler.upsert(sooner);
    let mut disabled = candidate("disabled");
    disabled.enabled = false;
    disabled.cooldowns.insert("gpt-5".to_string(), 150);
    scheduler.upsert(disabled);

    assert_eq!(
        scheduler.earliest_retry_at(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        Some(200)
    );

    let mut exhausted = candidate("exhausted");
    exhausted.quota = CandidateQuota::Exhausted;
    exhausted.cooldowns.insert("*".to_string(), 250);
    scheduler.upsert(exhausted);
    assert_eq!(
        scheduler.earliest_retry_at(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::from(["sooner".to_string()]),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        Some(300)
    );
}
#[test]
fn account_scope_allows_oauth_ready_candidate_shape() {
    let mut scheduler = PoolScheduler::new();
    let mut account = candidate("candidate-account");
    account.kind = CandidateKind::OAuthAccount;
    account.source_id = "openai".to_string();
    account.account_id = Some("account-1".to_string());
    scheduler.upsert(account);
    let scope = CandidateScope {
        account_ids: Some(BTreeSet::from(["account-1".to_string()])),
        ..CandidateScope::default()
    };

    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 0,
        })
        .is_some());
}
#[test]
fn oauth_candidates_honor_runtime_cooldowns_and_allow_stale_quota() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("account");
    account.quota = CandidateQuota::Stale;
    account.cooldowns.insert("gpt-5".into(), 10_000);
    scheduler.upsert(account);

    assert!(scheduler.set_cooldown("account", "gpt-5", 20_000));

    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    let snapshot = scheduler.runtime_order(100).remove(0);
    assert!(!snapshot.available);
    assert_eq!(snapshot.next_retry_at_ms, Some(20_000));
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 20_001,
        })
        .is_some());
}
#[test]
fn translated_protocols_share_the_same_scheduler() {
    let mut scheduler = PoolScheduler::new();
    let mut candidate = candidate("chat-source");
    candidate.protocol = WireApi::ChatCompletions;
    scheduler.upsert(candidate);

    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 0,
        })
        .is_some());
}
#[test]
fn explicit_empty_scope_selects_no_candidates() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("source"));
    let scope = CandidateScope {
        source_ids: Some(BTreeSet::new()),
        ..CandidateScope::default()
    };

    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 0,
        })
        .is_none());
}
#[test]
fn protected_account_keeps_its_quota_reserve() {
    let mut scheduler = PoolScheduler::new();
    let mut protected = oauth_candidate("protected");
    protected.quota = CandidateQuota::Available(100);
    scheduler.upsert(protected);
    let mut available = oauth_candidate("available");
    available.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(available);
    assert!(scheduler.set_protected_candidate(Some("protected"), 100));

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "available"
    );

    assert!(scheduler.update_candidate_availability(
        "protected",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(200),
    ));
    assert_eq!(
        scheduler.routing_quota_factor(scheduler.candidate("protected").unwrap()),
        100
    );
}
#[test]
fn protected_account_with_provider_credits_and_remaining_window_is_routable() {
    use crate::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind};

    let quota = QuotaSnapshot {
        primary: Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(4_200),
            explicitly_full: None,
            reset_at_ms: Some(500),
            window_minutes: Some(43_200),
            observed_at_ms: 100,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        provider_credits_available: true,
        available_credits_micro_units: Some(250_000_000),
        updated_at_ms: Some(100),
        ..Default::default()
    };
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("account");
    account.quota = CandidateQuota::from_snapshot(&quota, 100, 60_000);
    account.quota_updated_at_ms = quota.updated_at_ms;
    scheduler.upsert(account);
    assert!(scheduler.set_protected_candidate(Some("account"), 100));

    assert_eq!(
        scheduler.routing_quota_factor(scheduler.candidate("account").unwrap()),
        4_100
    );
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "account"
    );
    assert!(scheduler.runtime_order(100)[0].available);
}
#[test]
fn shared_source_rate_deadline_is_visible_to_rotation_and_expires_without_another_observation() {
    let mut scheduler = PoolScheduler::new();
    let first = candidate("first");
    let mut second = candidate("second");
    second.source_id = first.source_id.clone();
    scheduler.upsert(first);
    scheduler.upsert(second);

    for id in ["first", "second"] {
        assert!(scheduler.set_cooldown_with_reason(id, "gpt-5", 300, CooldownReason::RateLimit,));
    }
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = |now_ms| SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms,
    };
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert_eq!(scheduler.earliest_retry_at(request(100)), Some(300));
    assert!(scheduler.select(request(300)).is_some());
}
