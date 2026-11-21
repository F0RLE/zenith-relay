use super::{App, Duration, OwnerMutationError, denied, hmac, owner_mutation, unavailable};
use crate::{db, http, setup};
use axum::{extract::Request, http::StatusCode, response::Response};
use serde_json::json;
use sqlx::types::Json;
use sqlx::{Postgres, postgres::PgArguments, query::QueryScalar};
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::Instant,
};
use subtle::ConstantTimeEq;
use webauthn_rs::prelude::{
    AuthenticationResult, CredentialID, Passkey, PasskeyAuthentication, PublicKeyCredential,
    RequestChallengeResponse, Uuid, Webauthn,
};

const LIFE: Duration = Duration::from_secs(180);
const START_LIMIT: usize = 24;

pub(super) struct Attempts<T>(Mutex<Book<T>>);

struct Book<T> {
    pending: HashMap<Uuid, (Context, T)>,
    starts: VecDeque<Instant>,
}

impl<T> Default for Attempts<T> {
    fn default() -> Self {
        Self(Mutex::new(Book {
            pending: HashMap::new(),
            starts: VecDeque::new(),
        }))
    }
}

impl<T> Attempts<T> {
    pub(super) fn reserve(&self, now: Instant) -> bool {
        let mut book = self.0.lock().unwrap();
        book.pending.retain(|_, (c, _)| c.until > now);
        while book.starts.front().is_some_and(|t| *t + LIFE <= now) {
            book.starts.pop_front();
        }
        if book.starts.len() >= START_LIMIT {
            return false;
        }
        book.starts.push_back(now);
        true
    }

    pub(super) fn issue(&self, ctx: Context, step: T) -> Uuid {
        let id = Uuid::new_v4();
        self.0.lock().unwrap().pending.insert(id, (ctx, step));
        id
    }

    pub(super) fn take(&self, id: Uuid, tag: &hmac::Tag, now: Instant) -> Option<(Context, T)> {
        let mut book = self.0.lock().unwrap();
        let (ctx, _) = book.pending.get(&id)?;
        if !bool::from(ctx.tag.as_ref().ct_eq(tag.as_ref())) {
            return None;
        }
        book.pending.remove(&id).filter(|(ctx, _)| ctx.until > now)
    }
}

pub(super) struct Context {
    pub(super) tag: hmac::Tag,
    pub(super) owner: Uuid,
    pub(super) deadline_ms: i64,
    until: Instant,
}

impl Context {
    pub(super) fn new(tag: hmac::Tag, candidate: &Candidate, now: Instant) -> Self {
        Self {
            tag,
            owner: candidate.owner_id,
            deadline_ms: candidate.deadline_ms,
            until: now + LIFE,
        }
    }

    pub(super) fn timeout(&self) -> Option<u32> {
        let millis = self
            .until
            .saturating_duration_since(Instant::now())
            .as_millis();
        (millis > 0).then_some(millis.min(120_000) as u32)
    }
}

#[derive(sqlx::FromRow)]
pub(super) struct Candidate {
    owner_id: Uuid,
    deadline_ms: i64,
    pub(super) passkey_id: Uuid,
    revision: i64,
    passkey: Json<Passkey>,
}

pub(super) async fn candidates(s: &App, tag: &hmac::Tag) -> Result<Vec<Candidate>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM relay_hosted.owner_passkey_context($1,$2)")
        .bind(&s.origin)
        .bind(tag.as_ref())
        .fetch_all(&s.pool)
        .await
}

pub(super) struct Verification {
    state: PasskeyAuthentication,
    revisions: Vec<(CredentialID, i64)>,
}

pub(super) struct Approval {
    pub(super) proof: AuthenticationResult,
    pub(super) revision: i64,
}

impl Verification {
    pub(super) fn start(
        auth: &Webauthn,
        rows: Vec<Candidate>,
    ) -> Option<(RequestChallengeResponse, Self)> {
        let (keys, revisions): (Vec<_>, _) = rows
            .into_iter()
            .map(|r| {
                let id = r.passkey.cred_id().clone();
                (r.passkey.0, (id, r.revision))
            })
            .unzip();
        let (options, state) = auth.start_passkey_authentication(&keys).ok()?;
        Some((options, Self { state, revisions }))
    }

    pub(super) fn verify(
        &self,
        auth: &Webauthn,
        ctx: &Context,
        assertion: &PublicKeyCredential,
    ) -> Option<Approval> {
        if assertion.type_ != "public-key"
            || assertion
                .get_user_unique_id()
                .is_some_and(|v| v != ctx.owner.as_bytes())
        {
            return None;
        }
        let proof = auth
            .finish_passkey_authentication(assertion, &self.state)
            .ok()?;
        let (_, revision) = self
            .revisions
            .iter()
            .find(|(id, _)| id == proof.cred_id())?;
        proof.user_verified().then_some(Approval {
            proof,
            revision: *revision,
        })
    }

    pub(super) fn credential_ids(self) -> Vec<CredentialID> {
        self.revisions.into_iter().map(|(id, _)| id).collect()
    }
}

pub(super) struct Pending<T> {
    command: T,
    verification: Verification,
}

