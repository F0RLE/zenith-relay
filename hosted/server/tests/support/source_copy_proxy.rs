use crate::{PROVIDER, support::Cluster};
use axum::{
    Json, Router,
    extract::Request,
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde_json::json;
use sqlx::PgPool;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
};
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;
use zenith_relay_hosted::{config::Config, db, qualification, vault::Vault};
use zenith_relay_hosted_device::CopyPlan;

#[derive(Default)]
struct Traffic {
    direct: AtomicUsize,
    model_reads: AtomicUsize,
    responses: AtomicUsize,
    rejected: AtomicUsize,
    reject_auth: AtomicBool,
}

pub(super) struct Fixture {
    pub endpoint: String,
    pub proxy: String,
    traffic: Arc<Traffic>,
    tasks: [JoinHandle<()>; 2],
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Fixture {
    pub(super) fn counts(&self) -> [usize; 3] {
        [
            self.traffic.model_reads.load(SeqCst),
            self.traffic.responses.load(SeqCst),
            self.traffic.direct.load(SeqCst),
        ]
    }

    pub(super) async fn start() -> Self {
        let traffic = Arc::new(Traffic::default());
        let direct = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = direct.local_addr().unwrap().to_string();
        let endpoint = format!("http://{upstream}/v1");
        let app = Router::new().fallback({
            let traffic = traffic.clone();
            move || async move {
                traffic.direct.fetch_add(1, SeqCst);
                StatusCode::BAD_GATEWAY
            }
        });
        let direct_task = tokio::spawn(async move { axum::serve(direct, app).await.unwrap() });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!(
            "http://fixture-user:fixture-password@{}/",
            listener.local_addr().unwrap()
        );
        let app = Router::new().fallback({
            let traffic = traffic.clone();
            move |r: Request| async move {
                assert_eq!(r.uri().authority().unwrap().as_str(), upstream);
                assert!(r.headers()[header::AUTHORIZATION] == format!("Bearer {PROVIDER}"));
                // Basic encoding of the fixture username/password, not a real secret.
                assert!(r.headers()[header::PROXY_AUTHORIZATION] == "Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ=");
                if traffic.reject_auth.load(SeqCst) {
                    traffic.rejected.fetch_add(1, SeqCst);
                    return StatusCode::PROXY_AUTHENTICATION_REQUIRED.into_response();
                }
                match (r.method().as_str(), r.uri().path()) {
                    ("GET", "/v1/models") => {
                        traffic.model_reads.fetch_add(1, SeqCst);
                        Json(json!({"object":"list","data":[{"object":"model","id":"fixture-text"}]})).into_response()
                    }
                    ("POST", "/v1/responses") => {
                        traffic.responses.fetch_add(1, SeqCst);
                        let bytes = axum::body::to_bytes(r.into_body(), 8192).await.unwrap();
                        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(body["input"], "Reply with OK.");
                        assert_eq!(body["store"], false);
                        assert_eq!(body["max_output_tokens"], 64);
                        Json(json!({"object":"response","id":"resp_copy_probe","model":"fixture-text","status":"completed",
                            "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"OK"}]}],
                            "usage":{"input_tokens":4,"input_tokens_details":{"cached_tokens":0},"output_tokens":1,"total_tokens":5}})).into_response()
                    }
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
        });
        let proxy_task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            proxy,
            endpoint,
            traffic,
            tasks: [direct_task, proxy_task],
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn verify(
        &self,
        c: &Cluster,
        config: &Config,
        scope_id: Uuid,
        plan: &CopyPlan,
        vault_dir: &Path,
        vault_key: &Path,
    ) {
        let origin = &plan.origin;
        let vp = db::connect(config, &c.dsn("fixture_vault")).await.unwrap();
        let vault = Vault::open(&vp, origin, vault_dir, vault_key)
            .await
            .unwrap();
        let qp = db::connect(config, &c.dsn("fixture_qualifier"))
            .await
            .unwrap();
        let valid = json!({
            "id":Uuid::new_v4(),"scope":scope_id,"source":plan.source_id,"source_label":plan.label,
            "endpoint":plan.endpoint,"model":"fixture-text","provider_revision":1,"proxy_revision":1,
            "uses_proxy":true,"max_input_bytes":4000,"max_output_tokens":64,"tokens_per_byte":1,
            "overhead_tokens":0,"close_after_s":65,"valid_for_s":3600,
        });
        let file = c.temp.path().join("probe.json");
        let save = |profile: &serde_json::Value| {
            std::fs::write(&file, serde_json::to_vec(profile).unwrap()).unwrap();
        };
        save(&valid);
        qualification::check(
            qualification::scope(&qp, origin).await.unwrap(),
            &vault,
            &file,
            &config.network.outbound_policy,
        )
        .await
        .unwrap();
        assert_eq!(self.traffic.model_reads.load(SeqCst), 1);
        assert_eq!(self.traffic.responses.load(SeqCst), 1);
        assert_eq!(self.traffic.direct.load(SeqCst), 0);

        let mut other_route = valid.clone();
        other_route["id"] = json!(Uuid::new_v4());
        other_route["endpoint"] = json!(format!("http://127.0.0.1:{}/v1", crate::support::port()));
        save(&other_route);
        assert_eq!(
            qualification::check(
                qualification::scope(&qp, origin).await.unwrap(),
                &vault,
                &file,
                &config.network.outbound_policy,
            )
            .await,
            Err("source differs from the saved route")
        );

        let mut refused = valid;
        refused["id"] = json!(Uuid::new_v4());
        save(&refused);
        self.traffic.reject_auth.store(true, SeqCst);
        assert_eq!(
            qualification::check(
                qualification::scope(&qp, origin).await.unwrap(),
                &vault,
                &file,
                &config.network.outbound_policy,
            )
            .await,
            Err("model check not confirmed")
        );
        assert_eq!(self.traffic.rejected.load(SeqCst), 1);
        assert_eq!(self.traffic.model_reads.load(SeqCst), 1);
        assert_eq!(self.traffic.responses.load(SeqCst), 1);
        assert_eq!(self.traffic.direct.load(SeqCst), 0);
    }
}

pub(super) async fn activate_scope(admin: &PgPool, owner: Uuid) -> Uuid {
    let provider = Uuid::new_v4();
    let scope = Uuid::new_v4();
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("INSERT INTO relay_hosted.identity_providers VALUES ($1,'https://identity.example','fixture',$2)")
        .bind(provider).bind([21u8; 32].as_slice()).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO relay_hosted.configuration_revisions VALUES
        ($1,2,1,'qualification','verify_customer_access',$2,$3,ARRAY[$4::uuid],now(),now()+interval '1 hour')")
        .bind(scope).bind(owner).bind(provider).bind(Uuid::new_v4()).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE relay_hosted.service_control SET mode='qualification',revision=2,qualification_scope_ref=$1")
        .bind(scope).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    scope
}
