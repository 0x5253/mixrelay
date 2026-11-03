//! Frames one editor connection still has to write. Command replies leave first, in the
//! order their requests produce them; each telemetry subscription keeps only its latest
//! unwritten window (meters folded to the interval maximum, state the latest snapshot),
//! so a slow editor never delays a reply or grows memory.
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use mixrelay_control_protocol::{ControlTelemetry, TelemetryEncoder};
use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use tokio::sync::Notify;

use crate::control_channel::RESPONSE_LIMIT;

#[derive(Default)]
pub(super) struct Outbound {
    queue: Mutex<Queue>,
    ready: Notify,
    stopped: Notify,
    written: Notify,
}

#[derive(Default)]
struct Queue {
    replies: VecDeque<Value>,
    streams: BTreeMap<u64, Stream>,
    /// The subscription written last, so busy streams take turns.
    last_stream: u64,
    /// No frame is accepted any more: the writer failed or the connection finished.
    closed: bool,
    /// Write what is queued, then stop.
    finishing: bool,
}

#[derive(Default)]
struct Stream {
    window: Option<ControlTelemetry>,
    ended: Option<String>,
    encoder: TelemetryEncoder,
}

enum Next {
    Frame(Value),
    Idle,
    Done,
}

impl Outbound {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(&self) {
        self.ready.notify_one();
    }

    /// Queues a reply. A pending (`102`) frame is dropped while an earlier one for the same
    /// request is still unwritten: it says nothing new, and the backlog stays bounded by
    /// the requests in flight.
    pub(super) fn reply(&self, frame: Value) {
        let mut queue = self.queue();
        let repeated = frame["status"] == 102
            && queue
                .replies
                .iter()
                .any(|queued| queued["id"] == frame["id"] && queued["status"] == 102);
        if !queue.closed && !repeated {
            queue.replies.push_back(frame);
            drop(queue);
            self.wake();
        }
    }

    /// Replies not yet written; the connection admits no new request while requests in
    /// flight and this backlog together reach its limit.
    pub(super) fn backlog(&self) -> usize {
        self.queue().replies.len()
    }

    /// Resolves once fewer than `limit` replies wait to be written (or the queue closed).
    /// The condition is checked after registering for the wakeup, so a write between the
    /// caller's own check and this wait is never missed.
    pub(super) async fn backlog_below(&self, limit: usize) {
        loop {
            let written = self.written.notified();
            tokio::pin!(written);
            written.as_mut().enable();
            {
                let queue = self.queue();
                if queue.closed || queue.replies.len() < limit {
                    return;
                }
            }
            written.await;
        }
    }

    pub(super) fn open(&self, id: u64) {
        let mut queue = self.queue();
        if !queue.closed {
            queue.streams.entry(id).or_default();
        }
    }

    pub(super) fn window(&self, id: u64, window: ControlTelemetry) {
        let mut queue = self.queue();
        if let Some(stream) = queue.streams.get_mut(&id) {
            match &mut stream.window {
                Some(older) => older.absorb(window),
                slot @ None => *slot = Some(window),
            }
            drop(queue);
            self.wake();
        }
    }

    /// Ends a subscription after its pending window; the first reason wins.
    pub(super) fn end(&self, id: u64, reason: &str) {
        let mut queue = self.queue();
        if let Some(stream) = queue.streams.get_mut(&id) {
            stream.ended.get_or_insert_with(|| reason.to_owned());
            drop(queue);
            self.wake();
        }
    }

    /// Forgets a subscription the editor cancelled, including anything unwritten.
    pub(super) fn cancel(&self, id: u64) {
        self.queue().streams.remove(&id);
    }

    /// Writes what is queued, then lets the writer stop.
    pub(super) fn finish(&self) {
        self.queue().finishing = true;
        self.wake();
    }

    pub(super) fn close(&self) {
        let mut queue = self.queue();
        queue.closed = true;
        queue.replies.clear();
        queue.streams.clear();
        drop(queue);
        self.wake();
        self.stopped.notify_waiters();
        self.written.notify_waiters();
    }

    /// Resolves once nothing more can be written: the editor stopped reading or the
    /// connection finished.
    pub(super) async fn closed(&self) {
        loop {
            let stopped = self.stopped.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if self.queue().closed {
                return;
            }
            stopped.await;
        }
    }

    fn next(&self) -> Next {
        let mut guard = self.queue();
        let queue = &mut *guard;
        if queue.closed {
            return Next::Done;
        }
        if let Some(reply) = queue.replies.pop_front() {
            return Next::Frame(reply);
        }
        let Some((id, stream)) = next_ready(&mut queue.streams, queue.last_stream) else {
            return if queue.finishing {
                Next::Done
            } else {
                Next::Idle
            };
        };
        queue.last_stream = id;
        if let Some(window) = stream.window.take() {
            let body = stream.encoder.json(&window);
            return Next::Frame(json!({"id": id, "status": 206, "body": body}));
        }
        let reason = stream.ended.take().unwrap_or_default();
        queue.streams.remove(&id);
        Next::Frame(json!({
            "id": id,
            "status": 503,
            "body": {"code": "control_unavailable", "parameters": {"reason": reason}},
        }))
    }

    /// Writes frames until finished and drained, or until a write fails or stalls for
    /// `timeout`; then nothing more is accepted.
    pub(super) async fn write_all<W: AsyncWrite + Unpin>(
        &self,
        write: &mut W,
        timeout: Duration,
    ) -> io::Result<()> {
        let result = loop {
            let frame = match self.next() {
                Next::Frame(frame) => frame,
                Next::Idle => {
                    // `notify_one` stores a permit, so a frame queued meanwhile wakes us.
                    self.ready.notified().await;
                    continue;
                }
                Next::Done => break Ok(()),
            };
            let bytes = match encode(&frame) {
                Ok(bytes) => bytes,
                Err(error) => break Err(error),
            };
            match tokio::time::timeout(timeout, write.write_all(&bytes)).await {
                Ok(Ok(())) => self.written.notify_waiters(),
                Ok(Err(error)) => break Err(error),
                Err(_) => break Err(io::ErrorKind::TimedOut.into()),
            }
        };
        self.close();
        result
    }
}

/// The first stream after `last` with something to write, wrapping around to the lowest
/// id, so busy streams take turns.
fn next_ready(streams: &mut BTreeMap<u64, Stream>, last: u64) -> Option<(u64, &mut Stream)> {
    let mut wrapped = None;
    for (&id, stream) in streams {
        if stream.window.is_none() && stream.ended.is_none() {
            continue;
        }
        if id > last {
            return Some((id, stream));
        }
        if wrapped.is_none() {
            wrapped = Some((id, stream));
        }
    }
    wrapped
}

/// One line. An oversized reply fails only its own request, never the connection.
///
/// # Errors
/// Returns an error only when the replacement error frame cannot be serialized.
fn encode(frame: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(frame).unwrap_or_default();
    if bytes.is_empty() || bytes.len() > RESPONSE_LIMIT {
        let reason = "local control response exceeds limit";
        bytes = serde_json::to_vec(&json!({
            "id": frame["id"],
            "status": 500,
            "body": {"code": "control_request_failed", "parameters": {"reason": reason}},
        }))?;
    }
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests;
