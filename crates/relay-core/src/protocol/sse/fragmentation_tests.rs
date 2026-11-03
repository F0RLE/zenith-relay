use super::{consume_frames, line_end};

pub(super) fn scalar_line_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let position = bytes
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))?;
    let crlf = bytes[position] == b'\r' && bytes.get(position + 1) == Some(&b'\n');
    Some((position, position + 1 + usize::from(crlf)))
}

#[test]
fn line_end_matches_scalar_search_at_vector_boundaries() {
    for alignment in 0..32 {
        for length in [
            0, 1, 2, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 4096, 65536,
        ] {
            for ending in [b"".as_slice(), b"\r", b"\n", b"\r\n", b"\rX", b"\n\r"] {
                let mut bytes = vec![b'\n'; alignment];
                bytes.extend((0..length).map(|index| [b'x', 0x80, 0xff][index % 3]));
                bytes.extend_from_slice(ending);
                let input = &bytes[alignment..];
                assert_eq!(
                    line_end(input),
                    scalar_line_end(input),
                    "alignment={alignment}, length={length}, ending={ending:?}"
                );
            }
        }
    }
}

#[test]
fn fragmented_boundaries_match_full_rescanning() {
    for length in 0..=8 {
        for mut pattern in 0..3_usize.pow(length) {
            let input: Vec<_> = (0..length)
                .map(|_| {
                    let byte = b"x\r\n"[pattern % 3];
                    pattern /= 3;
                    byte
                })
                .collect();
            for split in 0..=input.len() {
                let mut pending = Vec::new();
                let mut reference = Vec::new();
                let mut actual = Vec::new();
                let mut expected = Vec::new();
                for chunk in [&input[..split], &input[split..]] {
                    reference.extend_from_slice(chunk);
                    let mut offset = 0;
                    while let Some((length, end)) = scalar_line_end(&reference[offset..]) {
                        offset += end;
                        if length == 0 {
                            expected.push(reference.drain(..offset).collect::<Vec<_>>());
                            offset = 0;
                        }
                    }
                    consume_frames(
                        &mut pending,
                        chunk,
                        &mut actual,
                        |_| false,
                        |frames, frame| frames.push(frame.to_vec()),
                    );
                    assert_eq!(actual, expected, "input {input:?}, split {split}");
                    assert_eq!(pending, reference, "input {input:?}, split {split}");
                }
            }
        }
    }
}
