use atomicwrites::{AtomicFile, DisallowOverwrite};
use ring::digest::{SHA256, digest};
use ring::{hkdf, hmac};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;
use zenith_relay_vault_envelope::{Envelope, OVERHEAD};
use zeroize::Zeroizing;

use super::{Error, Purpose, Result};
const MAX_SECRET: usize = 64 * 1024;
const MAX_FILE: usize = MAX_SECRET + OVERHEAD;
const MARKER: &[u8] = b"Relay Hosted vault v1";

fn private(meta: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        !meta.file_type().is_symlink() && meta.permissions().mode() & 0o077 == 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & REPARSE_POINT == 0
    }
}

fn safe_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(Error::Storage);
    }
    for parent in path.ancestors() {
        #[cfg(windows)]
        if matches!(parent.components().next_back(), Some(Component::Prefix(_))) {
            continue;
        }
        let m = parent.symlink_metadata().map_err(|_| Error::Storage)?;
        if m.file_type().is_symlink() {
            return Err(Error::Storage);
        }
        #[cfg(windows)]
        if !private(&m) {
            return Err(Error::Storage);
        }
    }
    path.canonicalize().map_err(|_| Error::Storage)
}

fn read_private(path: &Path, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    let m = path.symlink_metadata().map_err(|_| Error::Storage)?;
    if !m.is_file() || !private(&m) || m.len() > max as u64 {
        return Err(Error::Storage);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    File::open(path)
        .and_then(|f| f.take(max as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| Error::Storage)?;
    if bytes.len() > max {
        return Err(Error::Storage);
    }
    Ok(bytes)
}

#[derive(Clone)]
pub(super) struct Files {
    pub root: PathBuf,
    key: Arc<Zeroizing<[u8; 32]>>,
    installation: Uuid,
}

pub(super) fn digest_of(bytes: &[u8]) -> [u8; 32] {
    digest(&SHA256, bytes).as_ref().try_into().expect("SHA-256")
}

pub(crate) fn read_operator_key(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = read_private(&safe_path(path)?, 32)?;
    let mut key = Zeroizing::new([0; 32]);
    if bytes.len() != key.len() {
        return Err(Error::Storage);
    }
    key.copy_from_slice(&bytes);
    Ok(key)
}

pub(super) fn new_slot_input(path: &Path, root: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let path = safe_path(path)?;
    if path.starts_with(root) {
        return Err(Error::Storage);
    }
    read_private(&path, MAX_SECRET)
}

impl Files {
    /// A database-only reader cannot check guesses of low-entropy proxy credentials.
    pub(super) fn provision_tag(
        &self,
        operation: Uuid,
        source: Uuid,
        provider: &[u8],
        proxy: Option<&[u8]>,
    ) -> Result<hmac::Tag> {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, self.installation.as_bytes());
        let prk = salt.extract(self.key.as_slice());
        let okm = prk
            .expand(
                &[b"relay-hosted:static-source-receipt:v1"],
                hmac::HMAC_SHA256,
            )
            .map_err(|_| Error::Storage)?;
        let mut tag = hmac::Context::with_key(&hmac::Key::from(okm));
        tag.update(operation.as_bytes());
        tag.update(source.as_bytes());
        tag.update(&(provider.len() as u64).to_be_bytes());
        tag.update(provider);
        tag.update(&[u8::from(proxy.is_some())]);
        if let Some(proxy) = proxy {
            tag.update(proxy);
        }
        Ok(tag.sign())
    }

    pub fn open(root: &Path, key_file: &Path, installation: Uuid) -> Result<Self> {
        let root = safe_path(root)?;
        let key_file = safe_path(key_file)?;
        let dir = root.metadata().map_err(|_| Error::Storage)?;
        if !dir.is_dir() || !private(&dir) || key_file.starts_with(&root) {
            return Err(Error::Storage);
        }
        let key = read_operator_key(&key_file)?;
        Ok(Self {
            root,
            key: Arc::new(key),
            installation,
        })
    }

    fn context(&self, connection: Uuid, purpose: u8, revision: i64, generation: Uuid) -> Vec<u8> {
        [
            &b"relay-hosted-vault:v1:key1"[..],
            self.installation.as_bytes(),
            connection.as_bytes(),
            &[purpose],
            &revision.to_be_bytes(),
            generation.as_bytes(),
        ]
        .concat()
    }

    fn write(&self, path: &Path, aad: &[u8], plain: &[u8]) -> Result<[u8; 32]> {
        let frame = Envelope::Hosted(aad)
            .seal(&self.key, plain)
            .map_err(|_| Error::Storage)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        AtomicFile::new(path, DisallowOverwrite)
            .write_with_options(|f| f.write_all(&frame), options)
            .map_err(|_| Error::Storage)?;
        Ok(digest_of(&frame))
    }

    fn read(&self, path: &Path, aad: &[u8], expected: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let frame = read_private(path, MAX_FILE)?;
        if digest_of(&frame) != expected {
            return Err(Error::Storage);
        }
        Envelope::Hosted(aad)
            .open(&self.key, &frame)
            .map_err(|_| Error::Storage)
    }

    pub fn bind_marker(&self, known: Option<&[u8]>) -> Result<[u8; 32]> {
        let path = self.root.join("installation.zhv");
        let aad = self.context(Uuid::nil(), 0, 0, Uuid::nil());
        let manifest = match known {
            Some(bytes) => bytes.try_into().map_err(|_| Error::Storage)?,
            None => {
                // Only an orphan marker can be adopted after an interrupted first bind.
                for entry in self.root.read_dir().map_err(|_| Error::Storage)? {
                    if entry.map_err(|_| Error::Storage)?.path() != path {
                        return Err(Error::Storage);
                    }
                }
                if path.try_exists().map_err(|_| Error::Storage)? {
                    digest_of(&read_private(&path, MAX_FILE)?)
                } else {
                    self.write(&path, &aad, MARKER)?
                }
            }
        };
        if self.read(&path, &aad, &manifest)?.as_slice() != MARKER {
            return Err(Error::Storage);
        }
        Ok(manifest)
    }

    pub fn generation(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.zhv"))
    }

    pub fn seal(
        &self,
        connection: Uuid,
        purpose: Purpose,
        revision: i64,
        generation: Uuid,
        data: &[u8],
    ) -> Result<[u8; 32]> {
        if data.is_empty() || data.len() > MAX_SECRET {
            return Err(Error::Invalid);
        }
        self.write(
            &self.generation(generation),
            &self.context(connection, purpose as u8, revision, generation),
            data,
        )
    }

    pub fn unseal(
        &self,
        connection: Uuid,
        purpose: Purpose,
        revision: i64,
        generation: Uuid,
        digest: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        self.read(
            &self.generation(generation),
            &self.context(connection, purpose as u8, revision, generation),
            digest,
        )
    }

    pub fn candidates(&self) -> Result<Vec<Uuid>> {
        let mut entries = Vec::new();
        for e in fs::read_dir(&self.root).map_err(|_| Error::Storage)? {
            let e = e.map_err(|_| Error::Storage)?;
            let Some(id) = e
                .file_name()
                .to_str()
                .and_then(|s| s.strip_suffix(".zhv"))
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            if self.generation(id) == e.path() {
                if entries.len() >= 16384 {
                    return Err(Error::Storage);
                }
                entries.push(id);
            }
        }
        Ok(entries)
    }
}
