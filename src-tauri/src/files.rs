use std::{fs, path::Path};

pub fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Некорректный путь: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Не удалось создать {}: {err}", parent.display()))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)
        .map_err(|err| format!("Не удалось записать {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        format!("Не удалось заменить {}: {err}", path.display())
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

pub fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

pub fn unquote_toml_string(value: &str) -> Option<String> {
    let value = if let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        value
    } else {
        value.strip_prefix('\'')?.strip_suffix('\'')?
    };
    let mut out = String::new();
    let mut escaped = false;
    for ch in value.chars() {
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
