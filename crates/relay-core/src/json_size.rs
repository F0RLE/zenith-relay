//! Exact bounded JSON sizing without retaining a serialized copy.
use serde::Serialize;
use std::io::{self, Write};

pub(crate) fn serialized_len(value: &impl Serialize, max_bytes: usize) -> Option<usize> {
    let mut writer = SizeWriter {
        bytes: 0,
        max_bytes,
    };
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.bytes)
}

struct SizeWriter {
    bytes: usize,
    max_bytes: usize,
}

impl Write for SizeWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_all(bytes)?;
        Ok(bytes.len())
    }

    // serde_json emits many small writes; avoid the default write_all retry loop.
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > self.max_bytes - self.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON size limit exceeded",
            ));
        }
        self.bytes += bytes.len();
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "json_size/tests.rs"]
mod tests;
