use super::*;
use crate::scheduler::{CandidateKind, CandidateQuota};
use crate::ModelRules;
use std::collections::{BTreeSet, HashSet};

fn candidate(id: &str) -> RuntimeCandidate {
    RuntimeCandidate {
        id: id.to_string(),
        kind: CandidateKind::ApiSource,
        source_id: id.to_string(),
        account_id: None,
        protocol: WireApi::Responses,
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        models: ["gpt-5".to_string()].into(),
        model_rules: ModelRules::default(),
        health: CandidateHealth::Healthy,
        quota: CandidateQuota::Unknown,
        quota_updated_at_ms: None,
        quota_reset_at_ms: None,
        cooldowns: BTreeMap::new(),
        last_used_at: None,
        consecutive_failures: 0,
        secret_available: true,
    }
}

fn oauth_candidate(id: &str) -> RuntimeCandidate {
    RuntimeCandidate {
        kind: CandidateKind::OAuthAccount,
        account_id: Some(id.to_string()),
        ..candidate(id)
    }
}

fn select(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
    scheduler.select(SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
        scope: &CandidateScope::default(),
        tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    })
}

fn select_image(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
    scheduler.select_image(SelectionRequest {
        model: "gpt-image-2",
        allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
        scope: &CandidateScope::default(),
        tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    })
}

#[test]
fn runtime_snapshot_keeps_the_management_wire_shape() {
    let snapshot = CandidateRuntimeSnapshot {
        candidate_id: "source".into(),
        kind: CandidateKind::ApiSource,
        available: true,
        in_flight: 0,
        active_request_count: 0,
        active_models: Vec::new(),
        model_retries: Vec::new(),
        last_used_at_ms: None,
        next_retry_at_ms: None,
        half_open: false,
        dispatches: 0,
    };

    let value = serde_json::to_value(snapshot).unwrap();
    assert_eq!(value["activeRequestCount"], 0);
    assert_eq!(value["activeModels"], serde_json::json!([]));
    assert_eq!(value["modelRetries"], serde_json::json!([]));
    assert!(value.get("active_request_count").is_none());
}

#[test]
fn image_lane_is_separate_from_text_load_and_caps_each_oauth_account() {
    let mut first = oauth_candidate("first");
    first.models.insert("gpt-image-2".to_string());
    let mut second = oauth_candidate("second");
    second.models.insert("gpt-image-2".to_string());
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(first);
    scheduler.upsert(second);

    assert!(scheduler.reserve_for("first", "gpt-5", 100));
    let image = select_image(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(image.candidate_id, "first");
    assert_eq!(image.diagnostics.in_flight_before, 0);
    assert!(scheduler.reserve_image_for("first", "gpt-image-2", 100));

    let next_image = select_image(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(next_image.candidate_id, "second");
    let text = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(text.candidate_id, "second");
    assert_eq!(text.diagnostics.in_flight_before, 0);

    assert!(scheduler.release_image_for("first", Some("gpt-image-2")));
    assert!(scheduler.release_for("first", Some("gpt-5")));
}

#[test]
fn oauth_text_leases_allow_parallel_account_requests() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("oauth"));
    scheduler.upsert(candidate("api"));

    assert!(scheduler.reserve_for("oauth", "gpt-5", 100));
    assert!(scheduler.reserve_for("oauth", "gpt-5", 100));
    assert!(scheduler.reserve_for("api", "gpt-5", 100));
    assert!(scheduler.reserve_for("api", "gpt-5", 100));

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "oauth"
    );
    assert!(scheduler.release_for("oauth", Some("gpt-5")));
    assert!(scheduler.release_for("oauth", Some("gpt-5")));
    assert!(scheduler.release_for("api", Some("gpt-5")));
    assert!(scheduler.release_for("api", Some("gpt-5")));
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
    let mut stale = candidate("stale");
    stale.quota = CandidateQuota::Stale;
    candidates.push(stale);
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
fn selection_orders_api_priority_then_quota_and_stable_id() {
    let mut scheduler = PoolScheduler::new();
    let mut low_priority = candidate("a-low-priority");
    low_priority.priority = 1;
    low_priority.quota = CandidateQuota::Available(100);
    scheduler.upsert(low_priority);
    let mut high_priority = candidate("z-high-priority");
    high_priority.priority = 100;
    high_priority.quota = CandidateQuota::Available(1);
    scheduler.upsert(high_priority);
    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "z-high-priority");
    assert_eq!(selected.diagnostics.reason, SelectionReason::ManualPriority);

    scheduler = PoolScheduler::new();
    let mut unknown = candidate("unknown");
    unknown.quota = CandidateQuota::Unknown;
    scheduler.upsert(unknown);
    let mut known_low = candidate("known-low");
    known_low.quota = CandidateQuota::Available(1);
    scheduler.upsert(known_low);
    let mut known_high = candidate("known-high");
    known_high.quota = CandidateQuota::Available(2);
    scheduler.upsert(known_high);
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "known-high"
    );

    scheduler = PoolScheduler::new();
    let mut old = candidate("old");
    old.last_used_at = Some(1);
    scheduler.upsert(old);
    let mut new = candidate("new");
    new.last_used_at = Some(2);
    scheduler.upsert(new);
    scheduler.upsert(candidate("never"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "never"
    );

    scheduler = PoolScheduler::new();
    let mut light = candidate("a-light");
    light.weight = 1;
    scheduler.upsert(light);
    let mut heavy = candidate("z-heavy");
    heavy.weight = 2;
    scheduler.upsert(heavy);
    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "a-light");
    assert_eq!(selected.diagnostics.reason, SelectionReason::StableTieBreak);

    scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("b"));
    scheduler.upsert(candidate("a"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "a"
    );
}

