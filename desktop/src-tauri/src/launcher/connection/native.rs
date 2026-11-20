use super::ConnectionError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use url::{Host, Url};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEYS: [&str; 2] = ["egressProxyUrl", "egressProxyPacUrl"];
type Fields = Map<String, Value>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub(super) proxy_id: Option<String>,
    library: PathBuf,
    profile_id: String,
    original: Fields,
    pub(super) previous: Fields,
    pub(super) applied: Fields,
    pub(super) pending: bool,
}

impl Record {
    pub(crate) fn valid(&self) -> bool {
        self.proxy_id
            .as_deref()
            .is_none_or(crate::local_pool::accounts::proxy::is_proxy_id)
            && self.library.is_absolute()
            && uuid::Uuid::parse_str(&self.profile_id)
                .is_ok_and(|id| id.to_string() == self.profile_id)
            && [&self.original, &self.previous, &self.applied]
                .into_iter()
                .all(valid_fields)
    }
}

pub(super) struct Config {
    library: PathBuf,
    profile_id: String,
    meta: Vec<u8>,
    bytes: Vec<u8>,
    value: Value,
}

pub(super) fn endpoint(raw: &str) -> Result<String, ConnectionError> {
    if raw.len() > 2048 {
        return Err(ConnectionError::Unsupported);
    }
    let url = Url::parse(raw).map_err(|_| ConnectionError::Unavailable)?;
    if zenith_relay_core::url_has_userinfo(&url) {
        return Err(ConnectionError::AuthUnsupported);
    }
    if !url.port_or_known_default().is_some_and(|port| port > 0)
        || !matches!(url.scheme(), "http" | "https")
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.host().is_some_and(|host| match host {
            Host::Domain(domain) => domain
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
            Host::Ipv4(_) | Host::Ipv6(_) => true,
        })
    {
        return Err(ConnectionError::Unsupported);
    }
    Ok(url.to_string())
}

pub(super) fn resolve() -> Result<Option<Config>, ConnectionError> {
    if !super::super::claude_desktop::native_connection_support()
        || std::env::var_os("CLAUDE_USER_DATA_DIR").is_some()
    {
        return Ok(None);
    }
    let library = crate::local_pool::profiles::claude_desktop::paths::config_library()
        .map_err(|_| ConnectionError::Unavailable)?;
    Config::load(&library)
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, ConnectionError> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
                    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(ConnectionError::Unavailable);
                    }
                }
                if meta.file_type().is_symlink() {
                    return Err(ConnectionError::Unavailable);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ConnectionError::Unavailable),
        }
    }
    match crate::files::read_regular_file_bounded(path, MAX_BYTES) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ConnectionError::Unavailable),
    }
}

fn parse(bytes: &[u8]) -> Result<Value, ConnectionError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ConnectionError::Unavailable)?;
    if !value.is_object() {
        return Err(ConnectionError::Unavailable);
    }
    Ok(value)
}

fn valid_fields(fields: &Fields) -> bool {
    fields.iter().all(|(key, value)| {
        KEYS.contains(&key.as_str())
            && (value.is_null()
                || value.as_str().is_some_and(|raw| {
                    raw.len() <= 2048
                        && Url::parse(raw).is_ok_and(|url| {
                            matches!(url.scheme(), "http" | "https")
                                && url.host().is_some()
                                && !zenith_relay_core::url_has_userinfo(&url)
                                && url.query().is_none()
                                && url.fragment().is_none()
                        })
                }))
    })
}

