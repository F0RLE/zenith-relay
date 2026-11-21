use super::*;
use axum::{body::Bytes, routing::any};
use std::{collections::HashMap, path::Path, process::Output, sync::Mutex};

const ROLE_FILE: &str = "ZENITH_RELAY_HOSTED_MODEL_MONITOR_DATABASE_URL_FILE";
const METRIC: &str = "relay_hosted_model_probe_success";

fn monitor_config(f: &Fixture) -> std::path::PathBuf {
    let path = f._cluster.temp.path().join("monitor.toml");
    fs::write(&path, include_str!("../../config.example.toml")).unwrap();
    path
}

async fn collect(c: &Cluster, cfg: &Path, monitor: Option<&str>, feed: &str) -> Output {
    let mut cmd = c.cli("fixture_boot", cfg);
    // The ordinary credential must not be used, not even as a fallback.
    cmd.args(["collect-model-status", "--prometheus", feed])
        .env_remove(ROLE_FILE);
    if let Some(role) = monitor {
        cmd.env(ROLE_FILE, c.role_file(role));
    }
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}

struct Feed {
    calls: Mutex<Vec<(i64, Vec<Uuid>)>>,
    outcome: Mutex<HashMap<Uuid, &'static str>>,
    evidence_slot: i64,
}

async fn telemetry(State(feed): State<Arc<Feed>>, headers: HeaderMap, body: Bytes) -> Json<Value> {
    assert!(!headers.contains_key("authorization"));
    let query: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap();
    assert_eq!(query.len(), 4);
    assert_eq!(query["limit"], "65");
    assert_eq!(query["timeout"], "2s");
    let slot: i64 = query["time"]
        .strip_suffix(".999")
        .unwrap()
        .parse::<i64>()
        .unwrap()
        - 1799;
    assert_eq!(slot % 1800, 0);
    let ids = query["query"]
        .strip_prefix(&format!(r#"{METRIC}{{publication_id=~""#))
        .unwrap()
        .strip_suffix(r#""}[30m]"#)
        .unwrap()
        .split('|')
        .map(|id| id.parse::<Uuid>().unwrap())
        .collect();
    feed.calls.lock().unwrap().push((slot, ids));
    let result: Vec<_> = if slot == feed.evidence_slot {
        feed.outcome
            .lock()
            .unwrap()
            .iter()
            .map(|(publication, value)| {
                let samples: Vec<_> = (0..6).map(|i| json!([slot + i * 300, value])).collect();
                json!({"metric":{"__name__":METRIC,"publication_id":publication},"values":samples})
            })
            .collect()
    } else {
        Vec::new()
    };
    Json(json!({"status":"success","data":{"resultType":"matrix","result":result}}))
}

async fn observations(admin: &PgPool) -> Vec<(Uuid, i64, String)> {
    sqlx::query_as(
        "SELECT publication_id,extract(epoch FROM slot_at)::bigint,state
         FROM relay_hosted.model_status_observations ORDER BY slot_at,publication_id",
    )
    .fetch_all(admin)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_monitor_collector_fills_only_covered_publication_slots() {
    let f = Fixture::new().await;
    let m = Monitor::new(&f).await;
    setup_reader(&f).await;
    let reader = Reader::new(&f, f.space, 1).await;
    let other_item = Uuid::new_v4();
    stock(
        &f.admin,
        f.scope,
        &f.a,
        other_item,
        purchase_limit_race::OTHER_MODEL,
        1000,
        ["2", ".50", "3"],
    )
    .await;
    let other = publish_at(&f.admin, other_item, 1, None, 1, Some(m.last_slot - 86400)).await;

    // Seed older success and outage receipts past the collector's write window.
    // The missing half-hour between the outage and recovery is never backfilled.
    sqlx::query(
        "INSERT INTO relay_hosted.model_status_observations(publication_id,slot_at,state)
         VALUES ($1,to_timestamp($2::bigint::double precision),'operational'),
                ($1,to_timestamp($3::bigint::double precision),'unavailable')",
    )
    .bind(m.publication)
    .bind(m.last_slot - 5400)
    .bind(m.last_slot - 3600)
    .execute(&f.admin)
    .await
    .unwrap();

    let feed = Arc::new(Feed {
        evidence_slot: m.last_slot,
        outcome: Mutex::new(HashMap::from([(m.publication, "1"), (other, "0")])),
        calls: Mutex::new(Vec::new()),
    });
    let telemetry = Listener::start(
        Router::new()
            .route("/api/v1/query", post(telemetry))
            .with_state(feed.clone()),
    )
    .await;
    let address = telemetry.url.strip_prefix("http://").unwrap();
    let cfg = monitor_config(&f);
    let run = || collect(&f._cluster, &cfg, Some("model_monitor"), address);

    let first = run().await;
    assert!(first.status.success(), "{:?}", first);
    assert!(
        String::from_utf8(first.stdout)
            .unwrap()
            .contains("2 accepted")
    );
    {
        let calls = feed.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].0 - calls[0].0, 1800);
        for (_, ids) in calls.iter() {
            assert!(ids.len() == 2 && ids.contains(&m.publication) && ids.contains(&other));
        }
        assert!(calls.iter().any(|(slot, _)| *slot == m.last_slot));
    }
    let saved = observations(&f.admin).await;
    assert_eq!(saved.len(), 4);
    assert!(saved.contains(&(m.publication, m.last_slot, "operational".into())));
    assert!(saved.contains(&(other, m.last_slot, "unavailable".into())));
    assert!(!saved.iter().any(|(_, slot, _)| *slot == m.last_slot - 1800));
    for hours in [24, 168] {
        let status = reader.read_status(hours).await.unwrap();
        assert_eq!(status["summary"]["operational_ms"], 3_600_000);
        assert_eq!(status["summary"]["monitored_ms"], 7_200_000);
        assert_eq!(status["summary"]["requests"], 0);
        assert!(!status.to_string().contains(&m.publication.to_string()));
    }

    let retry = run().await;
    assert!(retry.status.success(), "{:?}", retry);
    assert!(
        String::from_utf8(retry.stdout)
            .unwrap()
            .contains("2 accepted")
    );
    feed.outcome.lock().unwrap().insert(m.publication, "0");
    let conflict = run().await;
    assert!(conflict.status.success(), "{:?}", conflict);
    assert!(
        String::from_utf8(conflict.stdout)
            .unwrap()
            .contains("1 conflicts")
    );
    assert_eq!(observations(&f.admin).await, saved);

    // Swap the publication. A fresh epoch does not consume the old series,
    // even if the data source returns it incorrectly for a new selector.
    let fresh = publish_at(&f.admin, f.a.item, 3, None, 3, Some(m.last_slot)).await;
    assert_ne!(m.publication, fresh);
    feed.calls.lock().unwrap().clear();
    let stale = run().await;
    assert!(!stale.status.success());
    assert!(
        String::from_utf8(stale.stderr)
            .unwrap()
            .contains("invalid or incomplete model monitor response")
    );
    {
        let calls = feed.calls.lock().unwrap();
        assert!(calls.iter().all(|(_, ids)| !ids.contains(&m.publication)));
        assert!(calls.iter().any(|(_, ids)| ids.contains(&fresh)));
    }
    assert_eq!(observations(&f.admin).await, saved);
    let republished = reader.read_status(24).await.unwrap();
    assert_eq!(republished["summary"]["monitored_ms"], 1_800_000);
    let current = republished["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["model"] == MODEL)
        .unwrap();
    assert!(
        current["history"]
            .as_array()
            .unwrap()
            .iter()
            .all(|bin| bin["state"] == "unknown")
    );
    assert_eq!(f.sends(), (0, 0));
    payment::assert_wallet(&f.admin, f.space, CREDIT, 1).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; set RELAY_TEST_PG_BIN"]
async fn model_monitor_collector_rejects_broad_roles_wrong_origin_and_restore() {
    let f = Fixture::new().await;
    Monitor::new(&f).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let endpoint = Listener::start(Router::new().fallback(any({
        let calls = calls.clone();
        move || async move {
            calls.fetch_add(1, SeqCst);
        }
    })))
    .await;
    let address = endpoint.url.strip_prefix("http://").unwrap();
    let cfg = monitor_config(&f);
    for (role, error) in [
        (None, "database credential file path missing"),
        (
            Some("fixture_boot"),
            "a separate restricted model monitor role is required",
        ),
        (
            Some("route_worker"),
            "a separate restricted model monitor role is required",
        ),
    ] {
        let out = collect(&f._cluster, &cfg, role, address).await;
        assert!(!out.status.success());
        assert!(String::from_utf8(out.stderr).unwrap().contains(error));
    }
    for (grant, revoke) in [
        (
            "GRANT SELECT (label) ON relay_hosted.customer_keys TO model_monitor",
            "REVOKE SELECT (label) ON relay_hosted.customer_keys FROM model_monitor",
        ),
        (
            "GRANT pg_read_all_data TO model_monitor",
            "REVOKE pg_read_all_data FROM model_monitor",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(grant))
            .execute(&f.admin)
            .await
            .unwrap();
        let out = collect(&f._cluster, &cfg, Some("model_monitor"), address).await;
        assert!(!out.status.success());
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains("a separate restricted model monitor role is required")
        );
        sqlx::query(sqlx::AssertSqlSafe(revoke))
            .execute(&f.admin)
            .await
            .unwrap();
    }
    let real = fs::read_to_string(&cfg).unwrap();
    fs::write(&cfg, real.replace(ORIGIN, "http://localhost:1444")).unwrap();
    let out = collect(&f._cluster, &cfg, Some("model_monitor"), address).await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("incompatible installation or public origin")
    );
    fs::write(&cfg, real).unwrap();
    let installation: Uuid = sqlx::query_scalar("SELECT id FROM relay_hosted.installations")
        .fetch_one(&f.admin)
        .await
        .unwrap();
    db::fence_restore(&f.admin, ORIGIN, installation, Uuid::new_v4())
        .await
        .unwrap();
    let out = collect(&f._cluster, &cfg, Some("model_monitor"), address).await;
    assert!(!out.status.success());
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("installation restore is fenced")
    );
    assert!(observations(&f.admin).await.is_empty());
    assert_eq!(calls.load(SeqCst), 0);
    assert_eq!(f.sends(), (0, 0));
}