#[test]
fn api_source_order_stays_stable_across_concurrent_and_sequential_requests() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("active-source"));
    scheduler.upsert(candidate("other-source"));

    for _ in 0..2 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "active-source");
        assert!(scheduler.reserve(&selected.candidate_id));
    }
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "active-source"
    );

    assert!(scheduler.release("active-source"));
    assert!(scheduler.release("active-source"));
    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "active-source");
    assert_eq!(selected.diagnostics.reason, SelectionReason::StableTieBreak);

    let selected = select(
        &mut scheduler,
        &HashSet::from(["active-source".to_string()]),
    )
    .unwrap();
    assert_eq!(selected.candidate_id, "other-source");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::FallbackAttempt
    );
}

#[test]
fn oauth_quota_ignores_last_use_and_legacy_priority() {
    let mut scheduler = PoolScheduler::new();
    let mut low_quota = oauth_candidate("low-quota");
    low_quota.quota = CandidateQuota::Available(1);
    low_quota.priority = 100;
    low_quota.last_used_at = None;
    scheduler.upsert(low_quota);
    let mut high_quota = oauth_candidate("high-quota");
    high_quota.quota = CandidateQuota::Available(9_000);
    high_quota.priority = 1;
    high_quota.last_used_at = Some(99);
    scheduler.upsert(high_quota);

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "high-quota"
    );
}

#[test]
fn prompt_cache_affinity_applies_to_accounts_with_quota_and_load_guards() {
    let mut scheduler = PoolScheduler::new();
    let mut cached = oauth_candidate("cached");
    cached.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(cached);
    let mut fullest = oauth_candidate("fullest");
    fullest.quota = CandidateQuota::Available(5_400);
    scheduler.upsert(fullest);
    assert!(scheduler.bind_prompt_affinity("thread", "cached", 0));

    let select_thread = |scheduler: &mut PoolScheduler| {
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("thread"),
                now_ms: 1,
            })
            .unwrap()
    };

    let selected = select_thread(&mut scheduler);
    assert_eq!(selected.candidate_id, "cached");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::PromptCacheAffinity
    );

    assert!(scheduler.reserve("cached"));
    assert_eq!(select_thread(&mut scheduler).candidate_id, "cached");
    assert!(scheduler.reserve("cached"));
    assert_eq!(select_thread(&mut scheduler).candidate_id, "fullest");
    assert!(scheduler.release("cached"));
    assert!(scheduler.release("cached"));

    assert!(scheduler.update_candidate_availability(
        "fullest",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(5_501),
    ));
    assert_eq!(select_thread(&mut scheduler).candidate_id, "fullest");

    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("a"));
    scheduler.upsert(candidate("b"));
    assert!(scheduler.bind_prompt_affinity("thread", "b", 0));
    let selected = select_thread(&mut scheduler);
    assert_eq!(selected.candidate_id, "b");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::PromptCacheAffinity
    );
}

#[test]
fn prompt_cache_affinity_does_not_override_api_source_order() {
    let mut scheduler = PoolScheduler::new();
    let mut first = candidate("first");
    first.priority = 2;
    scheduler.upsert(first);
    let mut cached = candidate("cached");
    cached.priority = 1;
    scheduler.upsert(cached);
    assert!(scheduler.bind_prompt_affinity("thread", "cached", 0));

    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("thread"),
            now_ms: 1,
        })
        .unwrap();

    assert_eq!(selected.candidate_id, "first");
    assert_eq!(selected.diagnostics.reason, SelectionReason::ManualPriority);
}

