mod diagnostics;
#[cfg(test)]
mod file_tests;
pub use diagnostics::CaptureLimits;
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File},
    io::Read,
    net::SocketAddr,
    path::{Component, Path},
};
use url::{Host, Url};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub database: Database,
    pub branding: Branding,
    pub auth: Option<Auth>,
    pub customer_oidc: Option<crate::identity::OidcSettings>,
    #[serde(default)]
    pub policies: PolicyLinks,
    #[serde(default)]
    pub network: crate::outbound::Network,
    #[serde(default)]
    pub diagnostics: diagnostics::Diagnostics,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refunds: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub support: Option<String>,
}

impl PolicyLinks {
    pub fn is_empty(&self) -> bool {
        self.terms.is_none()
            && self.privacy.is_none()
            && self.refunds.is_none()
            && self.support.is_none()
    }

    fn validate(&mut self) -> Result<(), &'static str> {
        for link in [
            &mut self.terms,
            &mut self.privacy,
            &mut self.refunds,
            &mut self.support,
        ]
        .into_iter()
        .flatten()
        {
            let error = "policy links must be public HTTPS URLs without credentials or parameters";
            let url = Url::parse(link).map_err(|_| error)?;
            if !link.starts_with("https://")
                || link.len() > 2048
                || link.contains('\\')
                || link.chars().any(|c| c.is_control() || c.is_whitespace())
                || url.domain().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(error);
            }
            *link = url.to_string();
            if link.len() > 2048 {
                return Err(error);
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub admin_rp_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Server {
    pub bind: SocketAddr,
    pub public_origin: String,
    #[serde(default)]
    pub web_api_intake: bool,
    #[serde(default)]
    pub web_model_checks: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub max_connections: u32,
    pub connect_timeout_ms: u64,
    pub lock_timeout_ms: u64,
    pub statement_timeout_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Branding {
    pub name: String,
}

fn read_limited(path: &Path, limit: u64, _private: bool) -> Option<Zeroizing<String>> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return None;
    }
    let directory = fs::canonicalize(path.parent()?).ok()?;
    let path = directory.join(path.file_name()?);
    if !path.starts_with(&directory) {
        return None;
    }
    let before = path.symlink_metadata().ok()?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > limit {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if before.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let file = File::open(&path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if _private && metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let mut text = Zeroizing::new(String::new());
    file.take(limit + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= limit).then_some(text)
}

fn checked_text(text: &str) -> Result<&str, &'static str> {
    if text.len() > 65_536 {
        return Err("configuration exceeds 64 KiB");
    }
    Ok(text)
}

impl Config {
    pub fn load() -> Result<Self, &'static str> {
        let path = env::var_os("ZENITH_RELAY_HOSTED_CONFIG").ok_or("configuration path missing")?;
        let text =
            read_limited(Path::new(&path), 65_536, false).ok_or("cannot read configuration")?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let mut c: Self =
            toml::from_str(checked_text(text)?).map_err(|_| "invalid configuration")?;
        if !c.server.bind.ip().is_loopback() || c.server.bind.port() == 0 {
            return Err("bind must be a loopback address with a nonzero port");
        }
        let origin = Url::parse(&c.server.public_origin).map_err(|_| "invalid public origin")?;
        let local = match origin.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        let canonical = origin.origin().ascii_serialization();
        if !(origin.scheme() == "https" || local && origin.scheme() == "http")
            || !origin.has_host()
            || (c.server.public_origin != canonical
                && c.server.public_origin.strip_suffix('/') != Some(&canonical))
        {
            return Err("expected an HTTPS origin (HTTP only on loopback)");
        }
        if let Some(auth) = &c.auth
            && origin.domain() != Some(&auth.admin_rp_id)
        {
            return Err("admin RP ID must be the exact public origin domain");
        }
        if c.server.web_api_intake && (origin.scheme() != "https" || c.auth.is_none()) {
            return Err("web API intake requires HTTPS and owner passkeys");
        }
        if c.server.web_model_checks && (!local || origin.scheme() != "https" || c.auth.is_none()) {
            return Err("web model checks require a local HTTPS qualification installation");
        }
        c.server.public_origin = canonical;
        if let Some(oidc) = &c.customer_oidc {
            oidc.validate(&origin)?;
            if c.auth.is_none() {
                return Err("customer qualification requires owner passkeys");
            }
        }
        let db = &c.database;
        if !(1..=64).contains(&db.max_connections)
            || !(100..=30_000).contains(&db.connect_timeout_ms)
            || !(100..=60_000).contains(&db.statement_timeout_ms)
            || !(100..=db.statement_timeout_ms).contains(&db.lock_timeout_ms)
        {
            return Err("invalid database limits");
        }
        let name = &c.branding.name;
        if name.trim() != name
            || !(1..=64).contains(&name.chars().count())
            || name.chars().any(char::is_control)
        {
            return Err("invalid branding name");
        }
        c.policies.validate()?;
        c.network.outbound_policy.validate()?;
        c.diagnostics.validate()?;
        Ok(c)
    }
}

pub fn database_url() -> Result<Zeroizing<String>, &'static str> {
    role_url("ZENITH_RELAY_HOSTED_DATABASE_URL_FILE")
}

pub fn role_url(variable: &str) -> Result<Zeroizing<String>, &'static str> {
    let path = env::var_os(variable).ok_or("database credential file path missing")?;
    let text = read_limited(Path::new(&path), 8192, true)
        .ok_or("cannot read private database credential")?;
    let dsn = text.trim_end_matches(['\r', '\n']);
    validate_database_url(dsn)?;
    Ok(Zeroizing::new(dsn.to_owned()))
}

pub fn validate_database_url(dsn: &str) -> Result<(), &'static str> {
    let url = Url::parse(dsn).map_err(|_| "invalid database credential")?;
    if !matches!(url.scheme(), "postgres" | "postgresql")
        || !url.has_host()
        || url.username().is_empty()
        || url.path().len() <= 1
        || url.path()[1..].contains('/')
        || url.fragment().is_some()
        || dsn.chars().any(char::is_whitespace)
    {
        return Err("expected an explicit PostgreSQL URL");
    }
    let mut mode = None;
    for (key, value) in url.query_pairs() {
        match &*key {
            "sslmode" if mode.is_none() => mode = Some(value),
            "sslrootcert" => (),
            _ => return Err("unsupported database URL option"),
        }
    }
    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
    let local = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !local && mode.as_deref() != Some("verify-full") {
        return Err("remote database connections require sslmode=verify-full");
    }
    Ok(())
}
