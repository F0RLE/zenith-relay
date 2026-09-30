use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

const PERIOD_MS: u64 = 30_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TotpCode {
    pub code: String,
    pub expires_at_ms: u64,
}

pub fn totp_code(secret: &str, now_ms: u64) -> Option<TotpCode> {
    let key = base32_decode(secret)?;
    let counter = now_ms / PERIOD_MS;
    let code = hotp(&key, counter)?;
    Some(TotpCode {
        code,
        expires_at_ms: counter.saturating_add(1).saturating_mul(PERIOD_MS),
    })
}

fn hotp(key: &[u8], counter: u64) -> Option<String> {
    let mut mac = HmacSha1::new_from_slice(key).ok()?;
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    Some(format!("{:06}", binary % 1_000_000))
}

fn base32_decode(value: &str) -> Option<Vec<u8>> {
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut output = Vec::new();
    for character in value.chars() {
        if character == '=' {
            break;
        }
        let index = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            '2'..='7' => character as u32 - '2' as u32 + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | index;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            output.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    (!output.is_empty()).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_matches_the_sha1_reference_vectors() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(totp_code(secret, 59_000).unwrap().code, "287082");
        assert_eq!(totp_code(secret, 1_111_111_109_000).unwrap().code, "081804");
        assert_eq!(totp_code(secret, 1_234_567_890_000).unwrap().code, "005924");
    }
}
