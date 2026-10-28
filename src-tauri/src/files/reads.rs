use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[cfg(test)]
mod tests;

/// Read a bounded regular file through one handle without following a leaf link.
/// The caller still owns authorization of the containing directory and pathname.
pub(crate) fn read_regular_file_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    // Keep invalid-node errors stable and refuse special nodes before opening.
    // This preflight is not trusted for the later handle's type or size.
    let initial = fs::symlink_metadata(path)?;
    if !initial.is_file() || initial.file_type().is_symlink() || initial.len() > max_bytes {
        return Err(unsafe_file());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .security_qos_flags(0);
    }
    // Validate and read the same handle. A growing file cannot bypass the bound.
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unsafe_file());
        }
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > max_bytes {
        return Err(unsafe_file());
    }
    read_limited(file, max_bytes)
}

fn read_limited(reader: impl Read, max_bytes: u64) -> io::Result<Vec<u8>> {
    let read_limit = max_bytes.checked_add(1).ok_or_else(unsafe_file)?;
    let mut bytes = Vec::new();
    reader.take(read_limit).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(unsafe_file());
    }
    Ok(bytes)
}

fn unsafe_file() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "file type or size is unsafe")
}