#[test]
fn prompt_cache_affinity_wins_over_a_large_quota_difference() {
    let mut scheduler = PoolScheduler::new();
    let mut cached = oauth_candidate("cached");
    cached.quota = CandidateQuota::Available(1_000);
    scheduler.upsert(cached);
    let mut fullest = oauth_candidate("fullest");
    fullest.quota = CandidateQuota::Available(9_000);
    scheduler.upsert(fullest);
    assert!(scheduler.bind_prompt_affinity("cache:thread", "cached", 0));

    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("cache:thread"),
            now_ms: 1,
        })
        .unwrap();

    assert_eq!(selected.candidate_id, "cached");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::PromptCacheAffinity
    );
}

#[test]
fn sticky_prompt_affinity_does_not_rebind_to_spillover_candidate() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(oauth_candidate("owner"));
    scheduler.upsert(oauth_candidate("spillover"));
    assert!(scheduler.bind_prompt_affinity("session:thread", "owner", 0));

    assert!(!scheduler.bind_prompt_affinity_sticky("session:thread", "spillover", 1));
    let selected = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: Some("session:thread"),
            now_ms: 2,
        })
        .unwrap();
    assert_eq!(selected.candidate_id, "owner");

    scheduler.remove("owner");
    assert!(scheduler.bind_prompt_affinity_sticky("session:thread", "spillover", 3));
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: Some("session:thread"),
                now_ms: 4,
            })
            .unwrap()
            .candidate_id,
        "spillover"
    );
}

#[test]
fn oauth_equal_quota_uses_stable_order_without_last_use() {
    let mut scheduler = PoolScheduler::new();
    let mut high_priority = oauth_candidate("high-priority");
    high_priority.priority = 100;
    high_priority.last_used_at = Some(20);
    scheduler.upsert(high_priority);
    let mut low_priority = oauth_candidate("low-priority");
    low_priority.priority = 1;
    low_priority.last_used_at = Some(10);
    scheduler.upsert(low_priority);

    let first = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(first.candidate_id, "high-priority");
    assert!(scheduler.record_success("high-priority", "gpt-5", 30));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "high-priority"
    );
}

#[test]
fn api_source_roles_remain_strict_around_fair_oauth_routing() {
    let mut primary = PoolScheduler::new();
    let mut source = candidate("primary-source");
    source.priority = API_SOURCE_PRIMARY_PRIORITY;
    primary.upsert(source);
    primary.upsert(oauth_candidate("account"));
    let selected = select(&mut primary, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "primary-source");
    assert_eq!(selected.diagnostics.reason, SelectionReason::SourceRole);

    let mut reserve = PoolScheduler::new();
    let mut source = candidate("reserve-source");
    source.priority = API_SOURCE_RESERVE_PRIORITY;
    reserve.upsert(source);
    reserve.upsert(oauth_candidate("account"));
    let selected = select(&mut reserve, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "account");
    assert_eq!(selected.diagnostics.reason, SelectionReason::SourceRole);
    let selected = select(&mut reserve, &HashSet::from(["account".to_string()])).unwrap();
    assert_eq!(selected.candidate_id, "reserve-source");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::FallbackAttempt
    );

    let mut stabilizer = PoolScheduler::new();
    stabilizer.upsert(candidate("stabilizer-source"));
    stabilizer.upsert(oauth_candidate("account"));
    let selected = select(&mut stabilizer, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "account");
    assert_eq!(selected.diagnostics.reason, SelectionReason::SourceRole);
    assert!(stabilizer.reserve("account"));
    assert_eq!(
        select(&mut stabilizer, &HashSet::new())
            .unwrap()
            .candidate_id,
        "account"
    );
}

#[test]
fn stabilizer_sources_are_exhausted_by_priority_before_last_reserve() {
    let mut scheduler = PoolScheduler::new();
    for (id, priority) in [
        ("stabilizer-first", 300),
        ("stabilizer-second", 200),
        ("stabilizer-third", 100),
        ("last-reserve", API_SOURCE_RESERVE_PRIORITY),
    ] {
        let mut source = candidate(id);
        source.priority = priority;
        scheduler.upsert(source);
    }

    let mut tried = HashSet::new();
    for expected in [
        "stabilizer-first",
        "stabilizer-second",
        "stabilizer-third",
        "last-reserve",
    ] {
        let selected = select(&mut scheduler, &tried).unwrap();
        assert_eq!(selected.candidate_id, expected);
        tried.insert(selected.candidate_id);
    }
}

