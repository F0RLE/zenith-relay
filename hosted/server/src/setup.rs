//! A short-lived, server-local first-owner ceremony. `serve` never mounts it.
use crate::{config::Config, db, http};
use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware,
    response::Response,
    routing::post,
};
use serde::Serialize;
use sqlx::PgPool;
use std::{
    fs,
    io::Write,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{net::TcpListener, sync::oneshot, time::Instant};
use webauthn_rs::prelude::*;
use webauthn_rs_proto::ResidentKeyRequirement;
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(180);
type Result<T> = std::result::Result<T, &'static str>;

fn secret() -> Zeroizing<String> {
    Zeroizing::new(format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    ))
}

enum Step {
    Start(Zeroizing<String>),
    Register(Zeroizing<String>, PasskeyRegistration),
    Prove(Zeroizing<String>, Box<Passkey>, PasskeyAuthentication),
    Spent,
}

impl Step {
    fn matches(&self, bearer: &str, stage: u8) -> bool {
        let code = match (self, stage) {
            (Self::Start(c), 0) | (Self::Register(c, _), 1) | (Self::Prove(c, _, _), 2) => c,
            _ => return false,
        };
        bool::from(code.as_bytes().ct_eq(bearer.as_bytes()))
    }
}

struct Ceremony {
    auth: Webauthn,
    db: PgPool,
    installation: Uuid,
    epoch: i64,
    db_deadline_ms: i64,
    origin: String,
    user: Uuid,
    until: Instant,
    step: Mutex<Step>,
    completed: Mutex<Option<oneshot::Sender<bool>>>,
}

impl Ceremony {
    fn finish(&self, bound: bool) {
        if let Some(sender) = self.completed.lock().unwrap().take() {
            let _ = sender.send(bound);
        }
    }

    fn take(&self, headers: &HeaderMap, stage: u8) -> Option<Step> {
        let origin = headers.get(header::ORIGIN)?.to_str().ok()?;
        let bearer = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        if self.until <= Instant::now() || origin != self.origin || bearer.len() != 64 {
            return None;
        }
        let mut step = self.step.lock().ok()?;
        step.matches(bearer, stage)
            .then(|| std::mem::replace(&mut *step, Step::Spent))
    }
}

fn denied() -> Response {
    http::error(
        StatusCode::FORBIDDEN,
        "setup_denied",
        "Setup authorization is not valid.",
    )
}

fn invalid() -> Response {
    http::error(
        StatusCode::BAD_REQUEST,
        "invalid_credential",
        "Passkey verification failed.",
    )
}

fn unavailable() -> Response {
    http::error(
        StatusCode::SERVICE_UNAVAILABLE,
        "setup_unavailable",
        "Setup unavailable.",
    )
}

pub(crate) async fn credential<T: serde::de::DeserializeOwned>(request: Request) -> Option<T> {
    if request
        .headers()
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?
        != "application/json"
    {
        return None;
    }
    let bytes = tokio::time::timeout(
        Duration::from_secs(5),
        to_bytes(request.into_body(), 64 * 1024),
    )
    .await
    .ok()?
    .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Serialize)]
struct Registration<'a> {
    session: &'a str,
    options: CreationChallengeResponse,
}

async fn start(State(s): State<Arc<Ceremony>>, req: Request) -> Response {
    if s.take(req.headers(), 0).is_none() {
        return denied();
    }
    let Ok((mut options, state)) = s
        .auth
        .start_passkey_registration(s.user, "owner", "Owner", None)
    else {
        return unavailable();
    };
    let Some(selection) = &mut options.public_key.authenticator_selection else {
        return unavailable();
    };
    selection.resident_key = Some(ResidentKeyRequirement::Required);
    selection.require_resident_key = true;
    let session = secret();
    let response = http::success(Registration {
        session: &session,
        options,
    });
    *s.step.lock().unwrap() = Step::Register(session, state);
    response
}

async fn register(State(s): State<Arc<Ceremony>>, req: Request) -> Response {
    let Some(Step::Register(session, state)) = s.take(req.headers(), 1) else {
        return denied();
    };
    let Some(reg) = credential::<RegisterPublicKeyCredential>(req).await else {
        return invalid();
    };
    if reg.type_ != "public-key" {
        return invalid();
    }
    let Ok(key) = s.auth.finish_passkey_registration(&reg, &state) else {
        return invalid();
    };
    if key.cred_id() != &reg.raw_id {
        return invalid();
    }
    let Ok((mut options, proof)) = s
        .auth
        .start_passkey_authentication(std::slice::from_ref(&key))
    else {
        return unavailable();
    };
    // Require an actual discoverable assertion before persisting this owner.
    // The server retains the one registered verifier and checks the user handle.
    options.public_key.allow_credentials.clear();
    *s.step.lock().unwrap() = Step::Prove(session, Box::new(key), proof);
    http::success(options)
}

