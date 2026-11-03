use std::{env, path::PathBuf};

pub(super) fn config_library() -> crate::local_pool::error::Result<PathBuf> {
    let path = library_from(
        std::env::consts::OS,
        env::var_os("CLAUDE_USER_DATA_DIR").map(PathBuf::from),
        env::var_os("LOCALAPPDATA").map(PathBuf::from),
        env::var_os("APPDATA").map(PathBuf::from),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_default(),
    );
    if !path.is_absolute() {
        return Err(crate::local_pool::error::LocalPoolError::new(
            crate::local_pool::error::ErrorCode::InvalidState,
            "Claude Desktop user-data directory must be an absolute path",
        ));
    }
    Ok(path)
}

fn library_from(
    os: &str,
    explicit: Option<PathBuf>,
    local: Option<PathBuf>,
    roaming: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: PathBuf,
) -> PathBuf {
    let nonempty = |path: &PathBuf| !path.as_os_str().is_empty();
    if let Some(explicit) = explicit.filter(nonempty) {
        return explicit.join("configLibrary");
    }
    let root = match os {
        "windows" => local
            .filter(nonempty)
            .map(|path| path.join("Claude-3p"))
            .unwrap_or_else(|| {
                roaming
                    .filter(nonempty)
                    .unwrap_or_else(|| home.join("AppData/Roaming"))
                    .join("Claude-3p")
            }),
        "macos" => home.join("Library/Application Support/Claude-3p"),
        _ => xdg
            .filter(nonempty)
            .unwrap_or_else(|| home.join(".config"))
            .join("Claude-3p"),
    };
    root.join("configLibrary")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_desktop_user_data_without_touching_claude_code_settings() {
        let home = PathBuf::from("home");
        assert_eq!(
            library_from(
                "windows",
                None,
                Some("local".into()),
                Some("roaming".into()),
                None,
                home.clone()
            ),
            PathBuf::from("local/Claude-3p/configLibrary")
        );
        assert_eq!(
            library_from("macos", None, None, None, None, home.clone()),
            home.join("Library/Application Support/Claude-3p/configLibrary")
        );
        assert_eq!(
            library_from("linux", None, None, None, Some("xdg".into()), home.clone()),
            PathBuf::from("xdg/Claude-3p/configLibrary")
        );
        assert_eq!(
            library_from(
                "windows",
                Some("custom".into()),
                Some("local".into()),
                None,
                None,
                home
            ),
            PathBuf::from("custom/configLibrary")
        );
    }
}
