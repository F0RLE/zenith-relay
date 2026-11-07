use super::super::client_socket::{MAX_QUEUED_BYTES, MAX_QUEUED_REQUESTS};
use super::super::{GatewayFailure, Message};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, VecDeque};

pub(super) type Lane = Option<String>;

pub(super) struct Envelope {
    pub(super) lane: Lane,
    pub(super) create: bool,
}

impl Envelope {
    pub(super) fn parse(message: &Message) -> Result<Self, GatewayFailure> {
        let fields: BTreeMap<String, &RawValue> = serde_json::from_slice(payload(message))
            .map_err(|_| GatewayFailure::invalid_request("request must be a valid JSON object"))?;
        let lane = fields
            .get("stream_id")
            .map(|raw| {
                let lane: String = serde_json::from_str(raw.get())
                    .map_err(|_| GatewayFailure::invalid_stream_id())?;
                if super::super::request::valid_stream_id(&lane) {
                    Ok(lane)
                } else {
                    Err(GatewayFailure::invalid_stream_id())
                }
            })
            .transpose()?;
        let kind = fields
            .get("type")
            .map(|raw| serde_json::from_str::<String>(raw.get()))
            .transpose()
            .map_err(|_| GatewayFailure::invalid_request("invalid request type"))?;
        Ok(Self {
            lane,
            create: kind.as_deref().is_none_or(|kind| kind == "response.create"),
        })
    }
}

pub(super) fn payload(message: &Message) -> &[u8] {
    match message {
        Message::Text(text) => text.as_bytes(),
        Message::Binary(bytes) => bytes.as_ref(),
        _ => &[],
    }
}

pub(super) struct Queued {
    pub(super) lane: Lane,
    pub(super) message: Message,
}

#[derive(Default)]
pub(super) struct Pending {
    entries: VecDeque<Queued>,
    bytes: usize,
}

impl Pending {
    pub(super) fn push(&mut self, lane: Lane, message: Message) -> Result<(), GatewayFailure> {
        let size = payload(&message).len();
        if self.entries.len() >= MAX_QUEUED_REQUESTS
            || size > MAX_QUEUED_BYTES.saturating_sub(self.bytes)
        {
            return Err(GatewayFailure::invalid_request(
                "WebSocket pending request queue is full",
            ));
        }
        self.bytes += size;
        self.entries.push_back(Queued { lane, message });
        Ok(())
    }

    pub(super) fn take_ready(&mut self, ready: impl Fn(&Lane) -> bool) -> Option<Queued> {
        let index = self.entries.iter().position(|entry| ready(&entry.lane))?;
        let entry = self.entries.remove(index)?;
        self.bytes -= payload(&entry.message).len();
        Some(entry)
    }

    pub(super) fn restore_front(&mut self, entry: Queued) {
        self.bytes += payload(&entry.message).len();
        self.entries.push_front(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplex_queue_skips_busy_lanes_without_reordering_each_lane() {
        let mut queue = Pending::default();
        for lane in ["a", "a", "b", "b"] {
            assert!(
                queue
                    .push(Some(lane.into()), Message::Text(lane.into()))
                    .is_ok()
            );
        }
        assert_eq!(
            queue
                .take_ready(|lane| lane.as_deref() == Some("b"))
                .unwrap()
                .lane
                .as_deref(),
            Some("b")
        );
        assert_eq!(
            queue.take_ready(|_| true).unwrap().lane.as_deref(),
            Some("a")
        );
        assert_eq!(
            queue.take_ready(|_| true).unwrap().lane.as_deref(),
            Some("a")
        );
        assert_eq!(
            queue.take_ready(|_| true).unwrap().lane.as_deref(),
            Some("b")
        );
        assert_eq!(queue.bytes, 0);
    }

    #[test]
    fn multiplex_envelopes_validate_lanes_without_reencoding_the_payload() {
        for stream in ["null", "3", "\"\"", "\"bad lane\""] {
            assert!(
                Envelope::parse(&Message::Text(
                    format!(r#"{{"stream_id":{stream}}}"#).into()
                ))
                .is_err()
            );
        }
        let message = Message::Text(r#"{ "type":"response.create","stream_id":"ma\u0069n","extension":123456789012345678901234567890 }"#.into());
        let envelope = Envelope::parse(&message).ok().unwrap();
        assert_eq!(envelope.lane.as_deref(), Some("main"));
        assert!(envelope.create);
    }

    #[test]
    fn multiplex_queue_bounds_count_and_bytes_and_restores_retired_work() {
        let mut queue = Pending::default();
        for index in 0..MAX_QUEUED_REQUESTS {
            assert!(
                queue
                    .push(None, Message::Text(index.to_string().into()))
                    .is_ok()
            );
        }
        let bytes = queue.bytes;
        assert!(
            queue
                .push(Some("other".into()), Message::Text("overflow".into()))
                .is_err()
        );
        assert_eq!(queue.bytes, bytes);
        let first = queue.take_ready(|_| true).unwrap();
        queue.restore_front(first);
        assert_eq!(queue.bytes, bytes);
        assert_eq!(payload(&queue.take_ready(|_| true).unwrap().message), b"0");

        let mut queue = Pending::default();
        let chunk = Message::Binary(vec![0; MAX_QUEUED_BYTES / 4].into());
        for _ in 0..4 {
            assert!(queue.push(None, chunk.clone()).is_ok());
        }
        assert_eq!(queue.bytes, MAX_QUEUED_BYTES);
        assert!(queue.push(None, Message::Text("x".into())).is_err());
        let first = queue.take_ready(|_| true).unwrap();
        queue.restore_front(first);
        assert_eq!(queue.bytes, MAX_QUEUED_BYTES);
        assert!(queue.take_ready(|_| true).is_some());
        assert!(queue.push(None, chunk).is_ok());
    }
}
