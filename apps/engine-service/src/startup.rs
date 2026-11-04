//! Startup steps both native services share.
use mixrelay_audio_io::session::AudioBackendKind;
use mixrelay_engine_core::session_settings;
use mixrelay_engine_host::session_preferences::SessionPreferences;
use std::sync::Arc;

/// The saved session format store for `backend`; a store that cannot be opened is passed on
/// as its reason, for the runtime status to report.
pub(crate) fn saved_session_preferences(
    backend: AudioBackendKind,
) -> Result<Arc<dyn session_settings::SessionPreferences>, String> {
    SessionPreferences::for_backend(backend)
        .map(|store| Arc::new(store) as Arc<dyn session_settings::SessionPreferences>)
        .map_err(|error| error.to_string())
}
