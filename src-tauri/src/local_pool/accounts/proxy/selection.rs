use super::super::credentials::StoredCodexCredentials;
use super::COMMON_PROXY_SECRET_REF;
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::GatewaySettings,
    store::secret_store,
};
use zenith_relay_core::{protocol::ProxyMode, ProxyConfig};

pub fn effective_proxy_url(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<String>> {
    let proxy = choose_proxy_url(
        credentials.proxy_url(),
        credentials.bypass_common_proxy(),
        settings.common_proxy_configured,
        || secret_store::load(COMMON_PROXY_SECRET_REF),
    )?;
    ensure_account_proxy(settings, proxy.as_ref().map(|_| ()))?;
    Ok(proxy)
}

pub fn effective_proxy_config(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<ProxyConfig>> {
    effective_proxy_url(settings, credentials)?
        .map(|proxy_url| {
            ProxyConfig::parse(&proxy_url).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "stored account proxy URL is invalid",
                )
            })
        })
        .transpose()
}

pub fn common_proxy_url(settings: &GatewaySettings) -> Result<Option<String>> {
    if !settings.common_proxy_configured {
        return Ok(None);
    }
    let proxy_url = secret_store::load(COMMON_PROXY_SECRET_REF)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })?;
    ProxyConfig::parse(&proxy_url).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "stored common proxy URL is invalid",
        )
    })?;
    Ok(Some(proxy_url))
}

pub fn common_proxy_config(settings: &GatewaySettings) -> Result<Option<ProxyConfig>> {
    common_proxy_url(settings)?
        .map(|proxy_url| {
            ProxyConfig::parse(&proxy_url).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "stored common proxy URL is invalid",
                )
            })
        })
        .transpose()
}

/// The proxy choice already known without reading the common-proxy secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProxyRoute {
    pub has_account_proxy: bool,
    pub account_proxy_valid: bool,
    pub bypass_common_proxy: bool,
}

#[cfg(test)]
pub fn proxy_status(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
    common_available: bool,
) -> (ProxyMode, bool) {
    let account_proxy = credentials.proxy_url();
    proxy_route_status(
        settings,
        ProxyRoute {
            has_account_proxy: account_proxy.is_some(),
            account_proxy_valid: account_proxy
                .is_some_and(|proxy_url| ProxyConfig::parse(proxy_url).is_ok()),
            bypass_common_proxy: credentials.bypass_common_proxy(),
        },
        common_available,
    )
}

/// One route decision for both a live credential and a snapshot fact.
/// The second value is whether that route can carry account traffic.
pub fn proxy_route_status(
    settings: &GatewaySettings,
    route: ProxyRoute,
    common_available: bool,
) -> (ProxyMode, bool) {
    if route.has_account_proxy {
        return (ProxyMode::Account, route.account_proxy_valid);
    }
    if route.bypass_common_proxy {
        return (ProxyMode::Direct, !settings.account_proxy_required);
    }
    if settings.common_proxy_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !settings.account_proxy_required)
}

pub fn proxy_route_is_usable(
    settings: &GatewaySettings,
    route: ProxyRoute,
    common_available: bool,
) -> bool {
    proxy_route_status(settings, route, common_available).1
}

pub fn ensure_account_proxy<T>(settings: &GatewaySettings, proxy: Option<T>) -> Result<()> {
    if settings.account_proxy_required && proxy.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "an account proxy is required; direct account traffic is blocked",
        ));
    }
    Ok(())
}

pub fn common_proxy_available(settings: &GatewaySettings) -> bool {
    settings.common_proxy_configured
        && secret_store::load(COMMON_PROXY_SECRET_REF)
            .ok()
            .flatten()
            .is_some_and(|proxy_url| ProxyConfig::parse(&proxy_url).is_ok())
}

pub(super) fn choose_proxy_url(
    account_proxy: Option<&str>,
    bypass_common_proxy: bool,
    common_configured: bool,
    load_common: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let Some(account_proxy_url) = account_proxy {
        return Ok(Some(account_proxy_url.to_string()));
    }
    if bypass_common_proxy {
        return Ok(None);
    }
    if !common_configured {
        return Ok(None);
    }
    load_common()?.map(Some).ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })
}
