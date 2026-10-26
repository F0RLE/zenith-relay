use crate::local_pool::DesktopState;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use zenith_relay_core::{model_metadata::ModelMetadataCatalog, pricing::PricingCatalog};

mod accounting;
use accounting::CatalogAccounting;

static MODEL_ACCOUNTING: CatalogAccounting<ModelMetadataCatalog> = CatalogAccounting::new();
static PRICE_ACCOUNTING: CatalogAccounting<PricingCatalog> = CatalogAccounting::new();

struct ProcessMemory {
    pid: u32,
    role: &'static str,
    resident_bytes: Option<u64>,
    private_bytes: Option<u64>,
}

struct MemoryDiagnostics {
    sampled_at_ms: u64,
    build_profile: &'static str,
    processes: Vec<ProcessMemory>,
    caches: Vec<zenith_relay_core::gateway::CacheMemoryDiagnostics>,
}

fn descendant_pids(
    root: u32,
    processes: impl IntoIterator<Item = (u32, Option<u32>, bool)>,
) -> BTreeSet<u32> {
    let mut children = BTreeMap::<_, Vec<_>>::new();
    for (pid, parent, webview) in processes {
        if let (true, Some(parent)) = (webview, parent) {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut selected = BTreeSet::from([root]);
    let mut pending = vec![root];
    while let Some(parent) = pending.pop() {
        if let Some(pids) = children.remove(&parent) {
            for pid in pids {
                if selected.insert(pid) {
                    pending.push(pid);
                }
            }
        }
    }
    selected
}

fn is_webview(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    [b"msedgewebview2".as_slice(), b"webkit".as_slice()]
        .iter()
        .any(|pattern| {
            name.windows(pattern.len())
                .any(|part| part.eq_ignore_ascii_case(pattern))
        })
}

fn webview_role(arguments: &[OsString]) -> &'static str {
    match arguments
        .iter()
        .filter_map(|argument| argument.to_str())
        .find_map(|argument| argument.strip_prefix("--type="))
    {
        Some("renderer") => "renderer",
        Some("gpu-process") => "gpu",
        Some("utility") => "utility",
        Some("crashpad-handler") => "crash_handler",
        Some(_) => "webview_other",
        None => "webview_browser",
    }
}

fn collect(state: &DesktopState) -> MemoryDiagnostics {
    let root = std::process::id();
    let mut system = System::new();
    // Discover ancestry first; do not read command lines or memory of unrelated apps.
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );
    let parents = system.processes().values().map(|process| {
        (
            process.pid().as_u32(),
            process.parent().map(|pid| pid.as_u32()),
            is_webview(process.name()),
        )
    });
    let descendants = descendant_pids(root, parents);
    let pids: Vec<_> = descendants.into_iter().map(Pid::from_u32).collect();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&pids),
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_memory()
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let processes = pids
        .into_iter()
        .filter_map(|pid| {
            let process = system.process(pid)?;
            let resident = process.memory();
            Some(ProcessMemory {
                pid: pid.as_u32(),
                role: if pid.as_u32() == root {
                    "native"
                } else {
                    webview_role(process.cmd())
                },
                resident_bytes: (resident > 0).then_some(resident),
                // sysinfo's Windows implementation reads PROCESS_MEMORY_COUNTERS_EX.PrivateUsage.
                private_bytes: cfg!(target_os = "windows")
                    .then(|| process.virtual_memory())
                    .filter(|bytes| *bytes > 0),
            })
        })
        .collect();
    let mut caches = zenith_relay_core::gateway::cache_memory_diagnostics();
    caches.extend(state.continuation_memory_diagnostics());
    let metadata = state.model_metadata_loader().source_memory_diagnostics();
    MODEL_ACCOUNTING.append(
        &state.model_metadata_loader().snapshot(),
        &mut caches,
        ModelMetadataCatalog::memory_diagnostics,
    );
    caches.push(zenith_relay_core::gateway::CacheMemoryDiagnostics {
        key: "model_metadata_sources",
        entries: metadata.map(|(entries, _)| entries),
        bytes: metadata.map(|(_, bytes)| bytes),
        limit_bytes: None,
        basis: "source_json",
    });
    let pricing = state.pricing_loader();
    let pricing_source = pricing.source_memory_diagnostics();
    PRICE_ACCOUNTING.append(
        &pricing.snapshot(),
        &mut caches,
        PricingCatalog::memory_diagnostics,
    );
    caches.push(zenith_relay_core::gateway::CacheMemoryDiagnostics {
        key: "pricing_sources",
        entries: pricing_source.map(|(entries, _)| entries),
        bytes: pricing_source.map(|(_, bytes)| bytes),
        limit_bytes: None,
        basis: "source_json",
    });
    caches.push(zenith_relay_core::gateway::CacheMemoryDiagnostics {
        key: "sqlite_pages",
        entries: None,
        bytes: None,
        limit_bytes: state.telemetry.memory_cache_budget(),
        basis: "budget_only",
    });
    MemoryDiagnostics {
        sampled_at_ms: zenith_relay_core::unix_time_ms(),
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        processes,
        caches,
    }
}

