use super::*;
mod automatic;
mod policy;
mod preview;
mod recovery;
mod rotation;
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
        provider_credits_micro_units: None,
        provider_credits_unlimited: false,
        quota_updated_at_ms: None,
        quota_reset_at_ms: None,
        cooldowns: BTreeMap::new(),
        last_used_at: None,

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
fn runtime_snapshot_keeps_the_management_wire_shape() {
    let snapshot = CandidateRuntimeSnapshot {
        candidate_id: "source".into(),
        kind: CandidateKind::ApiSource,
        available: true,
        next_for_new_request: false,
        activity_revision: 0,
        runtime_id: 0,
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
    assert_eq!(image.candidate_id, "second");
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
        "api"
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
fn removing_a_busy_candidate_drains_its_lease_before_final_removal() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("busy"));
    assert!(scheduler.reserve_for("busy", "gpt-5", 100));

    assert!(scheduler.remove("busy").is_some());
    // The candidate is no longer selectable, but its activity remains visible
    // so the in-flight request can release its lease normally.
    assert!(scheduler.candidate("busy").is_some());
    assert_eq!(scheduler.runtime_activity_for("busy").1, 1);
    assert!(select(&mut scheduler, &HashSet::new()).is_none());

    assert!(scheduler.release_for("busy", Some("gpt-5")));
    assert!(scheduler.candidate("busy").is_none());
    assert_eq!(scheduler.runtime_activity_for("busy").1, 0);
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

    assert_eq!(
        counts,
        [
            ("sixty-three".into(), 50),
            ("fifty-four".into(), 50),
            ("fifty-two".into(), 50),
            ("fifty-one".into(), 50),
        ]
        .into()
    );
    for (id, count) in counts {
        for _ in 0..count {
            assert!(scheduler.release(&id));
        }
    }
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
fn response_affinity_owner_outside_key_scope_can_be_reset_for_fallback() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    let scope = CandidateScope {
        source_ids: Some(BTreeSet::from(["fallback".to_string()])),
        ..CandidateScope::default()
    };
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(false),
        "a removed pool member must not keep an opaque response pinned forever"
    );

    // Affinity selection remains mandatory until the caller resets the
    // opaque continuation, preserving the safety rule for a still-configured
    // owner while allowing the request layer to make the membership change
    // explicit before choosing the fallback.
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .is_none());
    assert!(scheduler.invalidate_response_affinity("response"));
    assert_eq!(
        scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 1,
            })
            .unwrap()
            .candidate_id,
        "fallback"
    );
}

#[test]
fn optional_affinity_reports_a_temporarily_unavailable_owner() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    scheduler.upsert(candidate("fallback"));
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    let scope = CandidateScope::default();
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true)
    );
    assert_eq!(
        scheduler.response_affinity_owner_is_eligible(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true)
    );

    assert!(scheduler.set_candidate_health("owner", CandidateHealth::ReauthRequired));
    assert_eq!(
        scheduler.response_affinity_owner_supports_route(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(true),
        "reauth is a temporary availability state, not a route-shape change"
    );
    assert_eq!(
        scheduler.response_affinity_owner_is_eligible(
            "response",
            "gpt-5",
            &[WireApi::Responses],
            &scope,
            1,
        ),
        Some(false)
    );
}

#[test]
fn removed_candidate_keeps_response_owner_only_until_a_replacement_id_is_upserted() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    scheduler.upsert(candidate("fallback"));
    assert!(scheduler.bind_response_affinity("response", "owner", 0));

    assert!(scheduler.remove("owner").is_some());
    assert_eq!(
        scheduler.response_affinity_candidate("response", 1),
        Some("owner".into())
    );
    assert!(scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &HashSet::new(),
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .is_none());

    scheduler.upsert(candidate("owner"));
    assert_eq!(scheduler.response_affinity_candidate("response", 1), None);
}

#[test]
fn invalidated_response_affinity_allows_retry_on_another_candidate() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("owner"));
    let mut fallback = candidate("fallback");
    fallback.priority = 10;
    scheduler.upsert(fallback);
    assert!(scheduler.bind_response_affinity("response", "owner", 0));
    scheduler.set_cooldown("owner", "gpt-5", 10);

    // Retryable provider failures invalidate the owner binding before the
    // next selection. The tried set then excludes the cooled owner and lets
    // the scheduler use the next eligible candidate.
    assert!(scheduler.invalidate_response_affinity("response"));
    let tried = HashSet::from(["owner".to_string()]);
    let selection = scheduler
        .select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &CandidateScope::default(),
            tried: &tried,
            response_affinity_key: Some("response"),
            prompt_affinity_key: None,
            now_ms: 1,
        })
        .unwrap();
    assert_eq!(selection.candidate_id, "fallback");
    assert!(!selection.response_affinity_hit);
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
    assert_eq!(loaded[0].dispatches, 0);
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
        "second"
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
    assert!(!second.half_open);
    assert!(second.available);
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
fn old_dispatch_fence_cannot_release_a_reintroduced_candidate_fence() {
    let mut scheduler = PoolScheduler::new();
    scheduler.upsert(candidate("source"));
    let old_epoch = scheduler.begin_execution_fence("source").unwrap();
    scheduler.remove("source").unwrap();
    scheduler.upsert(candidate("source"));
    let new_epoch = scheduler.begin_execution_fence("source").unwrap();
    assert_ne!(old_epoch, new_epoch);

    scheduler.end_execution_fence("source", old_epoch);
    assert!(select(&mut scheduler, &HashSet::new()).is_none());
    scheduler.end_execution_fence("source", new_epoch);
    assert!(select(&mut scheduler, &HashSet::new()).is_some());
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
