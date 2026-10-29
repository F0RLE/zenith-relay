use super::{CatalogEntry, ImageModelPrice};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

/// Immutable catalog indexes retaining canonical entry keys and image quotes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CatalogLookup {
    canonical: BTreeMap<String, BTreeMap<String, Option<String>>>,
    images: BTreeMap<String, ImageModelPrice>,
}

impl CatalogLookup {
    pub(super) fn build(
        entries: &BTreeMap<String, CatalogEntry>,
        conflicts: &BTreeSet<String>,
    ) -> Self {
        let mut lookup = Self::default();
        // Iterate in the original lookup's lexical order. Image aliases retain
        // its first eligible match; canonical aliases must all agree.
        for (entry_key, entry) in entries {
            let normalized = super::normalize(&entry.model_id);
            if conflicts.contains(&normalized) {
                continue;
            }
            let leaf = super::unqualified(&entry.model_id);
            if let Some(provider) = &entry.provider {
                let family = lookup.canonical.entry(provider.clone()).or_default();
                match family.entry(leaf.clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert(Some(entry_key.clone()));
                    }
                    Entry::Occupied(mut slot) => {
                        if slot
                            .get()
                            .as_ref()
                            .is_some_and(|first| !entries[first].equivalent_pricing(entry))
                        {
                            slot.insert(None);
                        }
                    }
                }
            }
            if let Some(image) = entry.image {
                lookup.images.entry(normalized).or_insert(image);
                lookup.images.entry(leaf).or_insert(image);
            }
        }
        lookup
    }

    pub(super) fn canonical_entry_key(&self, model: &str, family: &str) -> Option<&str> {
        self.canonical
            .get(&super::normalize(family))?
            .get(&super::unqualified(model))?
            .as_deref()
    }

    pub(super) fn image(&self, model: &str) -> Option<ImageModelPrice> {
        self.images.get(&super::normalize(model)).copied()
    }

    pub(super) fn memory_usage(&self) -> (usize, usize) {
        use std::mem::size_of;
        let mut entries = self.images.len();
        let mut bytes = self.images.len() * size_of::<(String, ImageModelPrice)>();
        bytes += self.images.keys().map(String::capacity).sum::<usize>();
        for (provider, aliases) in &self.canonical {
            entries += aliases.len();
            bytes += size_of::<(String, BTreeMap<String, Option<String>>)>() + provider.capacity();
            for (alias, key) in aliases {
                bytes += size_of::<(String, Option<String>)>()
                    + alias.capacity()
                    + key.as_ref().map_or(0, String::capacity);
            }
        }
        (entries, bytes)
    }
}
