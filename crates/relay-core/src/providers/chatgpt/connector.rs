//! Provider sign-in and renewal, independent of Desktop windows and storage.

pub mod authority;
pub mod credentials;
pub mod import;
pub mod monitoring;
pub mod oauth;
pub mod oauth_flow;
pub mod observations;
pub mod reset_credits;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LimitedBodyError {
    Transport,
    TooLarge,
}

async fn collect_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, LimitedBodyError> {
    match crate::collect_limited(response, limit).await {
        Ok(bytes) => Ok(bytes),
        Err(crate::Error::UpstreamBodyTooLarge) => Err(LimitedBodyError::TooLarge),
        Err(_) => Err(LimitedBodyError::Transport),
    }
}

fn random_urlsafe(bytes: usize) -> String {
    let mut random_bytes = vec![0; bytes];
    rand::rng().fill_bytes(&mut random_bytes);
    URL_SAFE_NO_PAD.encode(random_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlsafe_tokens_are_unpadded_and_keep_the_requested_entropy() {
        let token = random_urlsafe(32);
        assert!(!token.contains('='));
        assert_eq!(URL_SAFE_NO_PAD.decode(token).unwrap().len(), 32);
    }
}
