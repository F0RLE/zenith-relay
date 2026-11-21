use super::{CompletedItem, Fingerprint, ItemFingerprint, ReplayItem, fingerprint};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

const MAX_DECLARATIONS: usize = 64;
const MAX_VISIBLE_ITEMS: usize = 128;

/// Local metadata from native catalog preparation, never parsed from client JSON.
pub(crate) struct CatalogReplay {
    pub(super) contract: Fingerprint,
    generated: Vec<usize>,
    lookup: Option<Lookup>,
}

struct Lookup {
    base_len: usize,
    declarations: BTreeMap<Fingerprint, Fingerprint>,
    supported: bool,
    receipt: Receipt,
}

pub(super) type Receipt = Arc<OnceLock<PublicHistory>>;

pub(super) struct PublicHistory {
    response_id: String,
    base_len: usize,
    visible: Vec<CompletedItem>,
    declarations: Vec<Fingerprint>,
}

impl CatalogReplay {
    pub(crate) fn new(tools: &Value) -> Self {
        Self {
            contract: fingerprint(tools),
            generated: Vec::new(),
            lookup: None,
        }
    }

    pub(crate) fn generated_declaration(&mut self, index: usize) {
        self.generated.push(index);
    }

    pub(crate) fn record_lookup(&mut self, input_len: usize, tools: &[Value], supported: bool) {
        // Only the last, publicly completed generation may certify its socket.
        if let Some(lookup) = &mut self.lookup {
            lookup.receipt = Receipt::default();
        }
        let lookup = self.lookup.get_or_insert_with(|| Lookup {
            base_len: input_len,
            declarations: BTreeMap::new(),
            supported: true,
            receipt: Receipt::default(),
        });
        lookup.supported &= supported;
        for tool in tools {
            if !lookup.supported {
                break;
            }
            // Distinct top-level declarations may be split across calls. Do not
            // reorder successive definitions of the same name or split namespaces.
            lookup.supported = lookup.declarations.len() < MAX_DECLARATIONS
                && tool["name"].is_string()
                && lookup
                    .declarations
                    .insert(fingerprint(&tool["name"]), fingerprint(tool))
                    .is_none();
        }
    }

    pub(crate) fn confirm(&self, response: &Value, visible: &[Value]) {
        let Some(lookup) = self.lookup.as_ref().filter(|l| l.supported) else {
            return;
        };
        let Some(id) = response["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 256)
        else {
            return;
        };
        if visible.len() <= MAX_VISIBLE_ITEMS {
            let _ = lookup.receipt.set(PublicHistory {
                response_id: id.to_owned(),
                base_len: lookup.base_len,
                visible: visible
                    .iter()
                    .map(|i| ReplayItem::new(i).completed())
                    .collect(),
                declarations: lookup.declarations.values().copied().collect(),
            });
        }
    }

    pub(super) fn receipt(&self) -> Option<Receipt> {
        Some(Arc::clone(&self.lookup.as_ref()?.receipt))
    }

    pub(super) fn cursor<'a>(
        &'a self,
        previous: &super::Checkpoint,
        input: &[ItemFingerprint],
        raw: &[Value],
    ) -> Option<PublicCursor<'a>> {
        if self.lookup.is_some() {
            return None;
        }
        let history = previous.request.receipt.as_ref()?.get()?;
        if history.response_id != previous.response_id
            || !input.starts_with(previous.request.input.get(..history.base_len)?)
        {
            return None;
        }
        let mut index = history.base_len;
        for expected in &history.visible {
            let replay = input.get(index)?;
            let item = &raw[index];
            if replay.content != expected.proof.content
                || expected.match_metadata(replay, item).is_none()
            {
                return None;
            }
            index += 1;
        }
        Some(PublicCursor {
            index,
            generated: &self.generated,
            remaining: history.declarations.clone(),
        })
    }
}

pub(super) struct PublicCursor<'a> {
    pub(super) index: usize,
    generated: &'a [usize],
    remaining: Vec<Fingerprint>,
}

impl PublicCursor<'_> {
    pub(super) fn before_output(&mut self, index: usize, raw: &[Value]) -> Option<usize> {
        if self.generated.binary_search(&index).is_err() {
            return Some(index);
        }
        // Only the declaration the adapter itself inserted before this call.
        let [tool] = raw.get(index)?["tools"].as_array()?.as_slice() else {
            return None;
        };
        let proof = fingerprint(tool);
        let pos = self.remaining.iter().position(|entry| *entry == proof)?;
        self.remaining.swap_remove(pos);
        Some(index + 1)
    }

    pub(super) fn complete(&self) -> bool {
        self.remaining.is_empty()
    }
}
