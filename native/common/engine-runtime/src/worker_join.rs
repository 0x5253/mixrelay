//! Control-thread joins retain worker identity and panic reason.

use std::{any::Any, thread::JoinHandle};

/// Joins `worker` off the audio thread, releasing its owners and returning its value.
///
/// # Errors
/// Logs and returns the worker name and panic reason if the thread panics.
pub fn join<T>(worker: JoinHandle<T>) -> Result<T, String> {
    let name = worker
        .thread()
        .name()
        .unwrap_or("unnamed worker")
        .to_owned();
    worker.join().map_err(|payload| {
        let reason = panic_reason(payload.as_ref());
        let error = format!("{name} panicked: {reason}");
        tracing::error!(worker = name, reason, "MixRelay worker panicked");
        error
    })
}

fn panic_reason(payload: &(dyn Any + Send)) -> &str {
    if let Some(message) = payload.downcast_ref::<String>() {
        message
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else {
        "non-string panic payload"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_reports_the_named_worker_and_owned_panic_reason() {
        let worker = std::thread::Builder::new()
            .name("native-retirement-test".into())
            .spawn(|| std::panic::panic_any(String::from("injected failure")))
            .unwrap();
        assert_eq!(
            join(worker),
            Err("native-retirement-test panicked: injected failure".into())
        );
    }

    #[test]
    fn panic_reason_handles_literal_and_opaque_payloads() {
        assert_eq!(panic_reason(&"literal failure"), "literal failure");
        assert_eq!(panic_reason(&42_u32), "non-string panic payload");
    }

    #[test]
    fn successful_join_is_not_a_shutdown_error() {
        assert_eq!(join(std::thread::spawn(|| 42_u32)), Ok(42));
    }
}
