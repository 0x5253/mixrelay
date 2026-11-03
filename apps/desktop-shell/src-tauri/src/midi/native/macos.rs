//! Endpoint object identity detects reconnection even when the stable unique ID is reused.
pub(super) fn connection_id(index: usize, stable_id: &str) -> String {
    // SAFETY: MIDIGetSource accepts any index and returns zero if discovery changed.
    // midir enumerates CoreMIDI Sources in the same order; this value is only a
    // reconnection fence, never a pointer dereferenced by this module.
    let endpoint = unsafe { coremidi_sys::MIDIGetSource(u64::try_from(index).unwrap_or(u64::MAX)) };
    format!("{stable_id}:{endpoint}")
}
