//! Scripted capture completion for state transactions and editor-close fencing.

use super::StateCapture;
use crate::ProcessorStateError;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

type Completion = Box<dyn FnOnce() + Send>;
struct Reply {
    result: Result<Vec<u8>, ProcessorStateError>,
    completion: Option<Completion>,
}
#[derive(Default)]
struct State {
    replies: Mutex<VecDeque<Reply>>,
    calls: AtomicUsize,
}
#[derive(Clone, Default)]
pub(crate) struct MemoryCapture(Arc<State>);

impl MemoryCapture {
    pub(crate) fn reply(&self, result: Result<Vec<u8>, ProcessorStateError>) {
        self.0.replies.lock().unwrap().push_back(Reply {
            result,
            completion: None,
        });
    }
    pub(crate) fn reply_with(
        &self,
        result: Result<Vec<u8>, ProcessorStateError>,
        completion: impl FnOnce() + Send + 'static,
    ) {
        self.0.replies.lock().unwrap().push_back(Reply {
            result,
            completion: Some(Box::new(completion)),
        });
    }
    pub(crate) fn calls(&self) -> usize {
        self.0.calls.load(Ordering::Relaxed)
    }
}
impl StateCapture for MemoryCapture {
    fn snapshot(&self, _: Duration) -> Result<Vec<u8>, ProcessorStateError> {
        self.0.calls.fetch_add(1, Ordering::Relaxed);
        let reply = self
            .0
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("capture response must be scripted");
        if let Some(completion) = reply.completion {
            completion();
        }
        reply.result
    }
}
