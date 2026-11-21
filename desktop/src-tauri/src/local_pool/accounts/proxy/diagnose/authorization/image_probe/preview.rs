//! Optional, explicitly requested local output; prompts and image bytes are not logged.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_PROMPT_BYTES: u64 = 16 * 1024;

pub(super) struct Preview {
    pub(super) prompt: String,
    pub(super) output: PathBuf,
}

pub(super) fn arguments(args: &[OsString]) -> Result<(&[OsString], Option<Preview>), &'static str> {
    let Some(index) = args.len().checked_sub(3) else {
        return Ok((args, None));
    };
    if args[index] != "--save-image" {
        return Ok((args, None));
    }
    // Retain the existing explicit quota-consumption consent and account selection.
    super::arguments(&args[..index])?;
    let prompt_path = Path::new(&args[index + 1]);
    let output = PathBuf::from(&args[index + 2]);
    if !prompt_path.is_absolute() || !output.is_absolute() {
        return Err("image_preview_paths_must_be_absolute");
    }
    if !output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        || !output.parent().is_some_and(Path::is_dir)
    {
        return Err("image_preview_requires_png_in_existing_directory");
    }
    if fs::symlink_metadata(&output).is_ok() {
        return Err("image_preview_output_already_exists");
    }
    let file = File::open(prompt_path).map_err(|_| "image_preview_prompt_unavailable")?;
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_PROMPT_BYTES)
    {
        return Err("image_preview_prompt_invalid");
    }
    let mut prompt = String::new();
    file.take(MAX_PROMPT_BYTES + 1)
        .read_to_string(&mut prompt)
        .map_err(|_| "image_preview_prompt_invalid")?;
    if prompt.trim().is_empty() || prompt.len() as u64 > MAX_PROMPT_BYTES {
        return Err("image_preview_prompt_invalid");
    }
    check_output_directory(&output)?;
    Ok((&args[..index], Some(Preview { prompt, output })))
}

fn check_output_directory(output: &Path) -> Result<(), &'static str> {
    // Check real write access before consuming quota, not just directory existence.
    let check = output.with_file_name(format!(".relay-image-preview-{}.png", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&check)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => "image_preview_directory_permission_denied",
            _ => "image_preview_directory_not_writable",
        })?;
    drop(file);
    // Remove only the exclusive empty file created by this preflight.
    crate::files::remove_regular_file(&check).map_err(|_| "image_preview_preflight_cleanup_failed")
}

impl Preview {
    pub(super) fn save(&self, response: &Value) -> Result<usize, &'static str> {
        let encoded = response
            .get("data")
            .and_then(Value::as_array)
            .filter(|images| images.len() == 1)
            .and_then(|images| images[0].get("b64_json"))
            .and_then(Value::as_str)
            .ok_or("image_preview_output_invalid")?;
        if super::response_outcome(response, super::png_dimensions(encoded)) != "completed" {
            return Err("image_preview_output_invalid");
        }
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|_| "image_preview_output_invalid")?;
        // Atomic create prevents overwriting a file added after preflight.
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.output)
            .map_err(|_| "image_preview_output_create_failed")?;
        output
            .write_all(&bytes)
            .and_then(|_| output.flush())
            .map_err(|_| "image_preview_output_write_failed")?;
        Ok(bytes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(root: &Path) -> Vec<OsString> {
        vec![
            "relay".into(),
            "--probe-bps-images".into(),
            root.as_os_str().into(),
            "--allow-generation".into(),
            "1024x1024".into(),
        ]
    }

    #[test]
    fn preview_keeps_default_probe_and_requires_explicit_consent() {
        let directory = crate::files::test_temp_root("bps-preview-consent");
        let mut args = args(&directory);
        assert!(arguments(&args).unwrap().1.is_none());
        args.extend([
            "--save-image".into(),
            directory.join("prompt.txt").into_os_string(),
            directory.join("image.png").into_os_string(),
        ]);
        args[3] = "--implicit".into();
        assert!(arguments(&args).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preview_loads_prompt_and_preserves_existing_files() {
        let directory = crate::files::test_temp_root("bps-preview-save");
        let prompt = directory.join("prompt.txt");
        let output = directory.join("image.png");
        fs::write(&prompt, "  synthetic image prompt\n").unwrap();
        let mut args = args(&directory);
        args.push("0123456789abcdef".into());
        args.extend([
            "--save-image".into(),
            prompt.as_os_str().into(),
            output.as_os_str().into(),
        ]);
        let (remaining, preview) = arguments(&args).unwrap();
        assert_eq!(remaining.len(), 6);
        let preview = preview.unwrap();
        assert_eq!(preview.prompt, "  synthetic image prompt\n");
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1024_u32.to_be_bytes());
        png.extend_from_slice(&1024_u32.to_be_bytes());
        let response = serde_json::json!({"data":[{"b64_json":STANDARD.encode(&png)}]});
        assert_eq!(preview.save(&response).unwrap(), png.len());
        assert_eq!(fs::read(&output).unwrap(), png);
        assert_eq!(
            preview.save(&response),
            Err("image_preview_output_create_failed")
        );
        assert!(arguments(&args).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preview_rejects_empty_oversized_prompt_and_provider_errors() {
        let directory = crate::files::test_temp_root("bps-preview-invalid");
        let prompt = directory.join("prompt.txt");
        let output = directory.join("image.png");
        let mut args = args(&directory);
        args.extend([
            "--save-image".into(),
            prompt.as_os_str().into(),
            output.as_os_str().into(),
        ]);
        for content in ["  ".to_owned(), "a".repeat(MAX_PROMPT_BYTES as usize + 1)] {
            fs::write(&prompt, content).unwrap();
            assert!(arguments(&args).is_err());
        }
        fs::write(&prompt, "synthetic image prompt").unwrap();
        let (_, preview) = arguments(&args).unwrap();
        assert!(
            preview
                .unwrap()
                .save(&serde_json::json!({"error":{"code":"synthetic"}}))
                .is_err()
        );
        assert!(!output.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