impl Config {
    pub(super) fn load(library: &Path) -> Result<Option<Self>, ConnectionError> {
        let Some(meta) = read(&library.join("_meta.json"))? else {
            return Ok(None);
        };
        let metadata = parse(&meta)?;
        // A bootstrap pointer or managed policy can supersede local settings.
        if metadata
            .get("hybridPointer")
            .is_some_and(|value| !value.is_null())
        {
            return Ok(None);
        }
        let Some(id) = metadata["appliedId"].as_str() else {
            return Ok(None);
        };
        let Ok(uuid) = uuid::Uuid::parse_str(id) else {
            return Err(ConnectionError::Unavailable);
        };
        if uuid.to_string() != id {
            return Err(ConnectionError::Unavailable);
        }
        let Some(bytes) = read(&library.join(format!("{id}.json")))? else {
            return Err(ConnectionError::Unavailable);
        };
        let value = parse(&bytes)?;
        if value
            .get("bootstrapUrl")
            .is_some_and(|value| !value.is_null())
        {
            return Ok(None);
        }
        Ok(Some(Self {
            library: library.into(),
            profile_id: id.to_owned(),
            meta,
            bytes,
            value,
        }))
    }

    pub(super) fn fields(&self) -> Fields {
        KEYS.iter()
            .filter_map(|key| {
                self.value
                    .get(key)
                    .map(|value| ((*key).into(), value.clone()))
            })
            .collect()
    }

    pub(super) fn matches_target(&self, record: &Record) -> bool {
        self.library == record.library && self.profile_id == record.profile_id
    }

    pub(super) fn externally_changed(&self, record: &Record) -> bool {
        self.matches_target(record)
            && self.fields() != record.applied
            && (!record.pending || self.fields() != record.previous)
    }

    pub(super) fn hash_into(&self, hash: &mut Sha256) {
        hash.update(self.library.as_os_str().as_encoded_bytes());
        hash.update(&self.meta);
        hash.update(&self.bytes);
    }

    pub(super) fn prepare(
        &self,
        record: Option<&Record>,
        proxy_id: Option<String>,
        endpoint: Option<String>,
    ) -> Result<Record, ConnectionError> {
        let fields = self.fields();
        if !valid_fields(&fields) {
            return Err(ConnectionError::AuthUnsupported);
        }
        if let Some(record) = record
            && (!record.valid()
                || !self.matches_target(record)
                || (fields != record.applied && (!record.pending || fields != record.previous)))
        {
            return Err(ConnectionError::Changed);
        }
        let original = record.map_or_else(|| fields.clone(), |record| record.original.clone());
        let applied = endpoint.map_or_else(
            || original.clone(),
            |url| Map::from_iter([(KEYS[0].into(), Value::String(url))]),
        );
        Ok(Record {
            proxy_id,
            library: self.library.clone(),
            profile_id: self.profile_id.clone(),
            original,
            previous: fields,
            applied,
            pending: true,
        })
    }

    pub(super) fn apply(
        &mut self,
        record: &Record,
        recovery_path: &Path,
    ) -> Result<(), ConnectionError> {
        let path = self.library.join(format!("{}.json", self.profile_id));
        if read(&self.library.join("_meta.json"))?.as_ref() != Some(&self.meta)
            || read(&path)?.as_ref() != Some(&self.bytes)
        {
            return Err(ConnectionError::Changed);
        }
        let mut value = self.value.clone();
        let object = value.as_object_mut().ok_or(ConnectionError::Unavailable)?;
        for key in KEYS {
            object.remove(key);
        }
        object.extend(record.applied.clone());
        let bytes = serde_json::to_vec_pretty(&value).map_err(|_| ConnectionError::Unavailable)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(ConnectionError::Unavailable);
        }
        crate::local_pool::profiles::claude_desktop::record_native_connection_update(
            &self.library,
            recovery_path,
            &self.profile_id,
            &self.bytes,
            &bytes,
        )
        .map_err(|_| ConnectionError::Unavailable)?;
        if read(&self.library.join("_meta.json"))?.as_ref() != Some(&self.meta)
            || read(&path)?.as_ref() != Some(&self.bytes)
        {
            return Err(ConnectionError::Changed);
        }
        crate::files::atomic_write_bytes(&path, &bytes)
            .map_err(|_| ConnectionError::Unavailable)?;
        if read(&path)?.as_ref() != Some(&bytes)
            || read(&self.library.join("_meta.json"))?.as_ref() != Some(&self.meta)
        {
            return Err(ConnectionError::Changed);
        }
        self.bytes = bytes;
        self.value = value;
        Ok(())
    }
}
