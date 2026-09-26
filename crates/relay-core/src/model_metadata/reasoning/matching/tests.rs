use super::*;

fn assert_matches(records: &[&str], queries: &[(&str, Option<usize>)]) {
    let records = records
        .iter()
        .enumerate()
        .map(|(index, id)| ((*id).to_owned(), index))
        .collect();
    let index = RecordIndex::new(&records);
    for (query, expected) in queries {
        assert_eq!(index.get(query).copied(), *expected, "query {query}");
    }
}

#[test]
fn exact_base_and_variant_precedence_preserves_ambiguity() {
    assert_matches(
        &["vendor/model-2.1", "vendor/model-2.1:batch"],
        &[
            (" Vendor/Model-2.1 ", Some(0)),
            ("vendor/model-2-1", Some(0)),
            ("vendor/model-2.1:batch", Some(1)),
            ("vendor/model-2-1:free", Some(0)),
            ("model-2.1", Some(0)),
            ("other/model-2.1", None),
        ],
    );
    assert_matches(
        &["vendor/model-2.1:free"],
        &[("vendor/model-2-1", Some(0)), ("model-2.1", None)],
    );
    assert_matches(
        &["vendor/model-2.1:free", "vendor/model-2.1:batch"],
        &[("vendor/model-2-1", None)],
    );
    assert_matches(
        &["vendor/model-2.1", "vendor/model-2-1"],
        &[
            ("vendor/model-2.1", Some(0)),
            ("vendor/model-2-1", Some(1)),
            ("vendor/model-2.1:free", None),
        ],
    );
}

#[test]
fn qualified_leaf_fallback_stays_inside_its_provider() {
    assert_matches(
        &["vendor/family/model", "other/model"],
        &[
            ("vendor/model", Some(0)),
            ("other/model", Some(1)),
            ("unrelated/model", None),
            ("model", None),
        ],
    );
    assert_matches(
        &["model", "other/model"],
        &[("vendor/model", Some(0)), ("model", Some(0))],
    );
    assert_matches(
        &["model", "vendor/family/model"],
        &[("vendor/model", None), ("other/model", Some(0))],
    );
    assert_matches(
        &["vendor/family/model", "vendor/other/model"],
        &[("vendor/model", None), ("model", None)],
    );
}

#[test]
fn punctuation_and_provider_names_do_not_become_version_aliases() {
    assert_matches(
        &["vendor-2-1/model-3.1", "vendor/жЁЎећ‹-3.1"],
        &[
            ("vendor-2.1/model-3-1", None),
            ("vendor-2-1/model-3-1", Some(0)),
            ("vendor/жЁЎећ‹-3-1", Some(1)),
            ("vendor/жЁЎећ‹-3_1", None),
        ],
    );
    assert_matches(
        &["vendor:first/model-1", "vendor:second/model-1", "/model"],
        &[
            ("vendor:first/other", Some(0)),
            ("vendor:second/other", Some(1)),
            ("vendor:third/other", None),
            ("/nested/model", Some(2)),
        ],
    );
}

#[test]
fn large_disjoint_catalog_keeps_qualified_and_missing_models_separate() {
    let records: BTreeMap<_, _> = (0..10_000)
        .map(|id| (format!("vendor/model-{id}.1"), id))
        .collect();
    let index = RecordIndex::new(&records);
    for id in 0..10_000 {
        assert_eq!(index.get(&format!("vendor/model-{id}-1")), Some(&id));
        assert_eq!(index.get(&format!("other/model-{id}.1")), None);
        assert_eq!(index.get(&format!("missing-{id}")), None);
    }
}