#[test]
fn active_and_sequential_requests_keep_the_highest_quota() {
    let mut scheduler = PoolScheduler::new();
    let mut full = candidate("full");
    full.quota = CandidateQuota::Available(100);
    scheduler.upsert(full);
    let mut low = candidate("low");
    low.quota = CandidateQuota::Available(1);
    scheduler.upsert(low);

    let first = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(first.candidate_id, "full");
    assert!(scheduler.reserve(&first.candidate_id));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "full"
    );
    assert!(scheduler.release(&first.candidate_id));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "full"
    );
}

#[test]
fn occupied_oauth_account_remains_eligible_for_text_selection() {
    let mut scheduler = PoolScheduler::new();
    let mut busy = oauth_candidate("busy");
    busy.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(busy);
    let mut free = oauth_candidate("free");
    free.quota = CandidateQuota::Available(4_999);
    scheduler.upsert(free);
    assert!(scheduler.reserve("busy"));

    let selected = select(&mut scheduler, &HashSet::new()).unwrap();

    assert_eq!(selected.candidate_id, "free");
    assert_eq!(selected.diagnostics.reason, SelectionReason::ParallelLoad);
}

#[test]
fn one_oauth_account_accepts_parallel_text_requests() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("only");
    account.quota = CandidateQuota::Available(5_000);
    scheduler.upsert(account);

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "only"
    );
    assert!(scheduler.reserve("only"));
    let second = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(second.candidate_id, "only");
    assert_eq!(second.diagnostics.in_flight_before, 1);
    assert!(scheduler.reserve("only"));
    assert!(scheduler.release("only"));
    assert!(scheduler.release("only"));
}

#[test]
fn higher_quota_account_remains_preferred_until_refresh() {
    let mut scheduler = PoolScheduler::new();
    let mut full = oauth_candidate("full");
    full.quota = CandidateQuota::Available(100);
    scheduler.upsert(full);
    let mut low = oauth_candidate("low");
    low.quota = CandidateQuota::Available(1);
    scheduler.upsert(low);

    for _ in 0..100 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "full");
        assert!(scheduler.reserve(&selected.candidate_id));
        assert!(scheduler.release(&selected.candidate_id));
    }
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "full"
    );
}

#[test]
fn equal_quota_sequential_requests_rotate_without_configured_weight() {
    let mut scheduler = PoolScheduler::new();
    for (id, weight) in [("full", 4), ("half", 2), ("quarter", 1)] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(5_000);
        account.weight = weight;
        scheduler.upsert(account);
    }

    let mut counts = BTreeMap::new();
    for _ in 0..70 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert!(scheduler.reserve(&selected.candidate_id));
        *counts.entry(selected.candidate_id.clone()).or_insert(0_u32) += 1;
        assert!(scheduler.release(&selected.candidate_id));
    }

    assert_eq!(counts.get("full"), Some(&24));
    assert_eq!(counts.get("half"), Some(&23));
    assert_eq!(counts.get("quarter"), Some(&23));
}

#[test]
fn quota_highest_uses_parallel_load_only_as_an_equal_quota_tie_break() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_routing_strategy(RoutingStrategy::QuotaHighest);
    for id in ["first", "second"] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(5_000);
        scheduler.upsert(account);
    }

    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "first");
    assert_eq!(selected.diagnostics.reason, SelectionReason::StableTieBreak);
    assert!(scheduler.reserve("first"));
    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "second");
    assert_eq!(selected.diagnostics.reason, SelectionReason::ParallelLoad);
    assert!(scheduler.release("first"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "first"
    );
}

#[test]
fn sequential_requests_use_the_greatest_refreshed_quota() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [("full", 10_000), ("half", 5_000), ("quarter", 2_500)] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
    }

    let mut counts = BTreeMap::new();
    for index in 0..70 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        if index == 0 {
            assert_eq!(selected.candidate_id, "full");
            assert_eq!(selected.diagnostics.reason, SelectionReason::QuotaHeadroom);
            assert_eq!(selected.diagnostics.eligible_candidates, 3);
            assert_eq!(
                selected.diagnostics.quota_remaining_basis_points,
                Some(10_000)
            );
            assert_eq!(selected.diagnostics.in_flight_before, 0);
        }
        assert!(scheduler.reserve(&selected.candidate_id));
        *counts.entry(selected.candidate_id.clone()).or_insert(0_u32) += 1;
        assert!(scheduler.release(&selected.candidate_id));
    }
    assert_eq!(counts.get("full"), Some(&70));
    assert_eq!(counts.get("half"), None);
    assert_eq!(counts.get("quarter"), None);
}

#[test]
fn quota_refresh_rebases_rotation_on_current_headroom() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [("first", 10_000), ("second", 1_000)] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
    }
    for _ in 0..11 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert!(scheduler.reserve(&selected.candidate_id));
        assert!(scheduler.release(&selected.candidate_id));
    }

    for id in ["first", "second"] {
        assert!(scheduler.update_candidate_availability(
            id,
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Available(5_000),
        ));
    }

    let first = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(first.candidate_id, "first");
    assert!(scheduler.reserve(&first.candidate_id));
    assert!(scheduler.release(&first.candidate_id));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "second"
    );
}

