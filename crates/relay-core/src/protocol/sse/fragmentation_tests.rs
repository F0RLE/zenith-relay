use super::{consume_frames, line_end};

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
                    while let Some((length, end)) = line_end(&reference[offset..]) {
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
