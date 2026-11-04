//! Reapers with an injected worker, shared by the reaping and parent process tests.
use super::{Reaper, wait_for_owned_child};
use std::{io, thread};

impl Reaper {
    /// A reaper whose worker thread cannot start.
    pub(in crate::parent) fn refuse_preparation() -> io::Result<Self> {
        Self::prepare_with(|_| {
            Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "reaper unavailable",
            ))
        })
    }

    /// A reaper whose worker reports when it ends.
    pub(in crate::parent) fn observe_exit() -> io::Result<(Self, std::sync::mpsc::Receiver<()>)> {
        let (ended, observed) = std::sync::mpsc::channel();
        let reaper = Self::prepare_with(|handoff| {
            thread::Builder::new()
                .name("mixrelay-test-reaper".into())
                .spawn(move || {
                    wait_for_owned_child(&handoff);
                    let _ = ended.send(());
                })
        })?;
        Ok((reaper, observed))
    }
}