#[test]
fn concurrent_requests_follow_available_quota_headroom() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [("full", 10_000), ("half", 5_000), ("quarter", 2_500)] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
    }

    let mut counts = BTreeMap::new();
    for _ in 0..7 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert!(scheduler.reserve(&selected.candidate_id));
        *counts.entry(selected.candidate_id).or_insert(0_u32) += 1;
    }

    assert_eq!(counts.get("full"), Some(&7));
    assert_eq!(counts.get("half"), None);
    assert_eq!(counts.get("quarter"), None);
}

#[test]
fn concurrent_requests_fill_each_oauth_account_once() {
    let mut scheduler = PoolScheduler::new();
    for (id, quota) in [
        ("sixty-three", 6_300),
        ("fifty-four", 5_400),
        ("fifty-two", 5_200),
        ("fifty-one", 5_100),
    ] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
    }

    let mut counts = BTreeMap::new();
    for _ in 0..200 {
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert!(scheduler.reserve(&selected.candidate_id));
        *counts.entry(selected.candidate_id).or_insert(0_u32) += 1;
    }

    assert_eq!(counts, [("sixty-three".to_string(), 200)].into());
}

#[test]
fn subscription_expiry_routing_is_strict_and_places_unknown_dates_last() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_routing_strategy(RoutingStrategy::SubscriptionExpiry);
    for (id, expires_at_ms, quota) in [
        ("unknown", None, CandidateQuota::Available(100)),
        ("nearest", Some(10), CandidateQuota::Available(100)),
        ("later", Some(20), CandidateQuota::Available(10_000)),
        ("disabled-unknown", None, CandidateQuota::Available(10_000)),
        ("exhausted-unknown", None, CandidateQuota::Exhausted),
    ] {
        let mut account = oauth_candidate(id);
        account.quota = quota;
        account.enabled = id != "disabled-unknown";
        scheduler.upsert(account);
        assert!(scheduler.set_candidate_subscription_expiry(id, expires_at_ms));
    }

    assert_eq!(
        scheduler
            .runtime_order(0)
            .into_iter()
            .take(3)
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>(),
        ["nearest", "later", "unknown"]
    );

    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "nearest");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::SubscriptionExpiry
    );
    assert!(scheduler.reserve("nearest"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "nearest"
    );
    assert!(scheduler.release("nearest"));
    assert!(scheduler.update_candidate_availability(
        "nearest",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Exhausted,
    ));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "later"
    );
    assert!(scheduler.update_candidate_availability(
        "later",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Exhausted,
    ));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "unknown"
    );
}

#[test]
fn subscription_plan_routing_keeps_group_order_until_the_group_is_unavailable() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_routing_strategy(RoutingStrategy::SubscriptionPlan);
    scheduler.set_subscription_plan_order(&["business".into(), "plus".into(), "unknown".into()]);
    for (id, plan, quota) in [
        ("business", Some("Business"), 100),
        ("plus", Some("plus"), 10_000),
        ("unknown", None, 10_000),
    ] {
        let mut account = oauth_candidate(id);
        account.quota = CandidateQuota::Available(quota);
        scheduler.upsert(account);
        assert!(scheduler.set_candidate_subscription_plan(id, plan));
    }

    let selected = select(&mut scheduler, &HashSet::new()).unwrap();
    assert_eq!(selected.candidate_id, "business");
    assert_eq!(
        selected.diagnostics.reason,
        SelectionReason::SubscriptionPlan
    );
    assert!(scheduler.reserve("business"));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "business"
    );
    assert!(scheduler.release("business"));
    assert!(scheduler.update_candidate_availability(
        "business",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Exhausted,
    ));
    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "plus"
    );
}

#[test]
fn subscription_plan_order_is_normalized_and_bounded() {
    assert_eq!(
        normalize_subscription_plan_order(vec![
            " Business ".into(),
            "business".into(),
            "PLUS".into()
        ])
        .unwrap(),
        ["business", "plus"]
    );
    assert!(normalize_subscription_plan_order(vec!["bad\nplan".into()]).is_err());
}

