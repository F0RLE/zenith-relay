use std::{fs, io::Write, path::Path};

mod reads;
pub(crate) use reads::read_regular_file_bounded;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use test_support::temp_root as test_temp_root;
#[cfg(test)]
pub(crate) use tests::fail_next_atomic_write;

/// Replace a caller-authorized file using exclusive, owner-private staging.
pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    atomic_write_bytes(path, content.as_bytes())
}

pub(crate) fn atomic_write_bytes(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid path: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Could not create {}: {err}", parent.display()))?;
    // Reserve a private staging file for each writer, including .tmp destinations.
    let temporary_path = parent.join(format!(".relay-write-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // An open failure must not remove a file that this writer did not create.
    let mut file = options
        .open(&temporary_path)
        .map_err(|err| format!("Could not write {}: {err}", temporary_path.display()))?;
    let written = (|| {
        #[cfg(test)]
        tests::check_injected_write_failure(path)?;
        file.write_all(content)?;
        file.sync_all()
    })();
    drop(file);
    written.map_err(|err| {
        let _ = fs::remove_file(&temporary_path);
        format!("Could not write {}: {err}", temporary_path.display())
    })?;
    fs::rename(&temporary_path, path).map_err(|err| {
        let _ = fs::remove_file(&temporary_path);
        format!("Could not replace {}: {err}", path.display())
    })
}

/// Removes one regular file. A missing path is success. A symlink or any other
/// node is refused so cleanup cannot follow a link.
pub(crate) fn remove_regular_file(path: &Path) -> Result<(), ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| ())
        }
        Ok(_) => Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

pub fn escape_json_string(raw_text: &str) -> String {
    raw_text
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub fn unquote_toml_string(raw_text: &str) -> Option<String> {
    let quoted_text = if let Some(double_quoted) = raw_text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    {
        double_quoted
    } else {
        raw_text.strip_prefix('\'')?.strip_suffix('\'')?
    };
    let mut out = String::new();
    let mut escaped = false;
    for ch in quoted_text.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    Some(out)
}
