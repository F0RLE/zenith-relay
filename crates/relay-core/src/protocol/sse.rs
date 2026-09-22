pub(crate) fn event_end(bytes: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while let Some((length, end)) = line_end(&bytes[offset..]) {
        offset += end;
        if length == 0 {
            return Some(offset);
        }
    }
    None
}

pub(crate) fn lines(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let (length, end) = line_end(bytes).unwrap_or((bytes.len(), bytes.len()));
        let line = &bytes[..length];
        bytes = &bytes[end..];
        Some(line)
    })
}

pub(crate) fn data(event: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    for (index, value) in lines(event)
        .filter_map(|line| line.strip_prefix(b"data:"))
        .enumerate()
    {
        if index > 0 {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    data
}

fn line_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let position = bytes
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))?;
    let crlf = bytes[position] == b'\r' && bytes.get(position + 1) == Some(&b'\n');
    // CR is a complete SSE terminator by itself. If its optional LF arrives
    // after dispatch, the next frame contains a harmless empty leading line.
    Some((position, position + 1 + usize::from(crlf)))
}

#[cfg(test)]
mod tests {
    use super::{data, event_end, lines};

    #[test]
    fn data_preserves_empty_lines_and_only_strips_one_optional_space() {
        for ending in ["\n", "\r\n", "\r"] {
            let event = [
                "event: synthetic",
                "data:",
                "data:  text ",
                ": comment",
                "data:",
                "",
                "",
            ]
            .join(ending);
            assert_eq!(data(event.as_bytes()), b"\n text \n");
        }
    }

    #[test]
    fn finds_lf_and_crlf_sse_event_boundaries() {
        assert_eq!(event_end(b"data: one\n\nnext"), Some(11));
        assert_eq!(event_end(b"data: one\r\n\r\nnext"), Some(13));
        assert_eq!(event_end(b"data: one\n"), None);
    }

    #[test]
    fn mixed_line_endings_always_select_the_first_event() {
        for first in [
            b"data: one\n\n".as_slice(),
            b"data: one\r\n\r\n",
            b"data: one\n\r\n",
            b"data: one\r\n\n",
        ] {
            for second in [b"data: two\n\n".as_slice(), b"data: two\r\n\r\n"] {
                let joined = [first, second].concat();
                assert_eq!(event_end(&joined), Some(first.len()));
                assert_eq!(event_end(&joined[first.len()..]), Some(second.len()));
            }
        }
    }

    #[test]
    fn split_crlf_delimiter_accepts_a_complete_cr_terminated_blank_line() {
        let frame = b"data: one\r\n\r\n";
        for end in 0..frame.len() - 1 {
            assert_eq!(event_end(&frame[..end]), None);
        }
        assert_eq!(event_end(&frame[..frame.len() - 1]), Some(frame.len() - 1));
        assert_eq!(event_end(frame), Some(frame.len()));
    }

    #[test]
    fn recognizes_every_pair_of_sse_line_terminators() {
        for line in ["\n", "\r\n", "\r"] {
            for blank in ["\n", "\r\n", "\r"] {
                // Adjacent CR + LF is one terminator, not a blank line.
                if line == "\r" && blank == "\n" {
                    continue;
                }
                let frame = format!("data: {{}}{line}{blank}");
                assert_eq!(event_end(frame.as_bytes()), Some(frame.len()));
                let combined = format!("{frame}data: {{}}\n\n");
                assert_eq!(event_end(combined.as_bytes()), Some(frame.len()));
            }
        }
    }

    #[test]
    fn fragmented_mixed_endings_never_merge_data_from_separate_events() {
        let input = b"event: synthetic\rdata: {\n data\r\ndata: }\n\rdata: {}\r\ndata: \r\n\r\ndata: []\r\r";
        for chunk_size in 1..=input.len() {
            let mut pending = Vec::new();
            let mut events = Vec::new();
            for chunk in input.chunks(chunk_size) {
                pending.extend_from_slice(chunk);
                while let Some(end) = event_end(&pending) {
                    let frame = pending.drain(..end).collect::<Vec<_>>();
                    let data = lines(&frame)
                        .filter_map(|line| line.strip_prefix(b"data:"))
                        .map(<[u8]>::to_vec)
                        .collect::<Vec<_>>();
                    if !data.is_empty() {
                        events.push(data);
                    }
                }
            }
            assert!(pending.is_empty(), "chunk size {chunk_size}");
            assert_eq!(
                events,
                vec![
                    vec![b" {".to_vec(), b" }".to_vec()],
                    vec![b" {}".to_vec(), b" ".to_vec()],
                    vec![b" []".to_vec()]
                ]
            );
        }
    }
}
