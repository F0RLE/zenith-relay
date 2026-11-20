use super::*;
use axum::body::{Body, Bytes};
use std::io;
use tokio::{sync::mpsc, time::timeout};
use tokio_stream::wrappers::ReceiverStream;

const RESPONSE_ID: &str = "resp_fixture";
const DELTA: &str = "OK ✓";
pub(super) const PRIVATE_ERROR: &str = "fixture-private-upstream-error";

pub(super) struct StreamReply {
    mime: &'static str,
    rx: mpsc::Receiver<Result<Bytes, io::Error>>,
}

impl StreamReply {
    pub(super) fn respond(self) -> Response {
        Response::builder()
            .header("content-type", self.mime)
            .body(Body::from_stream(ReceiverStream::new(self.rx)))
            .unwrap()
    }
}

pub(super) struct StreamWriter(mpsc::Sender<Result<Bytes, io::Error>>);

impl StreamWriter {
    pub(super) fn arm(source: &Source, mime: &'static str) -> Self {
        let (tx, rx) = mpsc::channel(8);
        assert!(
            source
                .provider
                .stream
                .lock()
                .unwrap()
                .replace(StreamReply { mime, rx })
                .is_none()
        );
        Self(tx)
    }

    pub(super) fn write(&self, frame: impl Into<Bytes>) {
        self.0
            .try_send(Ok(frame.into()))
            .expect("fixture can send frame");
    }

    pub(super) fn interrupt_read(&self) {
        self.0
            .try_send(Err(io::Error::other(PRIVATE_ERROR)))
            .expect("fixture can interrupt the body");
    }

    pub(super) async fn cancelled(&self) {
        timeout(Duration::from_secs(3), self.0.closed())
            .await
            .expect("upstream body was not dropped");
    }
}

fn event(kind: &str, number: u64, fields: Value) -> Bytes {
    let mut value = fields;
    value["type"] = json!(kind);
    value["sequence_number"] = json!(number);
    Bytes::from(format!("event: {kind}\r\ndata: {value}\r\n\r\n"))
}

pub(super) fn response_event(status: &str, sequence: u64, usage: Value) -> Bytes {
    let kind = if sequence == 0 { "created" } else { status };
    let output = if matches!(status, "completed" | "incomplete") {
        json!([{"id":"msg_fixture","type":"message","role":"assistant","status":status,
            "content":[{"type":"output_text","text":DELTA,"annotations":[]}]}])
    } else {
        json!([])
    };
    let error =
        (status == "failed").then(|| json!({"code":"server_error","message":PRIVATE_ERROR}));
    let incomplete = (status == "incomplete").then(|| json!({"reason":"max_output_tokens"}));
    event(
        &format!("response.{kind}"),
        sequence,
        json!({"response":{"object":"response","id":RESPONSE_ID,"model":MODEL,"status":status,
            "output":output,"usage":usage,"error":error,"incomplete_details":incomplete}}),
    )
}

pub(super) fn delta(sequence: u64) -> Bytes {
    event(
        "response.output_text.delta",
        sequence,
        json!({"item_id":"msg_fixture","output_index":0,"content_index":0,"delta":DELTA,"logprobs":[]}),
    )
}

pub(super) fn usage(input: i64, cached: i64, output: i64) -> Value {
    json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":cached},
        "output_tokens":output,"total_tokens":input + output})
}

pub(super) async fn qualifier(f: &Fixture) -> PgPool {
    db::connect(&fixture_config(), &f._cluster.dsn("route_qualifier"))
        .await
        .unwrap()
}

pub(super) async fn check(
    f: &Fixture,
    q: &PgPool,
    op: Uuid,
    item: Uuid,
) -> Result<(), &'static str> {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(
        file.path(),
        json!({"id":op,"scope":f.scope,"stream_inventory":item}).to_string(),
    )
    .unwrap();
    let scope = zenith_relay_hosted::qualification::scope(q, ORIGIN).await?;
    zenith_relay_hosted::qualification::check(
        scope,
        &f.vault,
        file.path(),
        &OutboundPolicy::default(),
    )
    .await
}

pub(super) async fn qualify(f: &Fixture, q: &PgPool, source: &Source) -> Uuid {
    let wire = StreamWriter::arm(source, "text/event-stream; charset=utf-8");
    wire.write(response_event("in_progress", 0, Value::Null));
    wire.write(delta(1));
    wire.write(response_event("completed", 2, usage(9, 2, 2)));
    let op = Uuid::new_v4();
    check(f, q, op, source.item).await.unwrap();
    op
}

pub(super) fn submit(f: &Fixture, operation: &str) -> reqwest::RequestBuilder {
    f.http
        .post(format!("{}/v1/responses", f.server.url))
        .bearer_auth(&f.bearer)
        .header("idempotency-key", operation)
        .json(
            &json!({"model":MODEL,"input":"x".repeat(1000),"max_output_tokens":100,"stream":true}),
        )
}

pub(super) async fn read_prefix(reply: &mut reqwest::Response, expected: &[u8]) {
    let read = async {
        let mut seen = Vec::new();
        while seen.len() < expected.len() {
            seen.extend_from_slice(&reply.chunk().await.unwrap().expect("body ended early"));
        }
        assert_eq!(seen, expected);
    };
    timeout(Duration::from_secs(3), read)
        .await
        .expect("expected bytes before the terminal");
}

pub(super) async fn duplicate(f: &Fixture, op: &str, request: Uuid, code: &str) {
    let r = submit(f, op).send().await.unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(request_id(&r), request);
    assert_eq!(r.headers()["x-should-retry"], "false");
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], code);
}