#[test]
fn response_affinity_is_mandatory() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("creator"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "creator", 0));

    let scope = CandidateScope::default();
    let empty = HashSet::new();
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &empty,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(selection.candidate_id, "creator");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        selection.diagnostics.reason,
        SelectionReason::ResponseAffinity
    );

    scheduler.set_cooldown("creator", "gpt-5", 10);
    assert_eq!(
        scheduler.select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &empty,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        }),
        None,
        "a continuation cannot move to a candidate that did not create the response"
    );
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
    assert_eq!(scheduler.record_failure("candidate"), Some(1));
    assert_eq!(scheduler.record_failure("candidate"), Some(2));
    assert!(scheduler.record_success("candidate", "gpt-5", 102));
    assert_eq!(
        scheduler
            .candidate("candidate")
            .unwrap()
            .consecutive_failures,
        0
    );
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
    assert_eq!(scheduler.record_failure("candidate"), Some(1));
    assert!(scheduler.reset_failures("candidate"));
    assert_eq!(
        scheduler
            .candidate("candidate")
            .unwrap()
            .consecutive_failures,
        0
    );
}

#[test]
fn cooldown_classification_requires_every_source_to_be_cooled() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("first"));
    scheduler.upsert(candidate("second"));
    assert!(scheduler.set_cooldown_with_reason(
        "first",
        "gpt-5",
        10_000,
        CooldownReason::RateLimit,
    ));
    assert!(scheduler.set_cooldown_with_reason(
        "second",
        "gpt-5",
        10_000,
        CooldownReason::RateLimit,
    ));

    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let allowed_protocols = [WireApi::Responses];
    let request = || SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &allowed_protocols,
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 100,
    };
    assert_eq!(
        scheduler.all_applicable_cooldown(request()),
        Some((10_000, CooldownReason::RateLimit))
    );

    assert!(scheduler.set_cooldown_with_reason("first", "*", 5_000, CooldownReason::Transient,));
    assert_eq!(
        scheduler.all_applicable_cooldown(request()),
        Some((10_000, CooldownReason::Transient))
    );
    assert!(scheduler.clear_cooldown("first", "*"));
    assert_eq!(
        scheduler.all_applicable_cooldown(request()),
        Some((10_000, CooldownReason::RateLimit))
    );

    assert!(scheduler.set_cooldown_with_reason(
        "second",
        "gpt-5",
        20_000,
        CooldownReason::Transient,
    ));
    assert_eq!(
        scheduler.all_applicable_cooldown(request()),
        Some((10_000, CooldownReason::Transient))
    );
    assert!(scheduler.clear_cooldown("second", "gpt-5"));
    assert_eq!(scheduler.all_applicable_cooldown(request()), None);
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
fn transient_cooldown_waits_for_the_configured_failure_threshold() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(3, false);
    scheduler.upsert(candidate("candidate"));

    assert_eq!(scheduler.record_failure("candidate"), Some(1));
    assert!(!scheduler.set_cooldown_with_reason_for_model_at(
        "candidate",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert_eq!(scheduler.record_failure("candidate"), Some(2));
    assert!(!scheduler.set_cooldown_with_reason_for_model_at(
        "candidate",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert!(scheduler
        .candidate("candidate")
        .unwrap()
        .cooldowns
        .is_empty());
}

#[test]
fn transient_cooldown_is_applied_at_the_threshold_when_another_candidate_exists() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(3, true);
    scheduler.upsert(candidate("first"));
    scheduler.upsert(candidate("second"));
    for _ in 0..3 {
        assert!(scheduler.record_failure("first").is_some());
    }

    assert!(scheduler.set_cooldown_with_reason_for_model_at(
        "first",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert_eq!(
        scheduler.candidate("first").unwrap().cooldowns.get("gpt-5"),
        Some(&10_000)
    );
}

#[test]
fn last_candidate_stays_available_by_default() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(3, true);
    scheduler.upsert(candidate("only"));
    for _ in 0..3 {
        assert!(scheduler.record_failure("only").is_some());
    }

    assert!(!scheduler.set_cooldown_with_reason_for_model_at(
        "only",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert!(scheduler.candidate("only").unwrap().cooldowns.is_empty());
}

#[test]
fn last_candidate_respects_request_scope_and_protocols() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(3, true);
    scheduler.upsert(candidate("first"));
    scheduler.upsert(candidate("second"));
    for _ in 0..3 {
        assert!(scheduler.record_failure("first").is_some());
    }
    let request_scope = CandidateScope {
        source_ids: Some(["first".to_string()].into()),
        ..CandidateScope::default()
    };

    assert!(!scheduler.set_cooldown_with_reason_for_model_at(
        "first",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &request_scope,
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert!(scheduler.candidate("first").unwrap().cooldowns.is_empty());
}

#[test]
fn last_candidate_ignores_unusable_peers() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(1, true);
    scheduler.upsert(candidate("first"));
    let mut disabled = candidate("second");
    disabled.enabled = false;
    scheduler.upsert(disabled);
    assert_eq!(scheduler.record_failure("first"), Some(1));

    assert!(!scheduler.set_cooldown_with_reason_for_model_at(
        "first",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Transient,
            now_ms: 100,
        },
    ));
    assert!(scheduler.candidate("first").unwrap().cooldowns.is_empty());
}

#[test]
fn mandatory_cooldown_bypasses_the_transient_policy() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_cooldown_policy(8, true);
    scheduler.upsert(candidate("only"));

    assert!(scheduler.set_cooldown_with_reason_for_model_at(
        "only",
        CooldownRequest {
            scope: "gpt-5",
            policy_model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            request_scope: &CandidateScope::default(),
            retry_at_ms: 10_000,
            reason: CooldownReason::Mandatory,
            now_ms: 100,
        },
    ));
    assert_eq!(
        scheduler.candidate("only").unwrap().cooldowns.get("gpt-5"),
        Some(&10_000)
    );
}

#[test]
fn affinity_retry_time_uses_only_the_response_owner() {
    let mut scheduler = PoolScheduler::new();
    let mut owner = candidate("owner");
    owner.cooldowns.insert("gpt-5".into(), 300);
    scheduler.upsert(owner);
    let mut other = candidate("other");
    other.cooldowns.insert("gpt-5".into(), 200);
    scheduler.upsert(other);
    assert!(scheduler.bind_response_affinity("response", "owner", 100));

    let scope = CandidateScope::default();
    assert_eq!(
        scheduler.earliest_retry_at(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 100,
        }),
        Some(300)
    );
}

#[test]
fn expired_cooldown_allows_only_one_half_open_probe_per_model() {
    let mut scheduler = PoolScheduler::new();
    let mut recovering = candidate("recovering");
    recovering.cooldowns.insert("gpt-5".to_string(), 100);
    scheduler.upsert(recovering);
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = || SelectionRequest {
        model: "gpt-5",
        allowed_protocols: &[WireApi::Responses],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 101,
    };

    let first = scheduler.select(request()).unwrap();
    assert!(first.half_open_probe);
    assert!(scheduler.reserve_for(&first.candidate_id, "gpt-5", 101));
    assert!(scheduler.select(request()).is_none());
    assert!(scheduler.release_for(&first.candidate_id, Some("gpt-5")));
    assert!(scheduler.select(request()).unwrap().half_open_probe);
}

#[test]
fn expired_global_cooldown_allows_only_one_probe_across_models() {
    let mut scheduler = PoolScheduler::new();
    let mut recovering = candidate("recovering");
    recovering.models.insert("gpt-6".to_string());
    recovering.cooldowns.insert("*".to_string(), 100);
    scheduler.upsert(recovering);
    let scope = CandidateScope::default();
    let tried = HashSet::new();
    let request = |model| SelectionRequest {
        model,
        allowed_protocols: &[WireApi::Responses],
        scope: &scope,
        tried: &tried,
        response_affinity_key: None,
        prompt_affinity_key: None,
        now_ms: 101,
    };

    let first = scheduler.select(request("gpt-5")).unwrap();
    assert!(first.half_open_probe);
    assert!(scheduler.reserve_for(&first.candidate_id, "gpt-5", 101));
    assert!(scheduler.select(request("gpt-6")).is_none());
    assert!(scheduler.release_for(&first.candidate_id, Some("gpt-5")));
    assert!(scheduler.select(request("gpt-6")).unwrap().half_open_probe);
}

#[test]
fn runtime_order_uses_scheduler_preference_and_exposes_live_state() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("first"));
    scheduler.upsert(candidate("second"));

    let initial = scheduler.runtime_order(50);
    assert_eq!(initial[0].candidate_id, "first");
    assert!(initial.iter().all(|candidate| candidate.available));

    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    let loaded = scheduler.runtime_order(50);
    assert_eq!(loaded[0].candidate_id, "first");
    assert_eq!(loaded[0].in_flight, 1);
    assert_eq!(loaded[0].active_request_count, 1);
    assert_eq!(
        loaded[0].active_models,
        vec![ActiveModelRuntime {
            model: "gpt-5".into(),
            request_count: 1,
        }]
    );
    assert_eq!(loaded[0].dispatches, 1);
    assert_eq!(loaded[0].last_used_at_ms, None);
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 50,
            })
            .unwrap()
            .candidate_id,
        "first"
    );
    assert!(scheduler.record_success("first", "gpt-5", 75));
    assert_eq!(scheduler.runtime_order(75)[0].last_used_at_ms, Some(75));

    assert!(scheduler.set_cooldown("second", "gpt-5", 100));
    let cooling = scheduler.runtime_order(50);
    let second = cooling
        .iter()
        .find(|candidate| candidate.candidate_id == "second")
        .unwrap();
    assert!(!second.available);
    assert_eq!(second.next_retry_at_ms, Some(100));
    assert_eq!(
        second.model_retries,
        vec![ModelRetryRuntime {
            model: "gpt-5".into(),
            retry_at_ms: 100,
        }]
    );

    assert!(scheduler.reserve_for("second", "gpt-5", 101));
    let probing = scheduler.runtime_order(101);
    let second = probing
        .iter()
        .find(|candidate| candidate.candidate_id == "second")
        .unwrap();
    assert!(second.half_open);
    assert!(!second.available);
}

