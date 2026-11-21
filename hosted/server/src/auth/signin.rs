use super::*;
use sqlx::types::Json;
use std::{
    collections::{HashMap, VecDeque},
    sync::Mutex,
    time::Instant,
};
use webauthn_rs::prelude::{
    DiscoverableAuthentication, DiscoverableKey, Passkey, PublicKeyCredential,
};

const LIFE: Duration = Duration::from_secs(120);
const CAP: usize = 64;

#[derive(Default)]
pub(super) struct Attempts(Mutex<Book>);

#[derive(Default)]
struct Book {
    pending: HashMap<[u8; 32], Pending>,
    starts: VecDeque<Instant>,
}

struct Pending {
    proof: DiscoverableAuthentication,
    epochs: (i64, i64, i64),
    until: Instant,
}

impl Book {
    fn budget(&mut self, now: Instant) -> bool {
        self.pending.retain(|_, p| p.until > now);
        while self.starts.front().is_some_and(|t| *t + LIFE <= now) {
            self.starts.pop_front();
        }
        if self.starts.len() >= CAP || self.pending.len() >= CAP {
            return false;
        }
        self.starts.push_back(now);
        true
    }
}

pub(super) async fn options(State(s): State<App>, headers: HeaderMap) -> Response {
    if !browser::same_origin(&headers, &s.origin) {
        return forbidden();
    }
    let Some(admin) = &s.auth else {
        return unavailable();
    };
    let now = Instant::now();
    if !admin.attempts.0.lock().unwrap().budget(now) {
        return http::error(
            StatusCode::TOO_MANY_REQUESTS,
            "admin_rate_limited",
            "Too many sign-in attempts.",
        );
    }
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let epochs =
        sqlx::query_as::<_, (i64, i64, i64)>("SELECT * FROM relay_hosted.owner_login_context($1)")
            .bind(&s.origin)
            .fetch_one(&s.pool)
            .await;
    let (Ok(epochs), Some(token), Ok((mut opts, proof))) = (
        epochs,
        browser::random(),
        admin.webauthn.start_discoverable_authentication(),
    ) else {
        return unavailable();
    };
    // Explicit button gesture rather than conditional autocomplete.
    opts.mediation = None;
    opts.public_key.timeout = Some(LIFE.as_millis() as u32);
    let mut book = admin.attempts.0.lock().unwrap();
    if let Some(old) = browser::cookie(&headers, ATTEMPT) {
        book.pending.remove(&*old);
    }
    book.pending.insert(
        *token,
        Pending {
            proof,
            epochs,
            until: now + LIFE,
        },
    );
    let mut r = http::success(opts);
    browser::set(&mut r, ATTEMPT, Some(&token), LIFE.as_secs() as u32);
    r
}

pub(super) async fn verify(State(s): State<App>, req: Request) -> Response {
    if !browser::same_origin(req.headers(), &s.origin) {
        return forbidden();
    }
    let Some(admin) = &s.auth else {
        return unavailable();
    };
    let Some(token) = browser::cookie(req.headers(), ATTEMPT) else {
        return denied();
    };
    let pending = admin.attempts.0.lock().unwrap().pending.remove(&*token);
    let Some(p) = pending.filter(|p| p.until > Instant::now()) else {
        return denied();
    };
    let Some(credential) = crate::setup::credential::<PublicKeyCredential>(req).await else {
        return denied();
    };
    if credential.type_ != "public-key" {
        return denied();
    }
    let Ok((user, id)) = admin
        .webauthn
        .identify_discoverable_authentication(&credential)
    else {
        return denied();
    };
    if db::closed_setup(&s.pool, &s.origin).await.is_err() {
        return unavailable();
    }
    let (installation_epoch, user_epoch, deadline) = p.epochs;
    let read = sqlx::query_as::<_, (i64, Json<Passkey>)>(
        "SELECT revision, passkey FROM relay_hosted.owner_verifier($1,$2,$3,$4,$5)",
    )
    .bind(&s.origin)
    .bind(user)
    .bind(id)
    .bind(installation_epoch)
    .bind(user_epoch)
    .fetch_optional(&s.pool)
    .await;
    let (revision, key) = match read {
        Ok(Some((rev, Json(key)))) if key.cred_id().as_slice() == id => (rev, key),
        Ok(_) => return denied(),
        Err(_) => return unavailable(),
    };
    let Ok(proof) = admin.webauthn.finish_discoverable_authentication(
        &credential,
        p.proof,
        &[DiscoverableKey::from(key)],
    ) else {
        return denied();
    };
    if !proof.user_verified() || proof.cred_id().as_slice() != id || Instant::now() >= p.until {
        return denied();
    }
    let Some(session) = browser::random() else {
        return unavailable();
    };
    let issued = sqlx::query_scalar::<_, bool>(
        "SELECT relay_hosted.issue_admin_session($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(&s.origin)
    .bind(user)
    .bind(id)
    .bind(installation_epoch)
    .bind(user_epoch)
    .bind(revision)
    .bind(deadline)
    .bind(i64::from(proof.counter()))
    .bind(proof.backup_state())
    .bind(proof.backup_eligible())
    .bind(admin.tag(b"session-v1", &session).as_ref())
    .fetch_one(&s.pool)
    .await;
    match issued {
        Ok(true) => {
            let mut r = http::success(serde_json::json!({"signed_in": true}));
            browser::set(&mut r, ATTEMPT, None, 0);
            browser::set(&mut r, SESSION, Some(&session), 8 * 3600);
            r
        }
        Ok(false) => denied(),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_budget_cannot_be_reset_by_consuming_or_replacing_a_cookie() {
        let mut book = Book::default();
        let start = Instant::now();
        for _ in 0..CAP {
            assert!(book.budget(start));
        }
        assert!(!book.budget(start + LIFE - Duration::from_nanos(1)));
        assert!(book.budget(start + LIFE));
        assert_eq!(book.starts.len(), 1);
    }
}
