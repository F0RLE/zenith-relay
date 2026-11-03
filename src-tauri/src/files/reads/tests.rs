use super::*;
use std::io::Cursor;

#[test]
fn growing_read_consumes_only_one_byte_beyond_the_limit_and_fails_closed() {
    struct GrowingReader(usize);
    impl Read for GrowingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0 += buffer.len();
            assert!(self.0 <= 9, "the read must remain bounded even without EOF");
            buffer.fill(b'x');
            Ok(buffer.len())
        }
    }

    let error = read_limited(GrowingReader(0), 8).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(!error.to_string().contains("xxxxxxxx"));
    assert_eq!(
        read_limited(Cursor::new(b"12345678"), 8).unwrap(),
        b"12345678"
    );
}

#[test]
fn limited_read_preserves_io_failures_and_rejects_limit_overflow() {
    struct FailedReader;
    impl Read for FailedReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("synthetic read failure"))
        }
    }

    assert_eq!(
        read_limited(FailedReader, 8).unwrap_err().kind(),
        io::ErrorKind::Other
    );
    assert_eq!(
        read_limited(Cursor::new(b""), u64::MAX).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}
