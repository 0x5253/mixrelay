//! Prepared input events, transport translation, and validated output admission.

use crate::parameter_ids::ParameterIds;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE,
    CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
    CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_LOOP_ACTIVE, CLAP_TRANSPORT_IS_PLAYING,
    CLAP_TRANSPORT_IS_RECORDING, clap_event_header, clap_event_midi, clap_event_note,
    clap_event_param_value, clap_event_transport, clap_input_events, clap_output_events,
    clap_transport_flags,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::process::{
    CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_SLEEP,
    CLAP_PROCESS_TAIL,
};
use std::ptr;
use truce_rack_core::error::{Error, Result};
use truce_rack_core::events::EventList;
use truce_rack_core::plugin::ProcessStatus;
use truce_rack_core::transport::TransportInfo;

/// Translate a host [`TransportInfo`] snapshot into CLAP's
/// `clap_event_transport`. `sample_rate` converts the sample
/// position into the seconds timeline CLAP also wants.
///
/// CLAP beat / second times are 31.32 fixed-point (see
/// `CLAP_BEATTIME_FACTOR`); beats are quarter notes.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
pub(super) fn build_clap_transport(t: &TransportInfo, sample_rate: f64) -> clap_event_transport {
    let beats_to_fixed = |b: f64| (b * CLAP_BEATTIME_FACTOR as f64).round() as i64;
    let secs_to_fixed = |s: f64| (s * CLAP_SECTIME_FACTOR as f64).round() as i64;

    let mut flags: clap_transport_flags = 0;
    let tempo = t.tempo_bpm.unwrap_or(0.0);
    if t.tempo_bpm.is_some() {
        flags |= CLAP_TRANSPORT_HAS_TEMPO;
    }
    let song_pos_beats = match t.song_position_beats {
        Some(b) => {
            flags |= CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
            beats_to_fixed(b)
        }
        None => 0,
    };
    let song_pos_seconds = match t.song_position_samples {
        Some(s) => {
            flags |= CLAP_TRANSPORT_HAS_SECONDS_TIMELINE;
            secs_to_fixed(s as f64 / sample_rate.max(1.0))
        }
        None => 0,
    };
    let (tsig_num, tsig_denom) = match t.time_signature {
        Some((n, d)) => {
            flags |= CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
            (n as u16, d as u16)
        }
        None => (0, 0),
    };
    let bar_start = t.bar_start_beats.map_or(0, beats_to_fixed);
    // Bar index: bar_start (in quarter notes) divided by the bar
    // length the time signature implies.
    let bar_number = match (t.bar_start_beats, t.time_signature) {
        (Some(bsb), Some((n, d))) => {
            let beats_per_bar = f64::from(n) * 4.0 / f64::from(d.max(1));
            (bsb / beats_per_bar.max(f64::EPSILON)).round() as i32
        }
        _ => 0,
    };
    if t.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    if t.recording {
        flags |= CLAP_TRANSPORT_IS_RECORDING;
    }
    if t.loop_active {
        flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
    }

    clap_event_transport {
        header: clap_event_header {
            size: u32::try_from(std::mem::size_of::<clap_event_transport>()).unwrap_or(0),
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags,
        song_pos_beats,
        song_pos_seconds,
        tempo,
        tempo_inc: 0.0,
        loop_start_beats: 0,
        loop_end_beats: 0,
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        bar_start,
        bar_number,
        tsig_num,
        tsig_denom,
    }
}

pub(super) fn map_clap_status(status: i32) -> ProcessStatus {
    match status {
        CLAP_PROCESS_CONTINUE | CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => ProcessStatus::Continue,
        CLAP_PROCESS_SLEEP => ProcessStatus::Sleep,
        CLAP_PROCESS_TAIL => ProcessStatus::Tail { tail_samples: 0 },
        _ => ProcessStatus::Error,
    }
}

pub(super) fn make_param_value_event(
    sample_offset: u32,
    param_id: u32,
    value: f64,
) -> clap_event_param_value {
    clap_event_param_value {
        header: clap_event_header {
            size: u32::try_from(std::mem::size_of::<clap_event_param_value>()).unwrap_or(0),
            time: sample_offset,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id,
        cookie: ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

pub(super) fn make_note_event(
    sample_offset: u32,
    event_type: u16,
    channel: u8,
    key: u8,
    velocity: f64,
) -> clap_event_note {
    clap_event_note {
        header: clap_event_header {
            size: u32::try_from(std::mem::size_of::<clap_event_note>()).unwrap_or(0),
            time: sample_offset,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: event_type,
            flags: 0,
        },
        note_id: -1,
        port_index: -1,
        channel: i16::from(channel),
        key: i16::from(key),
        velocity,
    }
}

pub(super) fn make_midi_event(sample_offset: u32, bytes: [u8; 3]) -> clap_event_midi {
    clap_event_midi {
        header: clap_event_header {
            size: u32::try_from(std::mem::size_of::<clap_event_midi>()).unwrap_or(0),
            time: sample_offset,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_MIDI,
            flags: 0,
        },
        port_index: 0,
        data: bytes,
    }
}

/// Owned storage for one block's CLAP-formatted input events.
///
/// We can't ship `*const clap_event_header` directly off our
/// `EventList` because the truce-rack event payloads have different
/// layouts than CLAP's. The fix is to translate up front and keep
/// the result alive for the `process` call.
pub(super) struct ConvertedInputEvents {
    /// Owned event memory — every entry is a CLAP event struct
    /// the input-events vtable hands out pointers into. Prepared storage remains stable
    /// until the synchronous native process call returns.
    pub(super) events: Vec<EventStorage>,
}

pub(super) enum EventStorage {
    Param(clap_event_param_value),
    Note(clap_event_note),
    Midi(clap_event_midi),
}

impl EventStorage {
    pub(super) fn header(&self) -> *const clap_event_header {
        match self {
            Self::Param(e) => &raw const e.header,
            Self::Note(e) => &raw const e.header,
            Self::Midi(e) => &raw const e.header,
        }
    }
}

pub(super) fn convert_midi_event(
    offset: u32,
    midi: truce_rack_core::events::MidiData,
) -> Option<EventStorage> {
    use truce_rack_core::events::MidiData;

    Some(match midi {
        MidiData::NoteOn {
            channel,
            note,
            velocity,
        } => EventStorage::Note(make_note_event(
            offset,
            CLAP_EVENT_NOTE_ON,
            channel,
            note,
            f64::from(velocity) / 127.0,
        )),
        MidiData::NoteOff {
            channel,
            note,
            velocity,
        } => EventStorage::Note(make_note_event(
            offset,
            CLAP_EVENT_NOTE_OFF,
            channel,
            note,
            f64::from(velocity) / 127.0,
        )),
        MidiData::ControlChange {
            channel,
            controller,
            value,
        } => {
            let status = 0xB0 | (channel & 0x0F);
            EventStorage::Midi(make_midi_event(
                offset,
                [status, controller & 0x7F, value & 0x7F],
            ))
        }
        MidiData::ProgramChange { channel, program } => {
            let status = 0xC0 | (channel & 0x0F);
            EventStorage::Midi(make_midi_event(offset, [status, program & 0x7F, 0]))
        }
        MidiData::PolyAftertouch {
            channel,
            note,
            pressure,
        } => {
            let status = 0xA0 | (channel & 0x0F);
            EventStorage::Midi(make_midi_event(
                offset,
                [status, note & 0x7F, pressure & 0x7F],
            ))
        }
        MidiData::ChannelAftertouch { channel, pressure } => {
            let status = 0xD0 | (channel & 0x0F);
            EventStorage::Midi(make_midi_event(offset, [status, pressure & 0x7F, 0]))
        }
        MidiData::PitchBend { channel, value } => {
            let status = 0xE0 | (channel & 0x0F);
            let lsb = u8::try_from(value & 0x7F).unwrap_or(0);
            let msb = u8::try_from((value >> 7) & 0x7F).unwrap_or(0);
            EventStorage::Midi(make_midi_event(offset, [status, lsb, msb]))
        }
        MidiData::Raw { len, data } if len >= 3 => {
            EventStorage::Midi(make_midi_event(offset, [data[0], data[1], data[2]]))
        }
        MidiData::Raw { .. } => return None,
    })
}

// Host event bursts are bounded independently of the complete pending-parameter cache.
// Storage is reserved during activation; oversized blocks fail before native processing begins.
pub(super) const MAX_HOST_EVENTS: usize = 4096;

impl ConvertedInputEvents {
    pub(super) fn for_process(parameter_count: usize) -> Result<Self> {
        let capacity = parameter_count
            .checked_add(MAX_HOST_EVENTS)
            .ok_or_else(|| Error::Other("CLAP input event capacity overflow".into()))?;
        let mut events = Vec::new();
        events
            .try_reserve_exact(capacity)
            .map_err(|error| Error::Other(format!("cannot reserve CLAP input events: {error}")))?;
        Ok(Self { events })
    }

    pub(super) fn fill_for_process(
        &mut self,
        list: &EventList,
        parameter_ids: &ParameterIds,
    ) -> bool {
        self.events.clear();
        let count = parameter_ids.pending_count().checked_add(list.len());
        if list.len() > MAX_HOST_EVENTS || count.is_none_or(|count| count > self.events.capacity())
        {
            return false;
        }
        parameter_ids.for_each_pending(|param_id, value| self.push_param_value(0, param_id, value));
        for event in list {
            self.push_rack(event);
        }
        true
    }

    pub(super) fn push_param_value(&mut self, sample_offset: u32, param_id: u32, value: f64) {
        self.events.push(EventStorage::Param(make_param_value_event(
            sample_offset,
            param_id,
            value,
        )));
    }

    pub(super) fn push_rack(&mut self, event: &truce_rack_core::events::Event) {
        use truce_rack_core::events::EventBody;
        let offset = event.sample_offset;
        match event.body {
            EventBody::Midi(midi) => self.push_midi(offset, midi),
            EventBody::ParamValue { param_id, value } => {
                self.push_param_value(offset, param_id, value);
            }
            EventBody::ParamGesture { .. } | EventBody::TransportFlag(_) => {
                // ParamGesture: CLAP's begin/end events have
                // separate types; covered in a follow-on once
                // hosts start emitting them.
                // TransportFlag: routed through clap_event_transport
                // on the process struct, not via input events.
            }
        }
    }

    pub(super) fn push_midi(&mut self, offset: u32, midi: truce_rack_core::events::MidiData) {
        if let Some(event) = convert_midi_event(offset, midi) {
            self.events.push(event);
        }
    }

    pub(super) fn as_clap(&self) -> clap_input_events {
        clap_input_events {
            ctx: std::ptr::from_ref::<Self>(self)
                .cast::<std::ffi::c_void>()
                .cast_mut(),
            size: Some(input_events_size),
            get: Some(input_events_get),
        }
    }
}

pub(super) unsafe extern "C" fn input_events_size(list: *const clap_input_events) -> u32 {
    let ctx = unsafe { (*list).ctx.cast::<ConvertedInputEvents>() };
    if ctx.is_null() {
        return 0;
    }
    u32::try_from(unsafe { (*ctx).events.len() }).unwrap_or(u32::MAX)
}

pub(super) unsafe extern "C" fn input_events_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let ctx = unsafe { (*list).ctx.cast::<ConvertedInputEvents>() };
    if ctx.is_null() {
        return ptr::null();
    }
    let idx = index as usize;
    let events = unsafe { &(*ctx).events };
    events.get(idx).map_or(ptr::null(), EventStorage::header)
}

/// Sink for events the plugin emits during `process`.
pub(super) struct OutputEventsSink<'a> {
    pub(super) target: Option<&'a mut EventList>,
}

impl<'a> OutputEventsSink<'a> {
    pub(super) fn new(target: Option<&'a mut EventList>) -> Self {
        Self { target }
    }

    pub(super) fn as_clap(&mut self) -> clap_output_events {
        clap_output_events {
            ctx: std::ptr::from_mut::<Self>(self).cast::<std::ffi::c_void>(),
            try_push: Some(output_events_try_push),
        }
    }
}

pub(super) unsafe extern "C" fn output_events_try_push(
    list: *const clap_output_events,
    event: *const clap_event_header,
) -> bool {
    if list.is_null() || event.is_null() || !event.is_aligned() {
        return false;
    }
    // SAFETY: CLAP requires a readable event header for this synchronous callback.
    let header = unsafe { &*event };
    if !valid_output_event(header) {
        return false;
    }
    let ctx = unsafe { (*list).ctx.cast::<OutputEventsSink<'_>>() };
    if ctx.is_null() {
        return true;
    }
    let target = unsafe { (*ctx).target.as_deref_mut() };
    let Some(target) = target else {
        // Plugin wanted to send an event; the host doesn't care
        // (Sink was constructed with `target: None`). Returning
        // true tells the plugin we accepted it.
        return true;
    };
    if let Some(rack_event) = unsafe { clap_event_to_rack(header) } {
        return target.try_push(rack_event);
    }
    false
}

pub(super) fn valid_output_event(header: &clap_event_header) -> bool {
    let size = match header.type_ {
        CLAP_EVENT_PARAM_VALUE => std::mem::size_of::<clap_event_param_value>(),
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => std::mem::size_of::<clap_event_note>(),
        CLAP_EVENT_MIDI => std::mem::size_of::<clap_event_midi>(),
        _ => return false,
    };
    header.space_id == CLAP_CORE_EVENT_SPACE_ID
        && usize::try_from(header.size).is_ok_and(|declared| declared >= size)
}

pub(super) unsafe fn clap_event_to_rack(
    header: &clap_event_header,
) -> Option<truce_rack_core::events::Event> {
    use truce_rack_core::events::{Event, EventBody, MidiData};
    if !valid_output_event(header) {
        return None;
    }
    let offset = header.time;
    match header.type_ {
        t if t == CLAP_EVENT_PARAM_VALUE => {
            // SAFETY: the size check covers this payload. Unaligned copying also accepts a
            // header-aligned record without creating a more strictly aligned reference.
            let e: clap_event_param_value =
                unsafe { std::ptr::read_unaligned(std::ptr::from_ref(header).cast()) };
            Some(Event {
                sample_offset: offset,
                body: EventBody::ParamValue {
                    param_id: e.param_id,
                    value: e.value,
                },
            })
        }
        t if t == CLAP_EVENT_NOTE_ON => {
            // SAFETY: the validated record declares a complete note payload.
            let e: clap_event_note =
                unsafe { std::ptr::read_unaligned(std::ptr::from_ref(header).cast()) };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let velocity = (e.velocity * 127.0).round().clamp(0.0, 127.0) as u8;
            Some(Event {
                sample_offset: offset,
                body: EventBody::Midi(MidiData::NoteOn {
                    channel: u8::try_from(e.channel.max(0)).unwrap_or(0),
                    note: u8::try_from(e.key.max(0)).unwrap_or(0),
                    velocity,
                }),
            })
        }
        t if t == CLAP_EVENT_NOTE_OFF => {
            // SAFETY: the validated record declares a complete note payload.
            let e: clap_event_note =
                unsafe { std::ptr::read_unaligned(std::ptr::from_ref(header).cast()) };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let velocity = (e.velocity * 127.0).round().clamp(0.0, 127.0) as u8;
            Some(Event {
                sample_offset: offset,
                body: EventBody::Midi(MidiData::NoteOff {
                    channel: u8::try_from(e.channel.max(0)).unwrap_or(0),
                    note: u8::try_from(e.key.max(0)).unwrap_or(0),
                    velocity,
                }),
            })
        }
        t if t == CLAP_EVENT_MIDI => {
            // SAFETY: the validated record declares a complete MIDI payload.
            let e: clap_event_midi =
                unsafe { std::ptr::read_unaligned(std::ptr::from_ref(header).cast()) };
            Some(Event {
                sample_offset: offset,
                body: EventBody::Midi(MidiData::Raw {
                    len: 3,
                    data: [e.data[0], e.data[1], e.data[2], 0, 0, 0, 0, 0],
                }),
            })
        }
        _ => None,
    }
}
