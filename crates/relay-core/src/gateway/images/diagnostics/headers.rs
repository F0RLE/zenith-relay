use crate::usage::{ImageDimensions, ImageHeaderStatus};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use serde_json::value::RawValue;

// Never decode a raster or the complete image; even JPEG metadata is bounded.
const ENCODED_HEADER_LIMIT: usize = (16 * 1024 / 3) * 4;
type Header = (
    ImageHeaderStatus,
    Option<&'static str>,
    Option<ImageDimensions>,
);

pub(super) fn exact_size(text: &str) -> Option<ImageDimensions> {
    ImageDimensions::parse(text)
}

fn dimensions(width: u32, height: u32) -> Option<ImageDimensions> {
    (width > 0 && height > 0).then_some(ImageDimensions { width, height })
}

pub(super) fn inspect(b64: Option<&RawValue>, url: Option<&RawValue>) -> Header {
    let Some(raw) = b64.filter(|raw| raw.get() != "\"\"") else {
        let status = if url.is_some_and(|raw| raw.get().starts_with('"') && raw.get().len() > 2) {
            ImageHeaderStatus::UrlNotFetched
        } else {
            ImageHeaderStatus::Missing
        };
        return (status, None, None);
    };
    let Some(encoded) = encoded_prefix(raw.get()) else {
        return (ImageHeaderStatus::InvalidBase64, None, None);
    };
    let Ok(bytes) = STANDARD
        .decode(&encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(&encoded))
    else {
        return (ImageHeaderStatus::InvalidBase64, None, None);
    };
    let (format, size) = probe(&bytes);
    (
        if size.is_some() {
            ImageHeaderStatus::Observed
        } else {
            ImageHeaderStatus::UnknownOrIncomplete
        },
        format,
        size,
    )
}

// Decode only a bounded JSON-string prefix, including escaped slashes/unicode
// ASCII. A provider's image tail is neither decoded nor validated here.
fn encoded_prefix(raw: &str) -> Option<Vec<u8>> {
    let raw = raw.strip_prefix('"')?.strip_suffix('"')?.as_bytes();
    let mut output = Vec::with_capacity(raw.len().min(ENCODED_HEADER_LIMIT));
    let mut offset = 0;
    while offset < raw.len() && output.len() < ENCODED_HEADER_LIMIT {
        let byte = raw[offset];
        offset += 1;
        let byte = if byte == b'\\' {
            let escape = *raw.get(offset)?;
            offset += 1;
            match escape {
                b'/' | b'\\' | b'"' => escape,
                b'u' => {
                    let hex = std::str::from_utf8(raw.get(offset..offset + 4)?).ok()?;
                    offset += 4;
                    u8::from_str_radix(hex, 16).ok()?
                }
                _ => return None,
            }
        } else {
            byte
        };
        output.push(byte);
    }
    Some(output)
}

fn probe(bytes: &[u8]) -> (Option<&'static str>, Option<ImageDimensions>) {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let size = (|| {
            if bytes.get(8..16)? != b"\0\0\0\rIHDR" {
                return None;
            }
            dimensions(
                u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
                u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
            )
        })();
        return (Some("png"), size);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return (Some("gif"), le16_size(bytes, 6));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return (Some("webp"), webp_size(bytes));
    }
    if bytes.starts_with(b"\xff\xd8") {
        return (Some("jpeg"), jpeg_size(bytes));
    }
    (None, None)
}

fn le16_size(bytes: &[u8], offset: usize) -> Option<ImageDimensions> {
    dimensions(
        u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as u32,
        u16::from_le_bytes(bytes.get(offset + 2..offset + 4)?.try_into().ok()?) as u32,
    )
}

fn webp_size(bytes: &[u8]) -> Option<ImageDimensions> {
    let mut offset: usize = 12;
    while offset.checked_add(8)? <= bytes.len() {
        let kind = bytes.get(offset..offset + 4)?;
        let length =
            u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        let start = offset + 8;
        match kind {
            b"VP8X" if length >= 10 => {
                let chunk = bytes.get(start..start + 10)?;
                let width = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], 0]) + 1;
                let height = u32::from_le_bytes([chunk[7], chunk[8], chunk[9], 0]) + 1;
                return dimensions(width, height);
            }
            b"VP8L" if length >= 5 && bytes.get(start) == Some(&0x2f) => {
                let packed = u32::from_le_bytes(bytes.get(start + 1..start + 5)?.try_into().ok()?);
                return dimensions((packed & 0x3fff) + 1, ((packed >> 14) & 0x3fff) + 1);
            }
            b"VP8 " if length >= 10 && bytes.get(start + 3..start + 6) == Some(b"\x9d\x01\x2a") => {
                let size = le16_size(bytes, start + 6)?;
                return dimensions(size.width & 0x3fff, size.height & 0x3fff);
            }
            _ => {}
        }
        offset = start.checked_add(length)?.checked_add(length % 2)?;
    }
    None
}

fn jpeg_size(bytes: &[u8]) -> Option<ImageDimensions> {
    let mut offset = 2;
    while offset < bytes.len() {
        if *bytes.get(offset)? != 0xff {
            return None;
        }
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        if matches!(marker, 0xda | 0xd9 | 0x00) {
            return None;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd8) {
            continue;
        }
        let length = u16::from_be_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && length >= 8 {
            let height = u16::from_be_bytes(bytes.get(offset + 3..offset + 5)?.try_into().ok()?);
            let width = u16::from_be_bytes(bytes.get(offset + 5..offset + 7)?.try_into().ok()?);
            return dimensions(width as u32, height as u32);
        }
        offset += length;
    }
    None
}
