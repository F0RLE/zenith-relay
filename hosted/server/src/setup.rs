//! Short-lived server-local owner ceremonies. `serve` never mounts them.
mod owner;
pub use owner::OwnerOperation;
use owner::Target;

use crate::{
    auth::{browser, enrollment},
    config::Config,
    db, http,
};
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
    target: Target,
    origin: String,
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
        let bearer =
            browser::one(headers, header::AUTHORIZATION.as_str())?.strip_prefix("Bearer ")?;
        if self.until <= Instant::now()
            || !browser::same_origin(headers, &self.origin)
            || bearer.len() != 64
        {
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
    let Some((options, state)) = enrollment::options(&s.auth, s.target.user, None) else {
        return unavailable();
    };
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
    let Some((options, proof, key)) = enrollment::register(&s.auth, &state, &reg) else {
        return invalid();
    };
    *s.step.lock().unwrap() = Step::Prove(session, Box::new(key), proof);
    http::success(options)
}

async fn prove(State(s): State<Arc<Ceremony>>, req: Request) -> Response {
    let Some(Step::Prove(_, key, state)) = s.take(req.headers(), 2) else {
        return denied();
    };
    let Some(reg) = credential::<PublicKeyCredential>(req).await else {
        return invalid();
    };
    let Some(key) = enrollment::prove(&s.auth, s.target.user, &state, *key, &reg) else {
        return invalid();
    };
    let mut tx = match s.db.begin().await {
        Ok(tx) => tx,
        Err(_) => return unavailable(),
    };
    if s.until <= Instant::now() {
        return denied();
    }
    match s.target.store(&mut tx, &s.origin, &key).await {
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
                "Owner ceremony is closed.",
            )
        }
        Err(_) => unavailable(),
    }
}

pub async fn run(
    c: &Config,
    pool: &PgPool,
    dist: &Path,
    code_dir: &Path,
    operation: OwnerOperation,
) -> Result<()> {
    let origin = &c.server.public_origin;
    db::closed_setup(pool, origin).await?;
    let auth = crate::auth::webauthn(c)?.ok_or("configure auth.admin_rp_id for owner passkeys")?;
    let Some(target) = Target::load(pool, origin, operation).await? else {
        println!("Owner recovery previously committed; no changes.");
        return Ok(());
    };
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
        target,
        origin: origin.clone(),
        step: Mutex::new(Step::Start(token)),
        until: Instant::now() + LIFETIME,
        completed: Mutex::new(Some(result)),
    });
    let app = Router::new()
        .route("/api/hosted/v1/setup/start", post(start))
        .route("/api/hosted/v1/setup/register", post(register))
        .route("/api/hosted/v1/setup/prove", post(prove));
    let page = operation.page();
    let app = http::website(app, dist, &[page])?
        .fallback(http::not_found)
        .method_not_allowed_fallback(http::wrong_method)
        .with_state(s.clone())
        .layer(middleware::from_fn(http::response_headers));
    eprintln!(
        "Open {origin}{page} and enter the code from {}",
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
        Err("owner ceremony closed without confirming a passkey")
    }
}
