// Relay's per-event memory budget, not a limit on the whole response.
pub(crate) const MAX_EVENT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn event_end(bytes: &[u8]) -> Option<usize> {
    event_end_from(bytes, 0)
}

fn event_end_from(bytes: &[u8], mut offset: usize) -> Option<usize> {
    while let Some((length, end)) = line_end(&bytes[offset..]) {
        let position = offset + length;
        offset += end;
        if position == 0
            || bytes[position - 1] == b'\n'
            || (bytes[position - 1] == b'\r' && bytes[position] != b'\n')
        {
            return Some(offset);
        }
    }
    None
}

/// Removes the next complete SSE event, including its blank-line terminator.
#[cfg(test)]
pub(crate) fn take_event(pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let end = event_end(pending)?;
    Some(pending.drain(..end).collect())
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

pub(crate) fn event_data(sse_event: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    let mut payloads = lines(sse_event)
        .filter_map(|line| line.strip_prefix(b"data:"))
        .map(|payload| payload.strip_prefix(b" ").unwrap_or(payload));
    let mut data = std::borrow::Cow::Borrowed(payloads.next().unwrap_or_default());
    for payload in payloads {
        let data_bytes = data.to_mut();
        data_bytes.push(b'\n');
        data_bytes.extend_from_slice(payload);
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

/// Delivers each complete SSE event until `stop` is true.
/// `pending` must be empty or the incomplete tail returned by a previous call.
/// Bytes after the stopping event are discarded. An already stopped caller
/// does not append `bytes`.
pub(crate) fn consume_frames<T>(
    pending: &mut Vec<u8>,
    bytes: &[u8],
    state: &mut T,
    stop: impl Fn(&T) -> bool,
    mut handle: impl FnMut(&mut T, &[u8]),
) {
    if stop(state) {
        return;
    }
    // The old tail has no complete event; revisit its last byte for split CRLF.
    let mut scan_from = pending.len().saturating_sub(1);
    let direct = pending.is_empty();
    let frames = if direct {
        bytes
    } else {
        pending.extend_from_slice(bytes);
        pending.as_slice()
    };
    let mut consumed = 0;
    while let Some(end) = event_end_from(&frames[consumed..], scan_from) {
        handle(state, &frames[consumed..consumed + end]);
        consumed += end;
        scan_from = 0;
        if stop(state) {
            pending.clear();
            return;
        }
    }
    if direct {
        pending.extend_from_slice(&bytes[consumed..]);
    } else {
        pending.drain(..consumed);
    }
}

/// Feeds `bytes` through an SSE buffer that cannot be borrowed with its owner.
/// The caller moves the buffer out and stores the returned tail.
pub(crate) fn push_pending_frames<T>(
    mut pending: Vec<u8>,
    bytes: &[u8],
    state: &mut T,
    stop: impl Fn(&T) -> bool,
    handle: impl FnMut(&mut T, &[u8]),
) -> Result<Vec<u8>, ()> {
    consume_bounded_frames(&mut pending, bytes, MAX_EVENT_BYTES, state, stop, handle)?;
    Ok(pending)
}

pub(crate) fn consume_bounded_frames<T>(
    pending: &mut Vec<u8>,
    mut bytes: &[u8],
    limit: usize,
    state: &mut T,
    stop: impl Fn(&T) -> bool,
    mut handle: impl FnMut(&mut T, &[u8]),
) -> Result<(), ()> {
    while !bytes.is_empty() && !stop(state) {
        let available = limit.saturating_sub(pending.len());
        if available == 0 {
            pending.clear();
            return Err(());
        }
        let count = available.min(bytes.len());
        consume_frames(pending, &bytes[..count], state, &stop, &mut handle);
        bytes = &bytes[count..];
    }
    Ok(())
}

#[cfg(test)]
#[path = "sse/throughput_tests.rs"]
mod throughput_tests;

#[cfg(test)]
#[path = "sse/limits_tests.rs"]
mod limits_tests;

#[cfg(test)]
#[path = "sse/fragmentation_tests.rs"]
mod fragmentation_tests;

#[cfg(test)]
mod tests {
    use super::{consume_frames, event_data, event_end, lines};

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
            assert_eq!(event_data(event.as_bytes()).as_ref(), b"\n text \n");
        }
    }

    #[test]
    fn single_data_line_borrows_its_payload() {
        for ending in ["\n", "\r\n", "\r"] {
            let event = format!("event: delta{ending}data: {{\"text\":\"hello\"}}{ending}{ending}");
            let data = event_data(event.as_bytes());
            assert!(matches!(data, std::borrow::Cow::Borrowed(_)));
            assert_eq!(data.as_ref(), b"{\"text\":\"hello\"}");
            assert!(std::ptr::eq(
                data.as_ptr(),
                event.as_bytes()[event.find('{').unwrap()..].as_ptr()
            ));
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
                consume_frames(
                    &mut pending,
                    chunk,
                    &mut events,
                    |_| false,
                    |events, frame| {
                        let event_data_lines = lines(frame)
                            .filter_map(|line| line.strip_prefix(b"data:"))
                            .map(<[u8]>::to_vec)
                            .collect::<Vec<_>>();
                        if !event_data_lines.is_empty() {
                            events.push(event_data_lines);
                        }
                    },
                );
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

    #[test]
    fn consume_frames_keeps_an_incomplete_tail_and_discards_after_stop() {
        let mut pending = Vec::new();
        let mut seen = Vec::new();
        consume_frames(
            &mut pending,
            b"data: one\n\ndata: two",
            &mut (),
            |_| false,
            |_, event| seen.push(event.to_vec()),
        );
        assert_eq!(seen, vec![b"data: one\n\n".to_vec()]);
        assert_eq!(pending, b"data: two");

        let mut delivered = 0;
        consume_frames(
            &mut pending,
            b"\n\ndata: three\n\n",
            &mut delivered,
            |delivered| *delivered > 0,
            |delivered, _| *delivered += 1,
        );
        assert_eq!(delivered, 1);
        assert!(pending.is_empty());

        let mut stopped = true;
        let mut handled = false;
        consume_frames(
            &mut pending,
            b"data: ignored\n\n",
            &mut stopped,
            |stopped| *stopped,
            |_, _| handled = true,
        );
        assert!(pending.is_empty());
        assert!(!handled);
    }

    #[test]
    fn consume_frames_borrows_complete_events_directly_from_the_input() {
        let first = b"data: first\n\n";
        let second = b"data: second\r\n\r\n";
        let tail = b"data: partial";
        let input = [first.as_slice(), second, tail].concat();
        let mut pending = Vec::new();
        let start = input.as_ptr();
        let mut seen = Vec::new();
        consume_frames(
            &mut pending,
            &input,
            &mut seen,
            |_| false,
            |seen, frame| seen.push((frame.as_ptr(), frame.to_vec())),
        );
        assert_eq!(seen[0], (start, first.to_vec()));
        assert_eq!(seen[1], (start.wrapping_add(first.len()), second.to_vec()));
        assert_eq!(pending, tail);
    }

    #[test]
    fn complete_frame_chunks_never_allocate_a_pending_buffer() {
        let mut pending = Vec::new();
        let mut delivered = 0;
        for frame in [b"data: one\n\n".as_slice(), b"data: two\r\n\r\n"] {
            consume_frames(
                &mut pending,
                frame,
                &mut delivered,
                |_| false,
                |total, event| *total += event.len(),
            );
            assert!(pending.is_empty());
            assert_eq!(pending.capacity(), 0);
        }
        assert_eq!(delivered, 24);
    }
}