async fn prove(State(s): State<Arc<Ceremony>>, req: Request) -> Response {
    let Some(Step::Prove(_, mut key, state)) = s.take(req.headers(), 2) else {
        return denied();
    };
    let Some(reg) = credential::<PublicKeyCredential>(req).await else {
        return invalid();
    };
    if reg.type_ != "public-key" || reg.get_user_unique_id() != Some(s.user.as_bytes()) {
        return invalid();
    }
    let Ok(proof) = s.auth.finish_passkey_authentication(&reg, &state) else {
        return invalid();
    };
    if !proof.user_verified() || key.update_credential(&proof).is_none() {
        return invalid();
    }
    let mut tx = match s.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return unavailable(),
    };
    if s.until <= Instant::now() {
        return denied();
    }
    match sqlx::query_scalar::<_, bool>(
        "SELECT relay_hosted.bind_first_owner($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(s.installation)
    .bind(s.epoch)
    .bind(&s.origin)
    .bind(s.user)
    .bind(key.cred_id().as_slice())
    .bind(sqlx::types::Json(&key))
    .bind(s.db_deadline_ms)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(true) => {
            if tx.commit().await.is_err() {
                return unavailable();
            }
            s.finish(true);
            http::success(serde_json::json!({"owner_bound":true}))
        }
        Ok(false) => {
            s.finish(false);
            http::error(
                StatusCode::CONFLICT,
                "setup_closed",
                "Owner setup is closed.",
            )
        }
        Err(_) => unavailable(),
    }
}

pub async fn run(c: &Config, pool: &PgPool, dist: &Path, code_dir: &Path) -> Result<()> {
    let origin = &c.server.public_origin;
    db::closed_setup(pool, origin).await?;
    let auth = crate::auth::webauthn(c)?.ok_or("configure auth.admin_rp_id for owner setup")?;
    let (installation, epoch, db_deadline_ms) = sqlx::query_as::<_, (Uuid, i64, i64)>(
        "SELECT id, authorization_epoch, floor(extract(epoch FROM clock_timestamp())*1000)::bigint + $2
         FROM relay_hosted.pending_owner WHERE public_origin=$1
         AND has_function_privilege('relay_hosted.bind_first_owner(uuid,bigint,text,uuid,bytea,jsonb,bigint)','EXECUTE')
         AND NOT has_function_privilege('relay_hosted.post_verified_payment(uuid,text,text,text,bigint)','EXECUTE')
         AND NOT has_schema_privilege('relay_hosted','CREATE')",
    ).bind(origin).bind(LIFETIME.as_millis() as i64).fetch_optional(pool).await.map_err(|_| "local setup role required")?
        .ok_or("owner already provisioned or setup role unavailable")?;
    let dir = fs::canonicalize(code_dir).map_err(|_| "private setup directory missing")?;
    if !dir.is_dir() || dir.starts_with(fs::canonicalize(dist).map_err(|_| "web build missing")?) {
        return Err("select a private setup directory outside the website");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&dir)
            .map_err(|_| "private setup directory missing")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("setup directory must be accessible only to its owner");
        }
    }
    let token = secret();
    let listener = TcpListener::bind(c.server.bind)
        .await
        .map_err(|_| "cannot bind setup listener")?;
    let mut file = tempfile::Builder::new()
        .prefix("relay-setup-")
        .tempfile_in(&dir)
        .map_err(|_| "cannot create setup file")?;
    file.write_all(token.as_bytes())
        .map_err(|_| "cannot write setup file")?;
    let (result, done) = oneshot::channel();
    let s = Arc::new(Ceremony {
        auth,
        db: pool.clone(),
        installation,
        epoch,
        db_deadline_ms,
        origin: origin.clone(),
        user: Uuid::new_v4(),
        step: Mutex::new(Step::Start(token)),
        until: Instant::now() + LIFETIME,
        completed: Mutex::new(Some(result)),
    });
    let app = Router::new()
        .route("/api/hosted/v1/setup/start", post(start))
        .route("/api/hosted/v1/setup/register", post(register))
        .route("/api/hosted/v1/setup/prove", post(prove));
    let app = http::website(app, dist, &["/setup/owner"])?
        .fallback(http::not_found)
        .method_not_allowed_fallback(http::wrong_method)
        .with_state(s.clone())
        .layer(middleware::from_fn(http::response_headers));
    eprintln!(
        "Open {origin}/setup/owner and enter the code from {}",
        file.path().display()
    );
    let (stop, signal) = oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = signal.await;
    });
    let server = std::future::IntoFuture::into_future(server);
    tokio::pin!(server);
    let bound = tokio::select! {
        result = done => result.unwrap_or(false),
        _ = tokio::time::sleep_until(s.until) => false,
        _ = tokio::signal::ctrl_c() => false,
        _ = &mut server => return Err("setup server stopped"),
    };
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .map_err(|_| "setup drain timed out")?
        .map_err(|_| "setup server stopped")?;
    if bound {
        Ok(())
    } else {
        Err("setup closed without binding an owner")
    }
}
