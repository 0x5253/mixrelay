//! File recording workers, command acknowledgement, blocking waits and final joins.

use crate::{
    RecorderError, RecorderFailure, RecorderRequest, RecorderState, RecorderStatus, RecorderTap,
    recording_files::FileFactory,
    sessions::{SessionFactory, SessionHandle},
    transport::{self, Transport},
    worker::Worker,
};
use parking_lot::Mutex;
use std::{
    sync::{Arc, atomic::Ordering, mpsc},
    thread::JoinHandle,
    time::Duration,
};

pub(crate) struct ThreadedFiles;

impl SessionFactory for ThreadedFiles {
    type Handle = ThreadedSession;
    fn prepare(&self, channels: u16) -> Result<(Self::Handle, RecorderTap), RecorderError> {
        ThreadedSession::new(channels)
    }
}

pub(crate) enum Command {
    Request(
        RecorderRequest,
        mpsc::SyncSender<Result<RecorderStatus, RecorderError>>,
    ),
    Shutdown,
}

pub(crate) fn run<F: FileFactory>(
    mut worker: Worker<F>,
    receiver: &mpsc::Receiver<Command>,
) -> Result<(), RecorderError> {
    loop {
        worker.service();
        // Active and paused takes service retirement even without status requests.
        let command = if worker.has_take() {
            receiver.recv_timeout(Duration::from_millis(2))
        } else {
            receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(Command::Request(request, reply)) => {
                let _ = reply.send(worker.command(&request));
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return worker.shutdown();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

pub(crate) struct ThreadedSession {
    transport: Arc<Transport>,
    status: Arc<Mutex<RecorderStatus>>,
    sender: mpsc::Sender<Command>,
    worker: Option<JoinHandle<Result<(), RecorderError>>>,
}

impl ThreadedSession {
    fn new(channels: u16) -> Result<(Self, RecorderTap), RecorderError> {
        let transport = Transport::new(channels, std::thread::yield_now)
            .map_err(RecorderFailure::CaptureAllocation)?;
        let status = Arc::new(Mutex::new(RecorderStatus::default()));
        let tap = RecorderTap::new(Arc::clone(&transport));
        let (sender, receiver) = mpsc::channel();
        let worker = Worker::new(
            Arc::clone(&transport),
            Arc::clone(&status),
            crate::disk_files::DiskFiles,
        )
        .map_err(RecorderFailure::WriterAllocation)?;
        let handle = std::thread::Builder::new()
            .name("mixrelay-wav-writer".into())
            .spawn(move || run(worker, &receiver))
            .map_err(RecorderFailure::WorkerSpawn)?;
        Ok((
            Self {
                transport,
                status,
                sender,
                worker: Some(handle),
            },
            tap,
        ))
    }
}

impl SessionHandle for ThreadedSession {
    fn channels(&self) -> u16 {
        self.transport.channels
    }
    fn live_taps(&self) -> usize {
        self.transport.taps.load(Ordering::SeqCst)
    }
    fn tap(&self) -> RecorderTap {
        RecorderTap::new(Arc::clone(&self.transport))
    }

    fn status(&self) -> RecorderStatus {
        let mut status = self.status.lock().clone();
        if let Some(error) = transport::fault(self.transport.gate.load(Ordering::SeqCst)) {
            status.state = RecorderState::Failed;
            status.error = Some(error.to_string());
        }
        status
    }

    fn command(&self, request: RecorderRequest) -> Result<RecorderStatus, RecorderError> {
        let (reply, receiver) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Request(request, reply))
            .map_err(|_| RecorderFailure::WorkerDisconnected)?;
        receiver
            .recv()
            .map_err(|_| RecorderFailure::WorkerUnacknowledged)?
    }

    fn shutdown(&mut self) -> Result<(), RecorderError> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        let _ = self.sender.send(Command::Shutdown);
        worker.join().map_err(|_| RecorderFailure::WorkerPanicked)?
    }
}

impl Drop for ThreadedSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
