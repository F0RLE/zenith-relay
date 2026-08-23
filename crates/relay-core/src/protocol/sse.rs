pub(crate) fn event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

#[cfg(test)]
mod tests {
    use super::event_end;

    #[test]
    fn finds_lf_and_crlf_sse_event_boundaries() {
        assert_eq!(event_end(b"data: one\n\nnext"), Some(11));
        assert_eq!(event_end(b"data: one\r\n\r\nnext"), Some(13));
        assert_eq!(event_end(b"data: one\n"), None);
    }
}
