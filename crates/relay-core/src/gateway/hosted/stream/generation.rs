use ring::digest::{Context, SHA256, digest};
use serde_json::Value;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostedGeneration {
    pub duration_ms: i32,
    pub output_tokens: i64,
}

#[derive(Default)]
pub(super) struct Generation {
    deltas: Option<Deltas>,
    ambiguous: bool,
    paused_chunk: Option<Instant>,
}

struct Deltas {
    first: Instant,
    last: Instant,
    index: usize,
    item: String,
    text: Context,
}

impl Generation {
    pub(super) fn mark_read_pause(&mut self, at: Instant) {
        // Keep the first pause inside the measured span, not a later pause
        // that might share the timestamp of the last delta.
        if self
            .deltas
            .as_ref()
            .zip(self.paused_chunk)
            .is_some_and(|(d, p)| p >= d.first)
        {
            return;
        }
        self.paused_chunk = Some(at);
    }

    pub(super) fn delta(&mut self, event: &Value, text: &str, at: Instant) {
        if self.ambiguous || text.is_empty() {
            return;
        }
        let binding = event["item_id"]
            .as_str()
            .filter(|id| crate::is_ascii_token(id, 256))
            .zip(
                event["output_index"]
                    .as_u64()
                    .and_then(|v| usize::try_from(v).ok()),
            );
        let Some((item, index)) = binding.filter(|_| event["content_index"].as_u64() == Some(0))
        else {
            self.ambiguous = true;
            return;
        };
        let d = self.deltas.get_or_insert_with(|| Deltas {
            first: at,
            last: at,
            index,
            item: item.into(),
            text: Context::new(&SHA256),
        });
        if (d.item.as_str(), d.index) != (item, index)
            || self.paused_chunk.is_some_and(|p| d.first <= p && p < at)
        {
            self.ambiguous = true;
            return;
        }
        d.last = at;
        d.text.update(text.as_bytes());
    }

    pub(super) fn complete(
        &mut self,
        response: &Value,
        output: Option<i64>,
    ) -> Option<HostedGeneration> {
        let d = self.deltas.take().filter(|_| !self.ambiguous)?;
        let reasoning = response["usage"]["output_tokens_details"]["reasoning_tokens"]
            .as_i64()
            .filter(|n| *n >= 0)?;
        let duration_ms =
            i32::try_from(d.last.checked_duration_since(d.first)?.as_millis()).ok()?;
        // Use Relay's visible-output estimate after the first token. Reject buffered bursts.
        let output_tokens = output?.checked_sub(reasoning)?.checked_sub(1)?;
        if !(1..=30000).contains(&duration_ms)
            || !(1..=i64::from(duration_ms)).contains(&output_tokens)
        {
            return None;
        }
        let items = response["output"].as_array()?;
        let message = items.get(d.index)?;
        if message["id"].as_str() != Some(&d.item)
            || message["type"] != "message"
            || message["role"] != "assistant"
            || message["status"] != "completed"
            || !items
                .iter()
                .enumerate()
                .all(|(i, v)| i == d.index || v["type"] == "reasoning")
        {
            return None;
        }
        let [part] = message["content"].as_array()?.as_slice() else {
            return None;
        };
        let final_text = part["text"].as_str()?;
        // Do not divide all output tokens by the timing of a partial or replayed body.
        if part["type"] != "output_text"
            || digest(&SHA256, final_text.as_bytes()).as_ref() != d.text.finish().as_ref()
        {
            return None;
        }
        Some(HostedGeneration {
            duration_ms,
            output_tokens,
        })
    }
}
