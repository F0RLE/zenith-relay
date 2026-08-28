use super::CommandResult;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use serde::Deserialize;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::accounts::{combine_import_documents, MAX_IMPORT_BYTES, MAX_IMPORT_ITEMS};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartAccountImportInput {
    #[serde(default)]
    pub(in crate::local_pool::accounts) content: Option<String>,
    #[serde(default)]
    pub(in crate::local_pool::accounts) documents: Vec<String>,
    #[serde(default)]
    pub(in crate::local_pool::accounts) source_file: Option<String>,
}

pub(crate) fn pick_account_import_documents(app: &AppHandle) -> CommandResult<Option<Vec<String>>> {
    let Some(files) = app
        .dialog()
        .file()
        .add_filter("Account files", &["json", "txt"])
        .blocking_pick_files()
    else {
        return Ok(None);
    };
    let paths = files
        .into_iter()
        .map(|file| {
            file.into_path().map_err(|_| {
                LocalPoolError::new(ErrorCode::InvalidState, "selected file path is invalid")
            })
        })
        .collect::<LocalResult<Vec<_>>>()?;
    read_import_documents(paths).map(Some).map_err(Into::into)
}

pub(crate) fn read_import_documents(paths: Vec<PathBuf>) -> LocalResult<Vec<String>> {
    if paths.is_empty() || paths.len() > MAX_IMPORT_ITEMS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("select between 1 and {MAX_IMPORT_ITEMS} import files"),
        ));
    }
    let mut total_bytes = 0usize;
    let mut documents = Vec::with_capacity(paths.len());
    for path in paths {
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                value.eq_ignore_ascii_case("json") || value.eq_ignore_ascii_case("txt")
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import file must use the .json or .txt extension",
            ));
        }
        let metadata = std::fs::metadata(&path).map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "failed to read selected import file")
        })?;
        if !metadata.is_file() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import path is not a file",
            ));
        }
        let length = usize::try_from(metadata.len()).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "selected import file is too large")
        })?;
        total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import files are too large",
            )
        })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import files are too large",
            ));
        }
        documents.push(std::fs::read_to_string(path).map_err(|_| {
            LocalPoolError::new(ErrorCode::Io, "failed to read selected import file")
        })?);
    }
    Ok(documents)
}

pub(in crate::local_pool::accounts) fn normalize_import_input(
    input: StartAccountImportInput,
) -> CommandResult<(String, Option<String>)> {
    let content = input.content.filter(|value| !value.trim().is_empty());
    if !input.documents.is_empty() {
        if content.is_some() || input.source_file.is_some() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "paste content and file documents cannot be imported together",
            )
            .into());
        }
        if input.documents.len() == 1 {
            return Ok((
                input
                    .documents
                    .into_iter()
                    .next()
                    .expect("one document exists"),
                None,
            ));
        }
        let content = combine_import_documents(&input.documents)
            .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.message))?;
        return Ok((content, None));
    }
    Ok((content.unwrap_or_default(), input.source_file))
}