#[test]
fn runtime_order_groups_parallel_requests_by_active_model() {
    let mut scheduler = PoolScheduler::new();
    let mut first = candidate("first");
    first
        .models
        .extend(["claude-opus-5".to_string(), "gpt-image-2".to_string()]);
    scheduler.upsert(first);

    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    assert!(scheduler.reserve_for("first", "gpt-5", 50));
    assert!(scheduler.reserve_for("first", "claude-opus-5", 50));
    assert!(scheduler.reserve_image_for("first", "gpt-image-2", 50));

    let snapshot = scheduler.runtime_order(50).remove(0);
    assert_eq!(snapshot.in_flight, 3);
    assert_eq!(snapshot.active_request_count, 4);
    assert_eq!(
        snapshot.active_models,
        vec![
            ActiveModelRuntime {
                model: "claude-opus-5".into(),
                request_count: 1,
            },
            ActiveModelRuntime {
                model: "gpt-5".into(),
                request_count: 2,
            },
            ActiveModelRuntime {
                model: "gpt-image-2".into(),
                request_count: 1,
            },
        ]
    );

    assert!(scheduler.release_for("first", Some("gpt-5")));
    assert!(scheduler.release_for("first", Some("gpt-5")));
    assert!(scheduler.release_for("first", Some("claude-opus-5")));
    assert!(scheduler.release_image_for("first", Some("gpt-image-2")));
    let released = scheduler.runtime_order(50).remove(0);
    assert_eq!(released.active_request_count, 0);
    assert!(released.active_models.is_empty());
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
        Some(250)
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
    account.consecutive_failures = 7;
    scheduler.upsert(account);

    assert!(scheduler.set_cooldown("account", "gpt-5", 20_000));
    assert_eq!(scheduler.record_failure("account"), Some(8));
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
fn execution_fences_are_reference_counted_and_capability_blocks_are_model_scoped() {
    let mut scheduler = PoolScheduler::new();
    let mut account = oauth_candidate("account");
    account.models.insert("gpt-5-mini".into());
    scheduler.upsert(account);

    assert!(scheduler.set_execution_fence("account", true));
    assert!(scheduler.set_execution_fence("account", true));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler.set_execution_fence("account", false));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler.set_execution_fence("account", false));

    assert!(scheduler.block_capability("account", "gpt-5"));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5-mini",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
        .is_some());
    assert!(scheduler.clear_capability_blocks("account"));
    assert!(select(&mut scheduler, &HashSet::new()).is_some());
}

