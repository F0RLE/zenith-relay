//! Codex's age-encrypted authorization namespace. The master key also protects
//! MCP/managed secrets: reuse it, never rotate it as part of profile recovery.
//! Contract: openai/codex 4b4a5cf, secrets/src/{lib,local}.rs.
use super::super::*;
use age::{scrypt, secrecy::SecretString};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::Rng;
use std::collections::BTreeMap;

const AUTH_KEY: &str = "global/CODEX_AUTH";
const FILE_NAME: &str = "codex_auth.age";
const MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Document {
    version: u8,
    secrets: BTreeMap<String, String>,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

pub(super) trait MasterKey {
    fn load(&self, home: &Path) -> Result<Option<String>>;
    fn save(&self, home: &Path, value: &str) -> Result<()>;
}

#[cfg(not(test))]
pub(super) struct NativeMasterKey;

#[cfg(not(test))]
fn key_entry(home: &Path) -> Result<keyring::Entry> {
    let home = canonical_profile_dir(home)?;
    let hash = key_hash(home.to_string_lossy().as_ref());
    keyring::Entry::new("codex", &format!("secrets|{}", &hash[..16])).map_err(|_| storage_error())
}

#[cfg(not(test))]
impl MasterKey for NativeMasterKey {
    fn load(&self, home: &Path) -> Result<Option<String>> {
        match key_entry(home)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(storage_error()),
        }
    }

    fn save(&self, home: &Path, value: &str) -> Result<()> {
        key_entry(home)?
            .set_password(value)
            .map_err(|_| storage_error())
    }
}

fn storage_error() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "Codex encrypted authorization cannot be read or updated; check its file and OS keyring",
    )
}

fn ciphertext(path: &Path) -> Result<Option<Vec<u8>>> {
    match crate::files::read_regular_file_bounded(path, MAX_BYTES) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(storage_error()),
    }
}

fn decode(bytes: &[u8], passphrase: &str) -> Result<Document> {
    let identity = scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    let plaintext = age::decrypt(&identity, bytes).map_err(|_| storage_error())?;
    if plaintext.len() > MAX_BYTES as usize {
        return Err(storage_error());
    }
    let document: Document = serde_json::from_slice(&plaintext).map_err(|_| storage_error())?;
    if document.version > 1 {
        return Err(storage_error());
    }
    Ok(document)
}

fn encode(document: &Document, passphrase: &str) -> Result<Vec<u8>> {
    let plaintext = serde_json::to_vec(document).map_err(|_| storage_error())?;
    if plaintext.len() > MAX_BYTES as usize {
        return Err(storage_error());
    }
    let recipient = scrypt::Recipient::new(SecretString::from(passphrase.to_owned()));
    let ciphertext = age::encrypt(&recipient, &plaintext).map_err(|_| storage_error())?;
    if ciphertext.len() > MAX_BYTES as usize {
        return Err(storage_error());
    }
    Ok(ciphertext)
}

pub(super) fn load(home: &Path, keys: &impl MasterKey) -> Result<Option<String>> {
    let Some(bytes) = ciphertext(&home.join("secrets").join(FILE_NAME))? else {
        return Ok(None);
    };
    let passphrase = keys.load(home)?.ok_or_else(storage_error)?;
    Ok(decode(&bytes, &passphrase)?.secrets.remove(AUTH_KEY))
}

