use super::super::order::{model_leaf, normalize};
use std::collections::BTreeMap;

// None is an ambiguous match, distinct from a missing key. Do not fall back to
// a weaker match when several records claim the same version or leaf.
struct CandidateIndex<'a, T> {
    unsuffixed: BTreeMap<String, Option<&'a T>>,
    versions: BTreeMap<String, Option<&'a T>>,
    leaves: BTreeMap<&'a str, Option<&'a T>>,
}

impl<T> Default for CandidateIndex<'_, T> {
    fn default() -> Self {
        Self {
            unsuffixed: BTreeMap::new(),
            versions: BTreeMap::new(),
            leaves: BTreeMap::new(),
        }
    }
}

impl<'a, T> CandidateIndex<'a, T> {
    fn insert(&mut self, model_id: &'a str, version: &str, metadata_record: &'a T) {
        if !model_id.contains(':') {
            self.unsuffixed
                .entry(version.to_owned())
                .and_modify(|candidate| *candidate = None)
                .or_insert(Some(metadata_record));
        }
        self.versions
            .entry(version.to_owned())
            .and_modify(|candidate| *candidate = None)
            .or_insert(Some(metadata_record));
        self.leaves
            .entry(model_leaf(model_id))
            .and_modify(|candidate| *candidate = None)
            .or_insert(Some(metadata_record));
    }
}

/// Build matching keys once per catalog instead of scanning and normalizing
/// every registry row for each model. The index borrows the records and lives
/// only for the merge, so it cannot retain an older catalog after refresh.
pub(super) struct RecordIndex<'a, T> {
    exact: &'a BTreeMap<String, T>,
    all: CandidateIndex<'a, T>,
    providers: BTreeMap<Option<&'a str>, CandidateIndex<'a, T>>,
}

impl<'a, T> RecordIndex<'a, T> {
    pub(super) fn new(records: &'a BTreeMap<String, T>) -> Self {
        let mut index = Self {
            exact: records,
            all: CandidateIndex::default(),
            providers: BTreeMap::new(),
        };
        for (model_id, metadata_record) in records {
            let version = version_match_id(model_id);
            index.all.insert(model_id, &version, metadata_record);
            let provider = model_id.split_once('/').map(|(provider, _)| provider);
            index.providers.entry(provider).or_default().insert(
                model_id,
                &version,
                metadata_record,
            );
        }
        index
    }

    pub(super) fn get(&self, model_id: &str) -> Option<&'a T> {
        let key = normalize(model_id);
        if let Some(metadata_record) = self.exact.get(&key) {
            return Some(metadata_record);
        }
        let provider = key.split_once('/').map(|(provider, _)| provider);
        let index = provider.map_or(Some(&self.all), |provider| {
            self.providers.get(&Some(provider))
        });
        if let Some(index) = index {
            let version = version_match_id(&key);
            // A base row wins over :free/:batch variants of the same version.
            if let Some(metadata_record) = index.unsuffixed.get(&version) {
                return *metadata_record;
            }
            if let Some(metadata_record) = index.versions.get(&version) {
                return *metadata_record;
            }
        }
        let leaf = model_leaf(&key);
        let local = index.and_then(|index| index.leaves.get(leaf));
        let unqualified = provider
            .and_then(|_| self.providers.get(&None))
            .and_then(|index| index.leaves.get(leaf));
        match (local, unqualified) {
            (Some(metadata_record), None) | (None, Some(metadata_record)) => *metadata_record,
            _ => None,
        }
    }
}

// Normalize only decimal release separators; provider boundaries and other
// punctuation remain significant. Variant suffixes are compared separately.
fn version_match_id(model_id: &str) -> String {
    let base_model_id = model_id.split_once(':').map_or(model_id, |(base, _)| base);
    let bytes = base_model_id.as_bytes();
    base_model_id
        .char_indices()
        .map(|(index, character)| {
            if character == '.'
                && index > 0
                && index + 1 < bytes.len()
                && bytes[index - 1].is_ascii_digit()
                && bytes[index + 1].is_ascii_digit()
            {
                '-'
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
