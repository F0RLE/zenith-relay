//! The Desktop ZDV1 envelope and its context-bound Hosted counterpart.
use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use zeroize::Zeroizing;

pub const OVERHEAD: usize = 4 + 12 + 16;

#[derive(Clone, Copy)]
pub enum Envelope<'a> {
    Desktop,
    Hosted(&'a [u8]),
}

impl Envelope<'_> {
    fn header(&self) -> &[u8; 4] {
        match self {
            Self::Desktop => b"ZDV1",
            Self::Hosted(_) => b"ZHV1",
        }
    }

    fn aad(&self) -> &[u8] {
        match self {
            Self::Desktop => &[],
            Self::Hosted(context) => context,
        }
    }

    pub fn seal(&self, key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|_| "system randomness unavailable")?;
        let ciphertext = ChaCha20Poly1305::new(key.into())
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plain,
                    aad: self.aad(),
                },
            )
            .map_err(|_| "vault encryption failed")?;
        let mut frame = Vec::with_capacity(16 + ciphertext.len());
        frame.extend_from_slice(self.header());
        frame.extend_from_slice(&nonce);
        frame.extend(ciphertext);
        Ok(frame)
    }

    pub fn open(&self, key: &[u8; 32], frame: &[u8]) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        if frame.len() < OVERHEAD || !frame.starts_with(self.header()) {
            return Err("secret vault file header is invalid");
        }
        let nonce = Nonce::try_from(&frame[4..16]).map_err(|_| "secret vault nonce is invalid")?;
        ChaCha20Poly1305::new(key.into())
            .decrypt(
                &nonce,
                Payload {
                    msg: &frame[16..],
                    aad: self.aad(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| "secret vault decryption failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_desktop_format_and_new_contexts_are_not_interchangeable() {
        let key = [13; 32];
        let nonce = [7; 12];
        let plain = b"synthetic test data";
        let cipher = ChaCha20Poly1305::new((&key).into());
        let legacy = cipher.encrypt(&Nonce::from(nonce), &plain[..]).unwrap();
        let old = [&b"ZDV1"[..], &nonce, &legacy].concat();
        assert_eq!(
            Envelope::Desktop.open(&key, &old).unwrap().as_slice(),
            plain
        );
        let new = Envelope::Desktop.seal(&key, plain).unwrap();
        assert_eq!(&new[..4], b"ZDV1");
        assert_eq!(
            cipher
                .decrypt(&Nonce::try_from(&new[4..16]).unwrap(), &new[16..])
                .unwrap(),
            plain
        );
        let ctx = Envelope::Hosted(b"slot A");
        let hosted = ctx.seal(&key, plain).unwrap();
        assert!(ctx.open(&key, &old).is_err());
        assert!(Envelope::Desktop.open(&key, &hosted).is_err());
        assert!(Envelope::Hosted(b"slot B").open(&key, &hosted).is_err());
    }
}
