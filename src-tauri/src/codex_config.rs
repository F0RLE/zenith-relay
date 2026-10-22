mod auth;
mod provider;

use crate::platform::default_codex_home;
pub(crate) use auth::load_api_key_for_launch;
use provider::{config_uses_zenith_provider, is_zenith_customer_key};
use std::sync::{Mutex, MutexGuard};

const PROVIDER_ID: &str = "codex_local_access";
const LEGACY_PROVIDER_ID: &str = "zenith";
const CONFIG_FILE: &str = "config.toml";
const AUTH_FILE: &str = "auth.json";
static CODEX_PROFILE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_codex_profile() -> MutexGuard<'static, ()> {
    zenith_relay_core::poison::mutex(&CODEX_PROFILE_LOCK)
}

#[cfg(test)]
mod tests;
