//! Native client for one owner-approved installation. The approval code is not a bearer.
mod copy;
mod inventory;
pub use copy::{
    COPY_COMMIT_PATH, COPY_STATUS_PATH, CopyInput, CopyKey, CopyPlan, CopyProof, CopyProxy,
    CopyReview, CopyState, MAX_REVIEW_CODE,
};
pub use inventory::{DevicePage, SavedSource};
use ring::{
    digest::{Context, SHA256},
    rand::{SecureRandom, SystemRandom},
};
use serde::Serialize;
use url::{Host, Url};
use uuid::Uuid;
use zeroize::Zeroizing;

pub const SCOPE: &str = "connections.read";
const INVALID: &str = "invalid_device_invitation";
pub(crate) const RESPONSE: &str = "invalid_device_response";

fn id(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s)
        .ok()
        .filter(|v| !v.is_nil() && v.to_string() == s)
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Destination {
    origin: String,
    installation: Uuid,
    device: Uuid,
}

impl Destination {
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        if raw.len() > 512 || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(INVALID);
        }
        let url = Url::parse(raw).map_err(|_| INVALID)?;
        let origin = url.origin().ascii_serialization();
        let local = match url.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(a)) => a.is_loopback(),
            Some(Host::Ipv6(a)) => a.is_loopback(),
            _ => false,
        };
        if !url.has_host() || !(url.scheme() == "https" || url.scheme() == "http" && local) {
            return Err(INVALID);
        }
        let (i, d) = raw
            .strip_prefix(&format!("{origin}/admin/connections#relay-device="))
            .and_then(|s| s.split_once('.'))
            .ok_or(INVALID)?;
        Ok(Self {
            origin,
            installation: id(i).ok_or(INVALID)?,
            device: id(d).ok_or(INVALID)?,
        })
    }
    pub fn invitation(&self) -> String {
        format!(
            "{}/admin/connections#relay-device={}.{}",
            self.origin, self.installation, self.device
        )
    }
    pub fn device(&self) -> Uuid {
        self.device
    }
    pub fn installation(&self) -> Uuid {
        self.installation
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

pub struct DeviceKey {
    target: Destination,
    secret: Zeroizing<[u8; 32]>,
}

fn decode(s: &str) -> Option<Zeroizing<[u8; 32]>> {
    if s.len() != 64
        || !s
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return None;
    }
    let mut bytes = Zeroizing::new([0; 32]);
    hex::decode_to_slice(s, bytes.as_mut()).ok()?;
    Some(bytes)
}

impl DeviceKey {
    pub fn generate(target: Destination) -> Result<Self, &'static str> {
        let mut secret = Zeroizing::new([0; 32]);
        SystemRandom::new()
            .fill(secret.as_mut())
            .map_err(|_| "device_randomness_unavailable")?;
        Ok(Self { target, secret })
    }
    pub fn target(&self) -> &Destination {
        &self.target
    }
    pub fn verifier(&self) -> [u8; 32] {
        let mut h = Context::new(&SHA256);
        for s in [
            b"relay-hosted-device-v1\0".as_slice(),
            self.target.origin.as_bytes(),
            b"\0",
            self.target.installation.as_bytes(),
            self.target.device.as_bytes(),
            self.secret.as_ref(),
        ] {
            h.update(s);
        }
        h.finish().as_ref().try_into().expect("SHA-256")
    }
    pub fn approval_code(&self) -> String {
        format!(
            "review1.{}.{}",
            self.target.device,
            hex::encode(self.verifier())
        )
    }
    pub fn review_proof(device: Uuid, code: &str) -> Option<[u8; 32]> {
        let rest = code.strip_prefix("review1.")?;
        let (d, digest) = rest.split_once('.')?;
        (id(d)? == device).then(|| decode(digest).map(|v| *v))?
    }
    pub fn protected_record(&self) -> Zeroizing<String> {
        let mut record = Zeroizing::new(self.target.invitation());
        record.push('\n');
        record.push_str(&Zeroizing::new(hex::encode(self.secret.as_ref())));
        record
    }
    pub fn restore(value: &str) -> Option<Self> {
        let (dest, s) = value.split_once('\n')?;
        Some(Self {
            target: Destination::parse(dest).ok()?,
            secret: decode(s)?,
        })
    }
    fn bearer(&self) -> Zeroizing<String> {
        let mut s = Zeroizing::new(format!(
            "rhd1.{}.{}.",
            self.target.installation, self.target.device
        ));
        s.push_str(&Zeroizing::new(hex::encode(self.secret.as_ref())));
        s
    }
    pub fn presented(origin: &str, bearer: &str) -> Option<(Uuid, Uuid, [u8; 32])> {
        let mut p = bearer.strip_prefix("rhd1.")?.split('.');
        let installation = id(p.next()?)?;
        let device = id(p.next()?)?;
        let secret = decode(p.next()?)?;
        if p.next().is_some() {
            return None;
        }
        let key = Self {
            target: Destination {
                origin: origin.to_owned(),
                installation,
                device,
            },
            secret,
        };
        Some((installation, device, key.verifier()))
    }
}
