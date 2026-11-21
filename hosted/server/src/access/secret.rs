use crate::auth::browser;
use ring::digest::{self, SHA256};
use std::fmt::Write;
use uuid::Uuid;
use zeroize::Zeroizing;

pub(super) struct Issued {
    pub id: Uuid,
    pub bearer: Zeroizing<String>,
    pub verifier: digest::Digest,
}

pub(super) fn mint() -> Option<Issued> {
    let entropy = browser::random()?;
    let id = Uuid::new_v4();
    let mut bearer = Zeroizing::new(String::with_capacity(101));
    write!(*bearer, "rlk_{}.", id.simple()).ok()?;
    for byte in entropy.iter() {
        write!(*bearer, "{byte:02x}").ok()?;
    }
    let verifier = digest::digest(&SHA256, bearer.as_bytes());
    Some(Issued {
        id,
        bearer,
        verifier,
    })
}

pub(crate) fn proof(bearer: &str) -> Option<(Uuid, digest::Digest)> {
    let (id, secret) = bearer.strip_prefix("rlk_")?.split_once('.')?;
    if id.len() != 32
        || secret.len() != 64
        || !id
            .bytes()
            .chain(secret.bytes())
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    Some((
        Uuid::parse_str(id).ok()?,
        digest::digest(&SHA256, bearer.as_bytes()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_complete_canonical_bearer_matches_its_verifier() {
        let key = mint().unwrap();
        let (id, digest) = proof(&key.bearer).unwrap();
        assert_eq!(id, key.id);
        assert_eq!(digest.as_ref(), key.verifier.as_ref());
        assert_ne!(mint().unwrap().verifier.as_ref(), digest.as_ref());
        assert!(proof(key.bearer.split_once('.').unwrap().0).is_none());
        assert!(proof(&key.bearer.to_uppercase()).is_none());
        assert!(proof(&hex::encode(digest)).is_none());
    }
}
