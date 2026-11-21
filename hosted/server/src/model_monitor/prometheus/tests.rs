use super::*;
use axum::{
    Router,
    body::Body,
    response::IntoResponse,
    routing::{any, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::SeqCst},
};
use tokio::net::TcpListener;

const START: i64 = 1_800_000_000;
const ID: Uuid = Uuid::from_u128(1);
const OTHER: Uuid = Uuid::from_u128(2);

fn series(id: Uuid, values: [&str; 6]) -> Value {
    let samples: Vec<_> = values
        .into_iter()
        .enumerate()
        .map(|(i, value)| json!([START + 300 * i as i64, value]))
        .collect();
    json!({
        "metric": {"__name__": METRIC, "publication_id": id, "job": "fixture"},
        "values": samples
    })
}

fn matrix(result: Vec<Value>) -> Value {
    json!({"status":"success","data":{"resultType":"matrix","result":result}})
}

fn decode(body: &Value) -> Result<Vec<Option<Condition>>, &'static str> {
    decode_slot(&serde_json::to_vec(body).unwrap(), START, &[ID.to_string()])
}

#[test]
fn six_fresh_outcomes_cover_exact_half_open_intervals() {
    for (values, outcome) in [
        (["1"; 6], Condition::Operational),
        (["0"; 6], Condition::Unavailable),
        (["1", "1", "1", "0", "0", "1"], Condition::Degraded),
    ] {
        let mut samples = series(ID, values);
        samples["values"][5][0] = serde_json::from_str(&format!("{}.999", START + 1799)).unwrap();
        assert_eq!(decode(&matrix(vec![samples])).unwrap(), [Some(outcome)]);
    }
}

#[test]
fn gaps_duplicates_stale_samples_and_invalid_outcomes_are_not_evidence() {
    let valid = series(ID, ["1"; 6]);
    for (index, point) in [
        (0, json!([START - 1, "1"])),
        (1, json!([START, "1"])),
        (1, json!([START + 299, "1"])),
        (1, json!([START + 600, "1"])),
        (2, json!([START + 600, "NaN"])),
        (2, json!([START + 600, "2"])),
        (5, json!([START + 1800, "1"])),
    ] {
        let mut row = valid.clone();
        row["values"][index] = point;
        assert_eq!(decode(&matrix(vec![row])).unwrap(), [None]);
    }
    let mut sub_ms = valid.clone();
    sub_ms["values"][1][0] = serde_json::from_str(&format!("{}.0001", START + 300)).unwrap();
    assert_eq!(decode(&matrix(vec![sub_ms])).unwrap(), [None]);
    let mut gap = valid.clone();
    gap["values"].as_array_mut().unwrap().remove(2);
    assert_eq!(decode(&matrix(vec![gap])).unwrap(), [None]);
    let mut extra = valid.clone();
    extra["values"]
        .as_array_mut()
        .unwrap()
        .push(json!([START + 1799, "1"]));
    assert_eq!(decode(&matrix(vec![extra])).unwrap(), [None]);
    let mut histogram = valid.clone();
    histogram["histograms"] = json!([[START, {"sum":"1"}]]);
    assert_eq!(decode(&matrix(vec![histogram])).unwrap(), [None]);
    assert_eq!(decode(&matrix(vec![])).unwrap(), [None]);

    let mut duplicate = valid.clone();
    duplicate["metric"]["job"] = json!("other-fixture");
    let two = matrix(vec![valid, duplicate, series(OTHER, ["0"; 6])]);
    assert_eq!(
        decode_slot(
            &serde_json::to_vec(&two).unwrap(),
            START,
            &[ID.to_string(), OTHER.to_string()]
        )
        .unwrap(),
        [None, Some(Condition::Unavailable)]
    );
}

#[test]
fn truncated_annotated_wrong_shape_or_unrequested_data_is_rejected() {
    let valid = series(ID, ["1"; 6]);
    let mut warn = matrix(vec![valid.clone()]);
    warn["warnings"] = json!(["partial response"]);
    let mut info = matrix(vec![valid.clone()]);
    info["infos"] = json!(["ambiguous input"]);
    let mut vector = matrix(vec![valid.clone()]);
    vector["data"]["resultType"] = json!("vector");
    let mut wrong_name = valid.clone();
    wrong_name["metric"]["__name__"] = json!("up");
    for body in [
        warn,
        info,
        vector,
        matrix(vec![valid.clone(); CAPACITY + 1]),
        matrix(vec![series(OTHER, ["1"; 6])]),
        matrix(vec![wrong_name]),
        json!({"status":"error","error":"private failure"}),
    ] {
        assert_eq!(
            decode(&body).unwrap_err(),
            "invalid or incomplete model monitor response"
        );
    }
    assert!(decode_slot(b"{", START, &[ID.to_string()]).is_err());
    for bad in ["\"1800000000\"", "-1", "1e3", "9999999999999999999"] {
        assert!(millis(bad).is_none());
    }
}

#[tokio::test]
async fn runtime_rejects_redirects_and_bounds_chunked_responses_without_retries() {
    for address in ["0.0.0.0:9090", "127.0.0.1:0", "[::]:9090", "192.0.2.1:9090"] {
        assert!(Transport::new(address.parse().unwrap()).is_err());
    }
    assert!(Transport::new("[::1]:9090".parse().unwrap()).is_ok());
    let queries = Arc::new(AtomicUsize::new(0));
    let followed = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/api/v1/query",
            post({
                let queries = queries.clone();
                move || async move {
                    match queries.fetch_add(1, SeqCst) {
                        0 => (
                            StatusCode::TEMPORARY_REDIRECT,
                            [(header::LOCATION, "/unexpected")],
                        )
                            .into_response(),
                        1 => {
                            let chunk = vec![b' '; MAX_BYTES / 2 + 1];
                            let stream = tokio_stream::iter([
                                Ok::<_, std::io::Error>(chunk.clone()),
                                Ok(chunk),
                            ]);
                            Body::from_stream(stream).into_response()
                        }
                        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                    }
                }
            }),
        )
        .route(
            "/unexpected",
            any({
                let followed = followed.clone();
                move || async move {
                    followed.fetch_add(1, SeqCst);
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let transport = Transport::new(listener.local_addr().unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (i, error) in [
        "model monitor query failed",
        "model monitor response exceeds 128 KiB",
        "model monitor query failed",
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(transport.read_slot(START, &[ID]).await.unwrap_err(), error);
        assert_eq!(queries.load(SeqCst), i + 1);
        assert_eq!(followed.load(SeqCst), 0);
    }
    server.abort();
}