#[test]
fn near_equal_quota_prefers_the_account_that_resets_first() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_routing_strategy(RoutingStrategy::QuotaHighest);
    let mut earlier = oauth_candidate("earlier");
    earlier.quota = CandidateQuota::Available(5_000);
    earlier.quota_reset_at_ms = Some(1_000);
    let mut later = oauth_candidate("later");
    later.quota = CandidateQuota::Available(5_050);
    later.quota_reset_at_ms = Some(2_000);
    scheduler.upsert(earlier);
    scheduler.upsert(later);

    assert_eq!(
        select(&mut scheduler, &HashSet::new())
            .unwrap()
            .candidate_id,
        "earlier"
    );
}

#[test]
fn provider_model_storm_breaker_is_shared_by_source_and_clears_on_success() {
    let mut scheduler = PoolScheduler::new();
    scheduler.set_provider_storm_breaker_enabled(true);
    let first = candidate("first");
    let mut second = candidate("second");
    second.source_id = first.source_id.clone();
    scheduler.upsert(first);
    scheduler.upsert(second);

    assert!(!scheduler.record_provider_rate_limit("first", "gpt-5", 1));
    assert!(!scheduler.record_provider_rate_limit("first", "gpt-5", 2));
    assert!(scheduler.record_provider_rate_limit("first", "gpt-5", 3));
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    assert!(scheduler.record_success("first", "gpt-5", 4));
    assert!(select(&mut scheduler, &HashSet::new()).is_some());
}