pub(super) fn replace(home: &Path, auth: Option<&str>, keys: &impl MasterKey) -> Result<()> {
    let directory = home.join("secrets");
    let path = directory.join(FILE_NAME);
    let previous = ciphertext(&path)?;
    if previous.is_none() && auth.is_none() {
        return Ok(());
    }
    let passphrase = match keys.load(home)? {
        Some(passphrase) => passphrase,
        None => {
            // These namespaces share the key. A missing existing key needs
            // recovery, not replacement with a newly generated passphrase.
            for name in [FILE_NAME, "local.age", "mcp_oauth.age"] {
                match fs::symlink_metadata(directory.join(name)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => return Err(storage_error()),
                }
            }
            let mut bytes = [0_u8; 32];
            rand::rng().fill_bytes(&mut bytes);
            let passphrase = STANDARD.encode(bytes);
            keys.save(home, &passphrase)?;
            passphrase
        }
    };
    let mut document = match &previous {
        Some(bytes) => decode(bytes, &passphrase)?,
        None => Document {
            version: 1,
            secrets: BTreeMap::new(),
            extensions: BTreeMap::new(),
        },
    };
    match auth {
        Some(auth) => {
            document
                .secrets
                .insert(AUTH_KEY.to_owned(), auth.to_owned());
        }
        None => {
            document.secrets.remove(AUTH_KEY);
        }
    }
    let next = encode(&document, &passphrase)?;
    if ciphertext(&path)? != previous || keys.load(home)?.as_deref() != Some(passphrase.as_str()) {
        return Err(profile_restore_blocked());
    }
    crate::files::atomic_write_bytes(&path, &next).map_err(|_| storage_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Keys(RefCell<Option<String>>);

    impl MasterKey for Keys {
        fn load(&self, _: &Path) -> Result<Option<String>> {
            Ok(self.0.borrow().clone())
        }
        fn save(&self, _: &Path, value: &str) -> Result<()> {
            *self.0.borrow_mut() = Some(value.to_owned());
            Ok(())
        }
    }

    #[test]
    fn reads_scrypt_documents_written_by_age_0_12() {
        // Generated with age 0.12.1 using only synthetic credentials.
        let encoded = concat!(
            "6167652d656e6372797074696f6e2e6f72672f76310a2d3e20736372797074204535",
            "484c5470524e6a2b67523754676e2b57537436512031340a2b5141496743727a4e34",
            "7158335164745974786d646c53347845482b3373794a6546356f395a44443465450a",
            "2d2d2d205748514a48444e746b555859626a6b5a34376461444f4a6f73506f356252",
            "4150544838764b516d584268490a4bbca3561d8f374b5ba46c49f2b551092aa2d1f9",
            "b51c58fdb9d4191925c09835b32aa1b10d7630bdba01d9de1fc18ffccdbd200fa00",
            "ba97c3bf0a680376abf1222acee717c28155da1d4c929d1e43eee49cb31620736fb",
            "86249b598d29a2433e52bd71b6c54272ee3f05fd78dd5fa5078bde67723f57bb81c",
            "13411d063d736d6463cf146dcc22d23976a026f501d2f1b"
        );
        let bytes = (0..encoded.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&encoded[offset..offset + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let document = decode(&bytes, "synthetic-relay-compatibility-key").unwrap();
        assert_eq!(document.version, 1);
        assert_eq!(document.secrets[AUTH_KEY], "synthetic-auth");
        assert_eq!(document.secrets["mcp/example"], "synthetic-mcp");
        assert_eq!(document.extensions["future_field"], true);
        assert!(decode(&bytes, "synthetic-wrong-key").is_err());
    }

    #[test]
    fn encrypted_auth_round_trips_without_touching_mcp_or_other_entries() {
        let root = crate::files::test_temp_root("codex-encrypted-auth");
        let directory = root.join("secrets");
        fs::create_dir_all(&directory).unwrap();
        let keys = Keys(RefCell::new(Some("synthetic-passphrase".into())));
        let mut document = Document {
            version: 1,
            secrets: BTreeMap::from([("global/OTHER".into(), "synthetic-other".into())]),
            extensions: BTreeMap::from([("extension".into(), json!(true))]),
        };
        document
            .secrets
            .insert(AUTH_KEY.into(), "synthetic-before".into());
        fs::write(
            directory.join(FILE_NAME),
            encode(&document, "synthetic-passphrase").unwrap(),
        )
        .unwrap();
        fs::write(directory.join("mcp_oauth.age"), "synthetic-mcp-sentinel").unwrap();
        replace(&root, Some("synthetic-after"), &keys).unwrap();
        assert_eq!(
            load(&root, &keys).unwrap().as_deref(),
            Some("synthetic-after")
        );
        let bytes = fs::read(directory.join(FILE_NAME)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-after"));
        let restored = decode(&bytes, "synthetic-passphrase").unwrap();
        assert_eq!(restored.secrets["global/OTHER"], "synthetic-other");
        assert_eq!(restored.extensions["extension"], true);
        replace(&root, None, &keys).unwrap();
        assert!(load(&root, &keys).unwrap().is_none());
        assert_eq!(
            fs::read_to_string(directory.join("mcp_oauth.age")).unwrap(),
            "synthetic-mcp-sentinel"
        );
        assert_eq!(keys.0.borrow().as_deref(), Some("synthetic-passphrase"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_shared_master_key_never_rekeys_existing_secrets() {
        let root = crate::files::test_temp_root("codex-missing-master-key");
        let directory = root.join("secrets");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("local.age"), "synthetic-existing-secrets").unwrap();
        let keys = Keys::default();
        assert!(replace(&root, Some("synthetic-auth"), &keys).is_err());
        assert!(keys.0.borrow().is_none());
        assert!(!directory.join(FILE_NAME).exists());
        assert_eq!(
            fs::read_to_string(directory.join("local.age")).unwrap(),
            "synthetic-existing-secrets"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