pub(super) enum VerifyError {
    Session(OwnerMutationError),
    Expired,
    InvalidAssertion,
    Unavailable,
}

impl VerifyError {
    pub(super) fn response(self) -> Response {
        match self {
            Self::Session(error) => error.response(),
            Self::Expired => expired(),
            Self::InvalidAssertion => http::error(
                StatusCode::BAD_REQUEST,
                "invalid_credential",
                "Passkey verification failed.",
            ),
            Self::Unavailable => unavailable(),
        }
    }
}

pub(super) async fn start<T>(
    s: &App,
    attempts: &Attempts<Pending<T>>,
    tag: hmac::Tag,
    command: T,
) -> Response {
    let admin = s.auth.as_ref().expect("authenticated");
    let now = Instant::now();
    let Ok(rows) = candidates(s, &tag).await else {
        return unavailable();
    };
    let Some(first) = rows.first() else {
        return denied();
    };
    if !attempts.reserve(now) {
        return http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "passkey_rate_limited",
            "Too many passkey attempts.",
        );
    }
    let ctx = Context::new(tag, first, now);
    let Some((mut options, verification)) = Verification::start(&admin.webauthn, rows) else {
        return unavailable();
    };
    let Some(timeout) = ctx.timeout() else {
        return expired();
    };
    options.public_key.timeout = Some(timeout);
    let ceremony_id = attempts.issue(
        ctx,
        Pending {
            command,
            verification,
        },
    );
    http::success(json!({"ceremony_id":ceremony_id,"options":options}))
}

pub(super) async fn verify<T>(
    s: &App,
    attempts: &Attempts<Pending<T>>,
    id: &str,
    req: Request,
) -> Result<(T, Context, Approval), VerifyError> {
    let tag = owner_mutation(s, req.headers()).map_err(VerifyError::Session)?;
    let admin = s.auth.as_ref().expect("authenticated");
    let id = Uuid::parse_str(id).map_err(|_| VerifyError::Expired)?;
    let (ctx, pending) = attempts
        .take(id, &tag, Instant::now())
        .ok_or(VerifyError::Expired)?;
    let approval = setup::credential::<PublicKeyCredential>(req)
        .await
        .and_then(|assertion| {
            pending
                .verification
                .verify(&admin.webauthn, &ctx, &assertion)
        })
        .ok_or(VerifyError::InvalidAssertion)?;
    if ctx.timeout().is_none() {
        return Err(VerifyError::Expired);
    }
    db::closed_setup(&s.pool, &s.origin)
        .await
        .map_err(|_| VerifyError::Unavailable)?;
    Ok((pending.command, ctx, approval))
}

pub(super) fn bind_proof<'q, O>(
    query: QueryScalar<'q, Postgres, O, PgArguments>,
    approval: Option<(&'q Context, &'q Approval)>,
) -> QueryScalar<'q, Postgres, O, PgArguments> {
    query
        .bind(approval.map(|(_, a)| a.proof.cred_id().as_slice()))
        .bind(approval.map(|(_, a)| a.revision))
        .bind(approval.map(|(_, a)| i64::from(a.proof.counter())))
        .bind(approval.map(|(_, a)| a.proof.backup_state()))
        .bind(approval.map(|(_, a)| a.proof.backup_eligible()))
        .bind(approval.map(|(c, _)| c.deadline_ms))
}

pub(super) fn error(code: &str) -> Option<Response> {
    match code {
        "fresh_required" => Some(http::error(
            StatusCode::FORBIDDEN,
            "fresh_passkey_required",
            "Confirm this change with an owner passkey.",
        )),
        "expired" => Some(expired()),
        "passkey_changed" => Some(http::error(
            StatusCode::CONFLICT,
            "passkey_changed",
            "Passkeys changed. Confirm the change again.",
        )),
        _ => None,
    }
}

fn expired() -> Response {
    http::error(
        StatusCode::CONFLICT,
        "passkey_ceremony_expired",
        "Passkey confirmation expired or was already used.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_session_bound_once_only_and_expires() {
        let key = hmac::Key::new(hmac::HMAC_SHA256, &[7; 32]);
        let session = hmac::sign(&key, b"owner-session");
        let other = hmac::sign(&key, b"another-session");
        let attempts = Attempts::default();
        let now = Instant::now();
        let context = || Context {
            tag: hmac::sign(&key, b"owner-session"),
            owner: Uuid::new_v4(),
            until: now + LIFE,
            deadline_ms: 100,
        };
        let id = attempts.issue(context(), "verify");
        assert!(attempts.take(id, &other, now).is_none());
        assert_eq!(attempts.take(id, &session, now).unwrap().1, "verify");
        assert!(attempts.take(id, &session, now).is_none());
        let expired = attempts.issue(context(), "verify");
        assert!(attempts.take(expired, &session, now + LIFE).is_none());
        for _ in 0..START_LIMIT {
            assert!(attempts.reserve(now));
        }
        assert!(!attempts.reserve(now + LIFE - Duration::from_millis(1)));
        assert!(attempts.reserve(now + LIFE));
    }
}
