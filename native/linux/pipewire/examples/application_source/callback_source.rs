use super::{
    source::SourceError,
    tone::{Rate, Tone},
    trace::{Event, Trace},
};
use pipewire::stream::{Stream, StreamBox, StreamFlags, StreamListener};
use std::{
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

/// The native dispatch axis of the lab, not an OS scheduling-priority grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dispatch {
    /// Asynchronous application work on the control loop.
    MainLoop,
    /// Synchronous work on `PipeWire`'s data thread, under its existing OS policy.
    DataLoop,
}

impl Dispatch {
    /// Parses an explicit lab case; omitted means the existing main-loop case.
    /// # Errors
    /// Rejects any value other than `main` or `data`.
    pub(super) fn parse(value: Option<&str>) -> Result<Self, SourceError> {
        match value {
            None | Some("main") => Ok(Self::MainLoop),
            Some("data") => Ok(Self::DataLoop),
            Some(_) => Err(SourceError::Dispatch),
        }
    }

    /// Flags for the one stream implementation. Neither variant requests RT rights.
    pub(super) fn flags(self) -> StreamFlags {
        match self {
            Self::MainLoop => StreamFlags::MAP_BUFFERS,
            Self::DataLoop => StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        }
    }

    /// Exact dispatch contract stored beside the trace.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::MainLoop => "main-loop, MAP_BUFFERS only",
            Self::DataLoop => "data-loop, MAP_BUFFERS | RT_PROCESS",
        }
    }
}

struct Source {
    tone: Tone,
    published_position: Arc<AtomicU64>,
    // Taken only when the drained listener is dropped on the control thread.
    trace: Option<Trace>,
    completed: SyncSender<Trace>,
}

impl Source {
    fn process(&mut self, stream: &Stream) {
        let started = Instant::now();
        let trace = self
            .trace
            .as_mut()
            .expect("the listener retains its trace until native callbacks are drained");
        let mut event = Event {
            elapsed_ns: trace.elapsed_ns(),
            first_frame: self.tone.position(),
            native_time: stream.time().ok().map(Into::into),
            ..Event::default()
        };
        if let Some(mut buffer) = stream.dequeue_buffer() {
            let requested = buffer.requested();
            event.requested_frames = Some(requested);
            if let [data] = buffer.datas_mut() {
                event.capacity_bytes = Some(data.as_raw().maxsize);
                // The upstream accessor asserts non-null; reject an unusable
                // descriptor before borrowing native data or its chunk.
                if !data.as_raw().chunk.is_null()
                    && let Some(bytes) = data.data()
                {
                    event.produced_frames = self.tone.fill(bytes, requested);
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = 8;
                    *chunk.size_mut() = event.produced_frames * 8;
                }
            }
            // The upstream Buffer guard requeues each dequeued buffer once.
        }
        self.published_position
            .store(self.tone.position(), Ordering::Release);
        event.work_ns = started.elapsed().as_nanos();
        trace.record(event);
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // CallbackSource disconnects/drains before unregistering this listener.
        // This channel is used only by control-thread teardown, never process().
        // On setup failure its receiver may already be gone; reclaim there too.
        if let Some(trace) = self.trace.take() {
            let _ = self.completed.try_send(trace);
        }
    }
}

/// Unique native stream/listener owner; callback state is never shared with main.
///
/// Only the process event is registered. Its Send data moves into the listener,
/// then returns after synchronous native disconnection. No Rc, mutable alias,
/// mutex, channel operation or serialization occurs in the process callback.
pub(super) struct CallbackSource<'core> {
    stream: StreamBox<'core>,
    listener: Option<StreamListener<Source>>,
    completed: Receiver<Trace>,
    published_position: Arc<AtomicU64>,
}

impl<'core> CallbackSource<'core> {
    /// Registers the sole process callback before connection; allocates trace now.
    /// # Errors
    /// Propagates native listener registration failure before any callback can run.
    pub(super) fn new(stream: StreamBox<'core>, rate: Rate) -> Result<Self, SourceError> {
        let (sender, completed) = sync_channel(1);
        let published_position = Arc::new(AtomicU64::new(0));
        let listener = stream
            .add_local_listener_with_user_data(Source {
                tone: Tone::new(rate),
                trace: Some(Trace::new(65_536)),
                completed: sender,
                published_position: Arc::clone(&published_position),
            })
            .process(|stream, source| source.process(stream))
            .register()?;
        Ok(Self {
            stream,
            listener: Some(listener),
            completed,
            published_position,
        })
    }

    /// The borrowed stream cannot outlive the listener/drain owner.
    pub(super) fn stream(&self) -> &Stream {
        &self.stream
    }

    /// Last frame boundary completed by the render callback.
    pub(super) fn frame_position(&self) -> u64 {
        self.published_position.load(Ordering::Acquire)
    }

    /// Drains the callback, unregisters its state and returns the owned trace.
    /// # Errors
    /// Propagates native disconnection or missing post-drain handback.
    pub(super) fn finish(mut self) -> Result<Trace, SourceError> {
        self.stream.disconnect()?;
        drop(self.listener.take());
        self.completed.try_recv().map_err(SourceError::TraceReturn)
    }
}

impl Drop for CallbackSource<'_> {
    fn drop(&mut self) {
        // pw_stream_disconnect deactivates/destroys its impl node, synchronizing
        // its data-loop removal before returning. Main callbacks cannot run here
        // either: this owner is on the thread that iterates the control loop.
        // The same order applies to connect failures and early returns.
        let _ = self.stream.disconnect();
        drop(self.listener.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_changes_only_the_data_callback_flag() {
        assert_eq!(Dispatch::parse(None).unwrap(), Dispatch::MainLoop);
        assert_eq!(Dispatch::parse(Some("main")).unwrap(), Dispatch::MainLoop);
        assert_eq!(Dispatch::parse(Some("data")).unwrap(), Dispatch::DataLoop);
        assert!(Dispatch::parse(Some("rt-priority")).is_err());
        assert_eq!(Dispatch::MainLoop.flags(), StreamFlags::MAP_BUFFERS);
        assert_eq!(
            Dispatch::DataLoop.flags() ^ Dispatch::MainLoop.flags(),
            StreamFlags::RT_PROCESS
        );
        assert_ne!(Dispatch::MainLoop.label(), Dispatch::DataLoop.label());
    }

    #[test]
    fn callback_data_is_send_and_trace_returns_only_when_its_owner_drops() {
        fn assert_send<T: Send>() {}
        assert_send::<Source>();
        let (sender, receiver) = sync_channel(1);
        let source = Source {
            tone: Tone::new(Rate::Hz48000),
            trace: Some(Trace::new(2)),
            completed: sender,
            published_position: Arc::new(AtomicU64::new(0)),
        };
        assert!(receiver.try_recv().is_err());
        drop(source);
        let _trace = receiver.try_recv().unwrap();
        assert!(receiver.try_recv().is_err());
    }
}
