//! The exact acknowledgement and the bounded reader thread.
use super::*;
use std::{io::Cursor, time::Instant};

#[test]
fn readiness_requires_the_exact_acknowledgement() {
    assert!(read_ready(&mut Cursor::new(*crate::READY)).is_ok());
    let error =
        read_ready(&mut Cursor::new(b"MRREADY")).expect_err("short acknowledgement must fail");
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

    let error =
        read_ready(&mut Cursor::new(b"MRREADY!")).expect_err("wrong acknowledgement must fail");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[test]
fn the_reader_thread_delivers_its_result() {
    assert!(wait_with_timeout(Cursor::new(*crate::READY), Duration::from_secs(1)).is_ok());
}

#[test]
fn readiness_timeout_is_bounded() {
    let started = Instant::now();
    let error = wait_with_timeout(SlowReader, Duration::from_millis(10))
        .expect_err("a reader that never acknowledges must time out");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(1));
}

struct SlowReader;

impl Read for SlowReader {
    fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
        thread::sleep(Duration::from_millis(100));
        Ok(0)
    }
}