/// Called on a blocking worker at explicit diagnostic boundaries, never on a
/// timer or from runtime snapshots. The renderer receives no monitoring DTO.
pub(crate) fn record_snapshot(state: &DesktopState, reason: &'static str) {
    if !super::is_debug_enabled() {
        return;
    }
    let snapshot = collect(state);
    if !super::is_debug_enabled() {
        return;
    }
    write_snapshot(&snapshot, reason, |message, details| {
        // Memory accounting must not overwrite the last actionable breadcrumb.
        super::persist::record_event(
            "operations",
            "info",
            "memory",
            Some("memory"),
            None,
            message,
            details,
        );
    });
}

fn write_snapshot(
    snapshot: &MemoryDiagnostics,
    reason: &str,
    mut write: impl FnMut(&str, &[(&str, String)]),
) {
    let sample = snapshot.sampled_at_ms.to_string();
    write(
        "snapshot",
        &[
            ("sampled_at_ms", sample.clone()),
            ("reason", reason.to_string()),
            ("build_profile", snapshot.build_profile.to_string()),
            ("process_count", snapshot.processes.len().to_string()),
            ("cache_count", snapshot.caches.len().to_string()),
        ],
    );
    for process in &snapshot.processes {
        write(
            "process",
            &[
                ("sampled_at_ms", sample.clone()),
                ("pid", process.pid.to_string()),
                ("role", process.role.to_string()),
                ("resident_bytes", number(process.resident_bytes)),
                ("private_bytes", number(process.private_bytes)),
            ],
        );
    }
    for cache in &snapshot.caches {
        write(
            "cache",
            &[
                ("sampled_at_ms", sample.clone()),
                ("component", cache.key.to_string()),
                ("basis", cache.basis.to_string()),
                ("entries", number(cache.entries)),
                ("bytes", number(cache.bytes)),
                ("limit_bytes", number(cache.limit_bytes)),
            ],
        );
    }
}

fn number(value: Option<impl std::fmt::Display>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_log_keeps_numeric_measurements_and_unknowns_separate() {
        let snapshot = MemoryDiagnostics {
            sampled_at_ms: 123,
            build_profile: "debug",
            processes: vec![ProcessMemory {
                pid: 1,
                role: "native",
                resident_bytes: Some(0),
                private_bytes: None,
            }],
            caches: vec![zenith_relay_core::gateway::CacheMemoryDiagnostics {
                key: "sqlite_pages",
                entries: None,
                bytes: None,
                limit_bytes: Some(1024),
                basis: "budget_only",
            }],
        };
        let mut rows = Vec::new();
        write_snapshot(&snapshot, "logs_opened", |message, details| {
            rows.push((
                message.to_string(),
                details
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            ));
        });
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].1["reason"], "logs_opened");
        assert_eq!(rows[0].1["build_profile"], "debug");
        assert_eq!(rows[1].1["resident_bytes"], "0");
        assert_eq!(rows[1].1["private_bytes"], "unavailable");
        assert_eq!(rows[2].1["bytes"], "unavailable");
        assert_eq!(rows[2].1["limit_bytes"], "1024");
        assert_eq!(rows[2].1["basis"], "budget_only");
        assert!(
            rows.iter()
                .all(|(_, details)| details["sampled_at_ms"] == "123")
        );
    }

    #[test]
    fn ancestry_excludes_other_apps_and_tolerates_orphans_and_cycles() {
        let parents = [
            (4, Some(3), true),
            (3, Some(2), true),
            (2, Some(1), true),
            (8, None, true),
            (9, Some(8), true),
            (10, Some(99), true),
            (11, Some(12), true),
            (12, Some(11), true),
            // An unselected launched application is not a bridge into its WebViews.
            (19, Some(1), false),
            (20, Some(19), true),
        ];
        assert_eq!(descendant_pids(1, parents), BTreeSet::from([1, 2, 3, 4]));
    }

    #[test]
    fn process_names_keep_case_insensitive_matching_without_allocating_copies() {
        for name in [
            "msedgewebview2.exe",
            "MSEDGEWEBVIEW2.EXE",
            "WebKit.WebProcess",
        ] {
            assert!(is_webview(OsStr::new(name)));
        }
        for name in ["", "zenith-relay.exe", "other-app.exe", "界面.exe"] {
            assert!(!is_webview(OsStr::new(name)));
        }
    }

    #[test]
    fn deep_process_tree_does_not_need_repeated_full_table_passes() {
        let processes = (2..=10_000).rev().map(|pid| (pid, Some(pid - 1), true));
        let selected = descendant_pids(1, processes);
        assert_eq!(selected.len(), 10_000);
        assert_eq!(selected.last(), Some(&10_000));
    }

    #[test]
    fn command_lines_are_reduced_to_fixed_roles() {
        for (argument, expected) in [
            ("--type=renderer", "renderer"),
            ("--type=gpu-process", "gpu"),
            ("--type=utility", "utility"),
            ("--type=unrecognized", "webview_other"),
            ("synthetic-sensitive-argument", "webview_browser"),
        ] {
            assert_eq!(webview_role(&[OsString::from(argument)]), expected);
        }
    }
}
