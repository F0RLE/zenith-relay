pub(crate) use keyring_core::{Entry, Error};
use std::sync::{Arc, LazyLock};

type Store = Arc<keyring_core::CredentialStore>;

static STORE: LazyLock<Result<Store, Error>> = LazyLock::new(create_store);

fn create_store() -> Result<Store, Error> {
    #[cfg(target_os = "windows")]
    let store = windows_native_keyring_store::Store::new()?;
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new()?;
    #[cfg(target_os = "linux")]
    let store = dbus_secret_service_keyring_store::Store::new()?;
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        Ok(store)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(Error::NoDefaultStore)
    }
}

pub(crate) fn entry(service: &str, user: &str) -> Result<Entry, Error> {
    let store = STORE.as_ref().map_err(|_| Error::NoDefaultStore)?;
    // Keep the native service/user identity and provider defaults unchanged.
    store.build(service, user, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an unlocked native OS credential store; writes only a unique synthetic entry"]
    fn synthetic_secret_survives_entry_recreation() {
        struct Cleanup(Entry);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let service = "Zenith Relay credential-store test";
        let user = format!("synthetic-{}", uuid::Uuid::new_v4());
        let cleanup = Cleanup(entry(service, &user).unwrap());
        assert!(matches!(cleanup.0.get_password(), Err(Error::NoEntry)));
        cleanup.0.set_password("synthetic-test-value").unwrap();
        assert_eq!(
            entry(service, &user).unwrap().get_password().unwrap(),
            "synthetic-test-value"
        );
        cleanup.0.delete_credential().unwrap();
        assert!(matches!(
            entry(service, &user).unwrap().get_password(),
            Err(Error::NoEntry)
        ));
    }
}
