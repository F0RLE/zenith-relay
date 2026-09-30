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
        .map(|value| {
            ProxyConfig::parse(&value).map_err(|_| {
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
    let value = secret_store::load(COMMON_PROXY_SECRET_REF)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })?;
    ProxyConfig::parse(&value).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "stored common proxy URL is invalid",
        )
    })?;
    Ok(Some(value))
}

pub fn common_proxy_config(settings: &GatewaySettings) -> Result<Option<ProxyConfig>> {
    common_proxy_url(settings)?
        .map(|value| {
            ProxyConfig::parse(&value).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "stored common proxy URL is invalid",
                )
            })
        })
        .transpose()
}

pub fn proxy_status(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
    common_available: bool,
) -> (ProxyMode, bool) {
    if credentials.proxy_url().is_some() {
        return (
            ProxyMode::Account,
            credentials
                .proxy_url()
                .is_some_and(|value| ProxyConfig::parse(value).is_ok()),
        );
    }
    if credentials.bypass_common_proxy() {
        return (ProxyMode::Direct, !settings.account_proxy_required);
    }
    if settings.common_proxy_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !settings.account_proxy_required)
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
            .is_some_and(|value| ProxyConfig::parse(&value).is_ok())
}

pub(super) fn choose_proxy_url(
    account_proxy: Option<&str>,
    bypass_common_proxy: bool,
    common_configured: bool,
    load_common: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let Some(value) = account_proxy {
        return Ok(Some(value.to_string()));
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
