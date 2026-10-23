use super::*;
use crate::automations::{WakeExecutionPolicy, WakeTrigger};
use crate::quota::QuotaWindowKind;

fn task(selector: AccountSelector) -> WakeTask {
    WakeTask {
        id: "synthetic-task".into(),
        name: "Synthetic wake".into(),
        enabled: true,
        account_selector: selector,
        window_kinds: [QuotaWindowKind::Primary].into(),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn remap(
    tasks: &mut [WakeTask],
    models: &[String],
    rules: &ModelRules,
    kind: OAuthClientKind,
) -> Result<(), &'static str> {
    let destination = match kind {
        OAuthClientKind::Codex => "codex",
        OAuthClientKind::Siwc => "siwc",
        OAuthClientKind::ExcelBps => "bps",
    };
    select_automation_route(
        tasks,
        &["codex".into(), "siwc".into(), "bps".into()],
        destination,
        models,
        rules,
        kind,
        42,
    )
}

#[test]
fn route_change_preserves_unrelated_selectors_and_uses_the_supplied_clock() {
    let mut tasks = vec![
        task(AccountSelector::AccountIds(
            [
                "codex".into(),
                "siwc".into(),
                "bps".into(),
                "unrelated".into(),
            ]
            .into(),
        )),
        task(AccountSelector::AllEligible),
        task(AccountSelector::Tags(["work".into()].into())),
        task(AccountSelector::AccountIds(["unrelated".into()].into())),
        task(AccountSelector::AccountIds(["bps".into()].into())),
    ];
    let untouched = tasks[1..].to_vec();
    remap(
        &mut tasks,
        &[],
        &ModelRules::default(),
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    assert_eq!(
        tasks[0].account_selector,
        AccountSelector::AccountIds(["bps".into(), "unrelated".into()].into())
    );
    assert_eq!(tasks[0].created_at_ms, 1);
    assert_eq!(tasks[0].updated_at_ms, 42);
    assert_eq!(&tasks[1..], untouched);
}

#[test]
fn explicit_models_must_be_supported_and_allowed_but_disabled_tasks_can_move() {
    let mut explicit = task(AccountSelector::AccountIds(["codex".into()].into()));
    explicit.model_policy = WakeModelPolicy::Explicit("MODEL-A".into());
    for (models, rules) in [
        (vec![], ModelRules::default()),
        (
            vec!["model-a".into()],
            ModelRules::from_allow_deny(&[], &["model-a".into()]),
        ),
        (
            vec!["model-a".into()],
            ModelRules::from_allow_deny(&["model-b".into()], &[]),
        ),
    ] {
        let mut tasks = vec![explicit.clone()];
        assert!(remap(&mut tasks, &models, &rules, OAuthClientKind::ExcelBps).is_err());
        assert_eq!(tasks, [explicit.clone()]);
    }
    let mut tasks = vec![explicit];
    remap(
        &mut tasks,
        &["model-a".into()],
        &ModelRules::default(),
        OAuthClientKind::Siwc,
    )
    .unwrap();
    assert_eq!(tasks[0].updated_at_ms, 42);
    tasks[0].enabled = false;
    tasks[0].account_selector = AccountSelector::AccountIds(["codex".into()].into());
    remap(
        &mut tasks,
        &[],
        &ModelRules::default(),
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    assert!(!tasks[0].enabled);
}

#[test]
fn codex_rejects_enabled_pool_automations_but_keeps_disabled_targets_editable() {
    let mut tasks = vec![task(AccountSelector::AccountIds(["siwc".into()].into()))];
    let original = tasks.clone();
    assert!(remap(
        &mut tasks,
        &[],
        &ModelRules::default(),
        OAuthClientKind::Codex
    )
    .is_err());
    assert_eq!(tasks, original);
    tasks[0].enabled = false;
    remap(
        &mut tasks,
        &[],
        &ModelRules::default(),
        OAuthClientKind::Codex,
    )
    .unwrap();
    assert_eq!(
        tasks[0].account_selector,
        AccountSelector::AccountIds(["codex".into()].into())
    );
}

#[test]
fn a_later_conflict_does_not_partially_retarget_the_batch() {
    let lightest = task(AccountSelector::AccountIds(["codex".into()].into()));
    let mut explicit = lightest.clone();
    explicit.id = "synthetic-explicit".into();
    explicit.model_policy = WakeModelPolicy::Explicit("missing".into());
    let mut tasks = vec![lightest, explicit];
    let original = tasks.clone();
    assert!(remap(
        &mut tasks,
        &[],
        &ModelRules::default(),
        OAuthClientKind::ExcelBps
    )
    .is_err());
    assert_eq!(tasks, original);
}
