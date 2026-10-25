use super::{order::ModelOrderKey, ModelDisplayIdentity, ModelMetadataCatalog};
use std::collections::BTreeMap;

struct DisplayEntry {
    order: ModelOrderKey,
    provider: String,
}

/// Presentation work shared only inside one immutable catalog projection.
/// This never retains routing, entitlement, quota, or a mutable catalog handle.
pub(crate) struct DisplayProjection<'a> {
    catalog: &'a ModelMetadataCatalog,
    entries: BTreeMap<String, DisplayEntry>,
}

impl<'a> DisplayProjection<'a> {
    pub(crate) fn new(catalog: &'a ModelMetadataCatalog) -> Self {
        Self {
            catalog,
            entries: BTreeMap::new(),
        }
    }

    fn entry(&mut self, model: &str) -> &DisplayEntry {
        if !self.entries.contains_key(model) {
            self.entries.insert(
                model.to_string(),
                DisplayEntry {
                    order: self.catalog.display_order_key(model),
                    provider: self
                        .catalog
                        .display_provider_for(model)
                        .unwrap_or_else(|| "other".into()),
                },
            );
        }
        &self.entries[model]
    }

    pub(crate) fn identity(&mut self, model: &str) -> ModelDisplayIdentity {
        self.entry(model).order.identity.clone()
    }

    pub(crate) fn merge<I, S>(&mut self, models: I, saved_order: &[String]) -> Vec<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let models = crate::normalize_model_ids(models);
        for model in &models {
            self.entry(model);
        }
        let mut keyed = models
            .into_iter()
            .map(|model| {
                let entry = &self.entries[&model];
                let normalized = crate::model_id_key(&model);
                (model, entry, normalized)
            })
            .collect::<Vec<_>>();
        keyed.sort_by(|left, right| {
            left.1
                .order
                .compare(&right.1.order)
                .then_with(|| left.2.cmp(&right.2))
        });
        let ordered = keyed
            .into_iter()
            .map(|(model, _, _)| model)
            .collect::<Vec<_>>();
        if saved_order.is_empty() {
            return ordered;
        }
        let groups = ordered
            .iter()
            .map(|model| {
                (
                    crate::model_id_key(model),
                    self.entries[model].provider.clone(),
                )
            })
            .collect();
        // Merge anchors separately for each member. A shared global rank is
        // not equivalent when different inventories omit saved anchors.
        crate::catalog::merge_model_display_order_with_groups(ordered, saved_order, &groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_anchors(
        catalog: &ModelMetadataCatalog,
        display: &mut DisplayProjection<'_>,
        ids: &[&str],
        anchors: &mut Vec<String>,
    ) {
        for inventory in [
            ids.to_vec(),
            vec![ids[1], ids[3]],
            vec![ids[4], ids[0], ids[2], ids[0], " "],
        ] {
            assert_eq!(
                display.merge(&inventory, anchors),
                catalog.merge_display_order(&inventory, anchors)
            );
            for id in inventory.into_iter().filter(|id| !id.trim().is_empty()) {
                assert_eq!(display.identity(id), catalog.display_identity_for(id));
            }
        }
        for id in ids {
            if !anchors.iter().any(|anchor| anchor.as_str() == *id) {
                anchors.push((*id).into());
                check_anchors(catalog, display, ids, anchors);
                anchors.pop();
            }
        }
    }

    #[test]
    fn memoized_display_matches_uncached_order_for_every_saved_anchor_sequence() {
        let catalog = ModelMetadataCatalog::empty();
        let ids = [
            "gpt-5.4",
            "claude-sonnet-4.5",
            "gemini-2.5-pro",
            "unknown-a",
            "unknown-b",
        ];
        let mut display = DisplayProjection::new(&catalog);
        check_anchors(&catalog, &mut display, &ids, &mut Vec::new());
        assert_eq!(display.entries.len(), ids.len());
        let case_varied = [" GPT-5.4 ", "gpt-5.4", "UNKNOWN-A", "missing"];
        let saved = ["stale".into(), "unknown-a".into(), "GPT-5.4".into()];
        assert_eq!(
            display.merge(case_varied, &saved),
            catalog.merge_display_order(case_varied, &saved)
        );
    }

    #[test]
    fn display_memoization_is_scoped_to_one_immutable_catalog() {
        let first =
            ModelMetadataCatalog::from_models_dev_json(r#"{"vendor/model":{"family":"alpha"}}"#)
                .unwrap();
        let second =
            ModelMetadataCatalog::from_models_dev_json(r#"{"vendor/model":{"family":"beta"}}"#)
                .unwrap();
        let mut old = DisplayProjection::new(&first);
        let mut current = DisplayProjection::new(&second);
        assert_eq!(
            old.identity("vendor/model"),
            first.display_identity_for("vendor/model")
        );
        assert_eq!(
            current.identity("vendor/model"),
            second.display_identity_for("vendor/model")
        );
        assert_ne!(
            old.identity("vendor/model"),
            current.identity("vendor/model")
        );
    }
}
