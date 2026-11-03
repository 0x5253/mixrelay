//! Routes system stop requests through acknowledged application shutdown.

/// Exec-launched helpers may receive SIGTERM during package replacement or removal.
/// Do not perform native control work inside a signal handler: route it through
/// the same acknowledged lifecycle on the application thread.
pub(crate) fn observe_background_stop(app: &tauri::AppHandle) -> std::io::Result<()> {
    let mut terminate = background_stop_signal()?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while terminate.recv().await.is_some() {
            let target = app.clone();
            if let Err(error) = app.run_on_main_thread(move || target.exit(0)) {
                tracing::error!(%error, "could not dispatch system background stop");
            }
        }
    });
    Ok(())
}

fn background_stop_signal() -> std::io::Result<tokio::signal::unix::Signal> {
    // Desktop setup runs outside Tokio. Registration needs the same reactor
    // that will poll the signal, before setup admits any background work.
    let runtime = tauri::async_runtime::handle();
    let _entered = runtime.inner().enter();
    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
}

#[cfg(test)]
mod tests {
    #[test]
    fn signal_registration_uses_the_tauri_reactor_outside_a_tokio_context() {
        assert!(tokio::runtime::Handle::try_current().is_err());
        let signal = super::background_stop_signal().unwrap();
        drop(signal);
        assert!(tokio::runtime::Handle::try_current().is_err());
    }
}
