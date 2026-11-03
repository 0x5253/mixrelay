//! Reads the private readiness acknowledgement within the startup deadline.
use std::{
    io::{self, Read},
    sync::mpsc,
    thread,
    time::Duration,
};

// Startup may restore durable graphs before the owned control listener is ready.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn wait(reader: impl Read + Send + 'static) -> io::Result<()> {
    wait_with_timeout(reader, HANDSHAKE_TIMEOUT)
}

fn wait_with_timeout(mut reader: impl Read + Send + 'static, timeout: Duration) -> io::Result<()> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("mixrelay-engine-ready".into())
        .spawn(move || {
            let result = read_ready(&mut reader);
            // The parent may have timed out and dropped the receiver. The
            // reader thread has no process ownership and may then exit quietly.
            let _ = sender.send(result);
        })
        .map_err(|error| {
            io::Error::new(error.kind(), format!("start readiness reader: {error}"))
        })?;

    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "controller-owned engine readiness timed out",
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(
            "controller-owned engine readiness reader ended without a result",
        )),
    }
}

/// Accepts exactly the readiness acknowledgement as the first bytes of `reader`.
fn read_ready(reader: &mut impl Read) -> io::Result<()> {
    let mut acknowledgement = [0; crate::READY.len()];
    reader.read_exact(&mut acknowledgement)?;
    if acknowledgement != *crate::READY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "controller-owned engine readiness acknowledgement was invalid",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
