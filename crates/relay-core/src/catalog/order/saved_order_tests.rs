use super::*;
use std::{hint::black_box, time::Instant};

#[test]
fn saved_order_preserves_spelling_groups_and_unknown_anchor_behavior() {
    let inventory = [
        " Alpha-1 ",
        "Beta-1",
        "alpha-2",
        "Beta-2",
        "unknown",
        "ALPHA-1",
        "",
    ];
    let groups = [
        ("alpha-1", "alpha"),
        ("alpha-2", "alpha"),
        ("beta-1", "beta"),
        ("beta-2", "beta"),
    ]
    .into_iter()
    .map(|(id, group)| (id.to_string(), group.to_string()))
    .collect();
    let saved = [" BETA-1 ", "absent", "ALPHA-2", "beta-1", ""].map(String::from);
    assert_eq!(
        merge_model_display_order_with_groups(inventory, &saved, &groups),
        ["Beta-1", "Beta-2", "Alpha-1", "alpha-2", "unknown"]
    );
    assert_eq!(
        merge_model_display_order_with_groups(inventory, &["absent".into()], &groups),
        ["Alpha-1", "Beta-1", "alpha-2", "Beta-2", "unknown"]
    );
}

#[test]
fn saved_order_without_groups_retains_discovery_insertion_and_all_anchors() {
    assert_eq!(
        merge_model_display_order(["a", "b", "c", "d", "e"], &["d".into(), "b".into()]),
        ["a", "c", "d", "b", "e"]
    );
    assert_eq!(
        merge_model_display_order(Vec::<String>::new(), &["missing".into()]),
        Vec::<String>::new()
    );
    assert_eq!(
        merge_model_display_order(["a", "b", "c"], &["c".into(), "b".into(), "a".into()]),
        ["c", "b", "a"]
    );
}

// A small string-based oracle keeps the pre-optimization insertion semantics
// independently readable; production must not repeat this work per comparison.
fn reference_order(
    inventory: &[String],
    saved: &[String],
    groups: &BTreeMap<String, String>,
) -> Vec<String> {
    let catalog = normalize_model_ids(inventory);
    let rank = |id: &str| {
        catalog
            .iter()
            .position(|model| model_id_key(model) == model_id_key(id))
            .unwrap()
    };
    let mut ordered = normalize_model_ids(saved)
        .into_iter()
        .filter_map(|saved| {
            catalog
                .iter()
                .find(|model| model_id_key(model) == model_id_key(&saved))
                .cloned()
        })
        .collect::<Vec<_>>();
    if ordered.is_empty() {
        return catalog;
    }
    for model in &catalog {
        if ordered.contains(model) {
            continue;
        }
        let peers = groups
            .get(&model_id_key(model))
            .map(|group| {
                ordered
                    .iter()
                    .enumerate()
                    .filter(|(_, existing)| groups.get(&model_id_key(existing)) == Some(group))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let next_peer = peers
            .iter()
            .find(|(_, existing)| rank(existing) > rank(model))
            .map(|(position, _)| *position);
        let position = next_peer
            .or_else(|| peers.last().map(|(position, _)| position + 1))
            .or_else(|| {
                ordered
                    .iter()
                    .position(|existing| rank(existing) > rank(model))
            })
            .unwrap_or(ordered.len());
        ordered.insert(position, model.clone());
    }
    ordered
}

#[test]
fn indexed_saved_order_matches_reference_for_all_small_anchor_sequences() {
    let inventory = ["Alpha-1", "Beta-1", "alpha-2", "unknown", "Beta-2"].map(String::from);
    let groups = [
        ("alpha-1", "alpha"),
        ("alpha-2", "alpha"),
        ("beta-1", "beta"),
        ("beta-2", "beta"),
    ]
    .into_iter()
    .map(|(id, group)| (id.to_string(), group.to_string()))
    .collect();
    let mut pending = vec![Vec::<String>::new()];
    while let Some(anchors) = pending.pop() {
        let mut saved = anchors
            .iter()
            .map(|id| format!(" {} ", id.to_ascii_uppercase()))
            .collect::<Vec<_>>();
        saved.extend(["absent".into(), String::new()]);
        saved.extend(anchors.first().cloned());
        for groups in [&groups, &BTreeMap::new()] {
            assert_eq!(
                merge_model_display_order_with_groups(&inventory, &saved, groups),
                reference_order(&inventory, &saved, groups)
            );
        }
        for id in &inventory {
            if !anchors.contains(id) {
                let mut next = anchors.clone();
                next.push(id.clone());
                pending.push(next);
            }
        }
    }
}

#[test]
#[ignore = "synthetic display-order timing; no providers or real inventory"]
fn measure_saved_display_order() {
    for size in [600, 2_000] {
        let inventory = (0..size)
            .map(|index| format!("synthetic-{index:04}"))
            .collect::<Vec<_>>();
        let groups = inventory
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), format!("provider-{}", index % 6)))
            .collect();
        let saved = inventory
            .iter()
            .step_by(30)
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let expected = merge_model_display_order_with_groups(&inventory, &saved, &groups);
        assert_eq!(expected.len(), size);
        let mut timings = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            let result = black_box(merge_model_display_order_with_groups(
                &inventory, &saved, &groups,
            ));
            timings.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(result, expected);
        }
        timings.sort_by(f64::total_cmp);
        println!(
            "saved_display_order models={size} samples=20 median_ms={:.3} p95_ms={:.3}",
            timings[10], timings[19]
        );
    }
}
