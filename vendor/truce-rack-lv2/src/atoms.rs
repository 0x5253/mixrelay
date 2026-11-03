//! Bounded LV2 atom sequences and MIDI/transport conversion.

use crate::features::UridMap;
use lv2_raw::{
    atom::{
        LV2Atom, LV2AtomEvent, LV2AtomObjectBody, LV2AtomPropertyBody, LV2AtomSequence,
        LV2AtomSequenceBody,
    },
    urid::LV2Urid,
};
use std::ffi::CStr;
use truce_rack_core::{
    ProcessFault,
    error::{Error, Result},
    events::{Event, EventBody, EventList, MidiData},
    transport::TransportInfo,
};

pub(super) const LV2_ATOM_SEQUENCE_URI: &[u8] = b"http://lv2plug.in/ns/ext/atom#Sequence\0";
// Keep activation-time MIDI scratch storage under the processor host's existing 16 MiB bound.
pub(super) const MAX_LV2_EVENT_BUFFER_BYTES: usize = 16 * 1024 * 1024;
/// URIDs needed to build a `time:Position` object atom, mapped once
/// at load. The `atom_*` entries are the value types each property
/// carries; the rest are the `time:` properties themselves.
#[derive(Clone, Copy)]
struct TimeUrids {
    position: LV2Urid,
    frame: LV2Urid,
    speed: LV2Urid,
    bar: LV2Urid,
    bar_beat: LV2Urid,
    beats_per_bar: LV2Urid,
    beat_unit: LV2Urid,
    bpm: LV2Urid,
    atom_long: LV2Urid,
    atom_float: LV2Urid,
    atom_int: LV2Urid,
    atom_object: LV2Urid,
}

impl TimeUrids {
    /// Map transport-property URIs once, before the realtime path can use them.
    fn new(map: &UridMap) -> Self {
        // SAFETY: only process-lifetime NUL-terminated LV2 URI constants are passed below.
        let map_uri = |uri: &[u8]| map.map_uri(unsafe { CStr::from_ptr(uri.as_ptr().cast()) });
        Self {
            position: map_uri(lv2_raw::time::LV2_TIME__POSITION),
            frame: map_uri(lv2_raw::time::LV2_TIME__FRAME),
            speed: map_uri(lv2_raw::time::LV2_TIME__SPEED),
            bar: map_uri(lv2_raw::time::LV2_TIME__BAR),
            bar_beat: map_uri(lv2_raw::time::LV2_TIME__BARBEAT),
            beats_per_bar: map_uri(lv2_raw::time::LV2_TIME__BEATSPERBAR),
            beat_unit: map_uri(lv2_raw::time::LV2_TIME__BEATUNIT),
            bpm: map_uri(lv2_raw::time::LV2_TIME__BEATSPERMINUTE),
            atom_long: map_uri(lv2_raw::atom::LV2_ATOM__LONG),
            atom_float: map_uri(lv2_raw::atom::LV2_ATOM__FLOAT),
            atom_int: map_uri(lv2_raw::atom::LV2_ATOM__INT),
            atom_object: map_uri(lv2_raw::atom::LV2_ATOM__OBJECT),
        }
    }
}

/// Upper bound on the bytes one `time:Position` event consumes:
/// event + object header plus eight 8-byte-aligned properties. Used
/// to bounds-check once instead of per-property.
pub(super) const TIME_POSITION_MAX_BYTES: usize =
    std::mem::size_of::<LV2AtomEvent>() + std::mem::size_of::<LV2AtomObjectBody>() + 8 * 32;

/// Write one `time:` property (key + typed scalar value) into an
/// object body at `off`, returning the next 8-byte-aligned offset.
///
/// # Safety
/// `buf` must be 8-byte aligned; byte offset `off` must have room for the property header plus `value`
/// rounded up to 8 bytes; callers guarantee this via a single
/// up-front [`TIME_POSITION_MAX_BYTES`] check.
unsafe fn write_property(
    buf: *mut u64,
    off: usize,
    key: LV2Urid,
    value_type: LV2Urid,
    value: &[u8],
) -> usize {
    let prop = unsafe { &mut *buf.byte_add(off).cast::<LV2AtomPropertyBody>() };
    prop.key = key;
    prop.context = 0;
    prop.value = LV2Atom {
        size: u32::try_from(value.len()).unwrap_or(0),
        mytype: value_type,
    };
    let data = unsafe {
        buf.byte_add(off + std::mem::size_of::<LV2AtomPropertyBody>())
            .cast::<u8>()
    };
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), data, value.len()) };
    let total = std::mem::size_of::<LV2AtomPropertyBody>() + value.len();
    off + ((total + 7) & !7)
}

/// Write a `time:Position` object event (at frame 0) into the atom
/// sequence buffer starting at `base`, returning the offset just
/// past it. Returns `base` unchanged if there isn't room.
///
/// # Safety
/// `buf` must point at a buffer of at least `cap` bytes that is
/// valid for writes in `[base, cap)` and aligned for atom structs.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]
unsafe fn write_time_position(
    urids: &TimeUrids,
    buf: *mut u64,
    base: usize,
    cap: usize,
    t: &TransportInfo,
) -> usize {
    if base + TIME_POSITION_MAX_BYTES > cap {
        return base;
    }
    let event_header = std::mem::size_of::<LV2AtomEvent>();
    let obj_body_off = base + event_header;

    // Object body header (id = 0 blank, otype = time:Position).
    let obj_body = unsafe { &mut *buf.byte_add(obj_body_off).cast::<LV2AtomObjectBody>() };
    obj_body.id = 0;
    obj_body.otype = urids.position;

    let mut off = obj_body_off + std::mem::size_of::<LV2AtomObjectBody>();

    // speed: 1.0 while rolling, 0.0 when stopped.
    let speed: f32 = if t.playing { 1.0 } else { 0.0 };
    off = unsafe {
        write_property(
            buf,
            off,
            urids.speed,
            urids.atom_float,
            &speed.to_ne_bytes(),
        )
    };

    if let Some(samples) = t.song_position_samples {
        off = unsafe {
            write_property(
                buf,
                off,
                urids.frame,
                urids.atom_long,
                &samples.to_ne_bytes(),
            )
        };
    }
    if let Some(tempo) = t.tempo_bpm {
        let bpm = tempo as f32;
        off = unsafe { write_property(buf, off, urids.bpm, urids.atom_float, &bpm.to_ne_bytes()) };
    }
    off = unsafe { write_time_signature(urids, buf, off, t) };

    // Back-patch the event header now that the object size is known.
    let object_body_size = off - obj_body_off;
    let event = unsafe { &mut *buf.byte_add(base).cast::<LV2AtomEvent>() };
    event.time_in_frames = 0;
    event.body = LV2Atom {
        size: object_body_size as u32,
        mytype: urids.atom_object,
    };

    // Pad the whole event to the sequence's 8-byte event alignment.
    (off + 7) & !7
}

/// Append the meter and bar-position properties to a bounded time:Position atom.
///
/// # Safety
/// `buf` must retain the writable capacity preflighted by `write_time_position`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]
unsafe fn write_time_signature(
    urids: &TimeUrids,
    buf: *mut u64,
    mut off: usize,
    t: &TransportInfo,
) -> usize {
    let Some((num, den)) = t.time_signature else {
        return off;
    };
    let beats_per_bar = num as f32;
    off = unsafe {
        write_property(
            buf,
            off,
            urids.beats_per_bar,
            urids.atom_float,
            &beats_per_bar.to_ne_bytes(),
        )
    };
    let beat_unit = den as i32;
    off = unsafe {
        write_property(
            buf,
            off,
            urids.beat_unit,
            urids.atom_int,
            &beat_unit.to_ne_bytes(),
        )
    };

    // Bar / barBeat need the musical position too.
    let beats_per_bar_qn = f64::from(num) * 4.0 / f64::from(den.max(1));
    if let Some(bar_start) = t.bar_start_beats {
        let bar = (bar_start / beats_per_bar_qn.max(f64::EPSILON)).round() as i64;
        off = unsafe { write_property(buf, off, urids.bar, urids.atom_long, &bar.to_ne_bytes()) };

        if let Some(beats) = t.song_position_beats {
            // Quarter notes since the bar, expressed in this signature's beats.
            let bar_beat = ((beats - bar_start) * f64::from(den) / 4.0) as f32;
            off = unsafe {
                write_property(
                    buf,
                    off,
                    urids.bar_beat,
                    urids.atom_float,
                    &bar_beat.to_ne_bytes(),
                )
            };
        }
    }
    off
}

/// Encode one truce-rack MIDI event into the (status, d1, d2, len)
/// triple LV2 atom MIDI events expect. Returns `None` for
/// non-MIDI events or unsupported MIDI variants.
fn midi_bytes(body: &EventBody) -> Option<(u8, u8, u8, usize)> {
    match body {
        EventBody::Midi(MidiData::NoteOn {
            channel,
            note,
            velocity,
        }) => Some((0x90 | (channel & 0x0F), note & 0x7F, velocity & 0x7F, 3)),
        EventBody::Midi(MidiData::NoteOff {
            channel,
            note,
            velocity,
        }) => Some((0x80 | (channel & 0x0F), note & 0x7F, velocity & 0x7F, 3)),
        EventBody::Midi(MidiData::PolyAftertouch {
            channel,
            note,
            pressure,
        }) => Some((0xA0 | (channel & 0x0F), note & 0x7F, pressure & 0x7F, 3)),
        EventBody::Midi(MidiData::ControlChange {
            channel,
            controller,
            value,
        }) => Some((0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F, 3)),
        EventBody::Midi(MidiData::ProgramChange { channel, program }) => {
            Some((0xC0 | (channel & 0x0F), program & 0x7F, 0, 2))
        }
        EventBody::Midi(MidiData::ChannelAftertouch { channel, pressure }) => {
            Some((0xD0 | (channel & 0x0F), pressure & 0x7F, 0, 2))
        }
        EventBody::Midi(MidiData::PitchBend { channel, value }) => Some((
            0xE0 | (channel & 0x0F),
            (value & 0x7F) as u8,
            ((value >> 7) & 0x7F) as u8,
            3,
        )),
        EventBody::Midi(MidiData::Raw { len, data }) if *len >= 1 && *len <= 3 => Some((
            data[0],
            *data.get(1).unwrap_or(&0),
            *data.get(2).unwrap_or(&0),
            *len as usize,
        )),
        _ => None,
    }
}

pub(super) fn midi_buffer_capacity(max_block_size: usize) -> Result<usize> {
    if max_block_size == 0 || u32::try_from(max_block_size).is_err() {
        return Err(Error::Other("invalid LV2 maximum block size".into()));
    }
    let event_slot = std::mem::size_of::<LV2AtomEvent>()
        .checked_add(8)
        .ok_or_else(|| Error::Other("LV2 MIDI buffer size overflow".into()))?;
    let capacity = std::mem::size_of::<LV2AtomSequence>()
        .checked_add(TIME_POSITION_MAX_BYTES)
        .and_then(|base| max_block_size.checked_mul(event_slot)?.checked_add(base))
        .map(|capacity| capacity.max(64))
        .filter(|capacity| *capacity <= MAX_LV2_EVENT_BUFFER_BYTES)
        .ok_or_else(|| Error::Other("LV2 MIDI buffer exceeds the 16 MiB limit".into()))?;
    Ok(capacity)
}

/// Inverse of `midi_bytes` — turn 1-3 bytes of LV2 MIDI atom payload
/// into a typed `EventBody`. Anything longer (sysex etc.) is wrapped
/// in `MidiData::Raw` up to the 8-byte cap.
fn decode_midi_atom(bytes: &[u8]) -> Option<EventBody> {
    if bytes.is_empty() {
        return None;
    }
    let status = bytes[0];
    let channel = status & 0x0F;
    let kind = status & 0xF0;
    let body = match (kind, bytes) {
        (0x80, [_, note, vel]) => MidiData::NoteOff {
            channel,
            note: *note,
            velocity: *vel,
        },
        (0x90, [_, note, 0]) => MidiData::NoteOff {
            channel,
            note: *note,
            velocity: 0,
        },
        (0x90, [_, note, vel]) => MidiData::NoteOn {
            channel,
            note: *note,
            velocity: *vel,
        },
        (0xA0, [_, note, pressure]) => MidiData::PolyAftertouch {
            channel,
            note: *note,
            pressure: *pressure,
        },
        (0xB0, [_, controller, value]) => MidiData::ControlChange {
            channel,
            controller: *controller,
            value: *value,
        },
        (0xC0, [_, program]) => MidiData::ProgramChange {
            channel,
            program: *program,
        },
        (0xD0, [_, pressure]) => MidiData::ChannelAftertouch {
            channel,
            pressure: *pressure,
        },
        (0xE0, [_, lsb, msb]) => MidiData::PitchBend {
            channel,
            value: u16::from(*msb) << 7 | u16::from(*lsb),
        },
        _ if bytes.len() <= 8 => {
            let mut data = [0u8; 8];
            data[..bytes.len()].copy_from_slice(bytes);
            #[allow(clippy::cast_possible_truncation)]
            MidiData::Raw {
                len: bytes.len() as u8,
                data,
            }
        }
        _ => return None,
    };
    Some(EventBody::Midi(body))
}

/// Aligned storage for native atom headers and event timestamps.
#[derive(Default)]
struct AtomBuffer {
    words: Vec<u64>,
}

impl AtomBuffer {
    fn prepared(bytes: usize) -> Result<Self> {
        let count = bytes
            .checked_add(7)
            .map(|bytes| bytes / 8)
            .filter(|count| *count <= MAX_LV2_EVENT_BUFFER_BYTES / 8)
            .ok_or_else(|| {
                Error::Other("LV2 atom capacity exceeds the event buffer limit".into())
            })?;
        let mut words = Vec::new();
        words
            .try_reserve_exact(count)
            .map_err(|failure| Error::Other(format!("LV2 atom allocation failed: {failure}")))?;
        words.resize(count, 0);
        Ok(Self { words })
    }

    fn len(&self) -> usize {
        self.words.len() * 8
    }
    fn as_ptr(&self) -> *const u64 {
        self.words.as_ptr().cast()
    }
    fn as_mut_ptr(&mut self) -> *mut u64 {
        self.words.as_mut_ptr().cast()
    }
}

struct AtomOutput {
    index: u32,
    buffer: AtomBuffer,
    cursor: usize,
    end: usize,
    pending: Option<Event>,
}

/// The input and output sequences belong to one native instance and never alias.
pub(super) struct AtomBuffers {
    input: AtomBuffer,
    outputs: Vec<AtomOutput>,
    midi_event_urid: LV2Urid,
    sequence_urid: LV2Urid,
    chunk_urid: LV2Urid,
    frame_time_urid: LV2Urid,
    time_urids: TimeUrids,
}

impl AtomBuffers {
    pub(super) fn new(
        midi_event_urid: LV2Urid,
        sequence_urid: LV2Urid,
        map: &UridMap,
        output_ports: impl Iterator<Item = u32>,
    ) -> Self {
        // SAFETY: both constants are static NUL-terminated LV2 URI strings.
        let chunk_urid =
            map.map_uri(unsafe { CStr::from_ptr(lv2_raw::atom::LV2_ATOM__CHUNK.as_ptr().cast()) });
        let frame_time_urid = map
            .map_uri(unsafe { CStr::from_ptr(lv2_raw::atom::LV2_ATOM__FRAMETIME.as_ptr().cast()) });
        Self {
            input: AtomBuffer::default(),
            outputs: output_ports
                .map(|index| AtomOutput {
                    index,
                    buffer: AtomBuffer::default(),
                    cursor: 0,
                    end: 0,
                    pending: None,
                })
                .collect(),
            midi_event_urid,
            sequence_urid,
            chunk_urid,
            frame_time_urid,
            time_urids: TimeUrids::new(map),
        }
    }

    pub(super) fn prepare(&mut self, capacity: usize) -> Result<()> {
        if self.input.len() >= capacity
            && self
                .outputs
                .iter()
                .all(|output| output.buffer.len() >= capacity)
        {
            self.input.words.fill(0);
            for output in &mut self.outputs {
                output.buffer.words.fill(0);
            }
            return Ok(());
        }
        // All allocations finish before any currently connected pointer is replaced.
        let input = AtomBuffer::prepared(capacity)?;
        let mut outputs = Vec::new();
        outputs
            .try_reserve_exact(self.outputs.len())
            .map_err(|failure| {
                Error::Other(format!("LV2 atom port allocation failed: {failure}"))
            })?;
        for output in &self.outputs {
            outputs.push(AtomOutput {
                index: output.index,
                buffer: AtomBuffer::prepared(capacity)?,
                cursor: 0,
                end: 0,
                pending: None,
            });
        }
        self.input = input;
        self.outputs = outputs;
        Ok(())
    }

    pub(super) fn input_ptr(&mut self) -> *mut std::ffi::c_void {
        self.input.as_mut_ptr().cast()
    }
    pub(super) fn output_ptr(&mut self, port_index: u32) -> *mut std::ffi::c_void {
        self.outputs
            .iter_mut()
            .find(|output| output.index == port_index)
            .map_or(std::ptr::null_mut(), |output| {
                output.buffer.as_mut_ptr().cast()
            })
    }

    /// Advertise each output port's writable payload capacity before every native run.
    pub(super) fn prepare_output(&mut self) {
        for output in &mut self.outputs {
            if output.buffer.len() < std::mem::size_of::<LV2AtomSequence>() {
                continue;
            }
            let atom = LV2Atom {
                size: (output.buffer.len() - std::mem::size_of::<LV2Atom>()) as u32,
                mytype: self.chunk_urid,
            };
            // SAFETY: u64 backing storage guarantees alignment and the full header is in bounds.
            unsafe { output.buffer.as_mut_ptr().cast::<LV2Atom>().write(atom) };
        }
    }

    pub(super) fn build_sequence(
        &mut self,
        events: &EventList,
        transport: Option<TransportInfo>,
    ) -> std::result::Result<(), AtomFault> {
        let header_size = std::mem::size_of::<LV2AtomSequence>();
        let cap = self.input.len();
        if cap < header_size {
            return Err(AtomFault::InputCapacity);
        }
        let buf = self.input.as_mut_ptr();

        // Header: capacity-bounded sequence whose `atom.size` only
        // counts the body + events (the LV2 atom convention).
        let body_size = std::mem::size_of::<LV2AtomSequenceBody>();
        let seq = unsafe { &mut *buf.cast::<LV2AtomSequence>() };
        seq.atom = LV2Atom {
            size: body_size as u32,
            mytype: self.sequence_urid,
        };
        seq.body = LV2AtomSequenceBody { unit: 0, pad: 0 };

        let mut write_off = header_size;

        // Inject a `time:Position` object as the first event (frame
        // 0) so transport-aware plugins see tempo / grid before any
        // MIDI. MIDI events follow, also at frame >= 0.
        if let Some(t) = transport {
            write_off = unsafe { write_time_position(&self.time_urids, buf, write_off, cap, &t) };
        }

        let event_header = std::mem::size_of::<LV2AtomEvent>();
        for ev in events {
            let Some((status, d1, d2, len)) = midi_bytes(&ev.body) else {
                continue;
            };
            let total = event_header + len;
            let padded = (total + 7) & !7; // 8-byte alignment per atom spec
            if write_off + padded > cap {
                return Err(AtomFault::InputCapacity);
            }
            let event_ptr = unsafe { buf.byte_add(write_off) }.cast::<LV2AtomEvent>();
            unsafe {
                (*event_ptr).time_in_frames = i64::from(ev.sample_offset);
                (*event_ptr).body = LV2Atom {
                    size: len as u32,
                    mytype: self.midi_event_urid,
                };
                let data_ptr = (event_ptr as *mut u8).add(event_header);
                if len >= 1 {
                    *data_ptr = status;
                }
                if len >= 2 {
                    *data_ptr.add(1) = d1;
                }
                if len >= 3 {
                    *data_ptr.add(2) = d2;
                }
            }
            write_off += padded;
        }
        // sequence body size is everything past the atom header
        seq.atom.size = (write_off - std::mem::size_of::<LV2Atom>()) as u32;
        Ok(())
    }

    /// Merge the native output ports by frame without growing the prepared destination.
    pub(super) fn drain_output(
        &mut self,
        events: &mut EventList,
        frames: u32,
    ) -> std::result::Result<(), AtomFault> {
        for output in &mut self.outputs {
            let sequence = SequenceEvents::new(
                &output.buffer,
                self.sequence_urid,
                self.chunk_urid,
                self.frame_time_urid,
            )?;
            output.cursor = sequence.as_ref().map_or(0, |sequence| sequence.cursor);
            output.end = sequence.as_ref().map_or(0, |sequence| sequence.end);
            output.pending = output.next_midi(self.midi_event_urid, frames)?;
        }
        while let Some((index, event)) = self
            .outputs
            .iter()
            .enumerate()
            .filter_map(|(index, output)| output.pending.map(|event| (index, event)))
            .min_by_key(|(_, event)| event.sample_offset)
        {
            if !events.try_push(event) {
                return Err(AtomFault::OutputCapacity);
            }
            self.outputs[index].pending =
                self.outputs[index].next_midi(self.midi_event_urid, frames)?;
        }
        Ok(())
    }
}

impl AtomOutput {
    fn next_midi(
        &mut self,
        midi_urid: LV2Urid,
        frames: u32,
    ) -> std::result::Result<Option<Event>, AtomFault> {
        let mut sequence = SequenceEvents {
            buffer: &self.buffer,
            cursor: self.cursor,
            end: self.end,
        };
        while let Some(event) = sequence.next_event()? {
            let offset = u32::try_from(event.time_in_frames)
                .ok()
                .filter(|offset| *offset < frames)
                .ok_or(AtomFault::InvalidEvent)?;
            if event.body.mytype == midi_urid
                && let Some(body) = decode_midi_atom(SequenceEvents::payload(event))
            {
                self.cursor = sequence.cursor;
                return Ok(Some(Event {
                    sample_offset: offset,
                    body,
                }));
            }
        }
        self.cursor = sequence.cursor;
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AtomFault {
    InputCapacity,
    InvalidSequence,
    InvalidEvent,
    OutputCapacity,
}

impl AtomFault {
    pub(super) fn into_error(self) -> Error {
        let fault = match self {
            Self::InputCapacity => ProcessFault::InputEventCapacity,
            Self::InvalidSequence | Self::InvalidEvent => ProcessFault::InvalidOutputEvent,
            Self::OutputCapacity => ProcessFault::OutputEventCapacity,
        };
        Error::Process {
            format: crate::FORMAT,
            fault,
        }
    }
}

/// A checked cursor over one instance's output storage; no native size can widen its bounds.
struct SequenceEvents<'a> {
    buffer: &'a AtomBuffer,
    cursor: usize,
    end: usize,
}

impl<'a> SequenceEvents<'a> {
    fn new(
        buffer: &'a AtomBuffer,
        sequence_urid: LV2Urid,
        chunk_urid: LV2Urid,
        frame_time_urid: LV2Urid,
    ) -> std::result::Result<Option<Self>, AtomFault> {
        if buffer.len() < std::mem::size_of::<LV2AtomSequence>() {
            return Err(AtomFault::InvalidSequence);
        }
        // SAFETY: the aligned allocation contains a complete header; its native extent
        // and type are checked before any event or payload is inspected.
        let sequence = unsafe { &*buffer.as_ptr().cast::<LV2AtomSequence>() };
        if sequence.atom.mytype == chunk_urid {
            return Ok(None);
        }
        if sequence.atom.mytype != sequence_urid
            || (sequence.body.unit != 0 && sequence.body.unit != frame_time_urid)
        {
            return Err(AtomFault::InvalidSequence);
        }
        let end = std::mem::size_of::<LV2Atom>() + sequence.atom.size as usize;
        if end < std::mem::size_of::<LV2AtomSequence>() || end > buffer.len() {
            return Err(AtomFault::InvalidSequence);
        }
        Ok(Some(Self {
            buffer,
            cursor: std::mem::size_of::<LV2AtomSequence>(),
            end,
        }))
    }

    fn next_event(&mut self) -> std::result::Result<Option<&'a LV2AtomEvent>, AtomFault> {
        if self.cursor == self.end {
            return Ok(None);
        }
        let header = std::mem::size_of::<LV2AtomEvent>();
        if self.end - self.cursor < header {
            return Err(AtomFault::InvalidEvent);
        }
        // SAFETY: every cursor is advanced by an 8-byte padded event length, so this complete
        // header remains aligned within the already validated sequence and allocation extents.
        let event = unsafe {
            &*self
                .buffer
                .as_ptr()
                .byte_add(self.cursor)
                .cast::<LV2AtomEvent>()
        };
        let length = header + event.body.size as usize;
        let padded = length
            .checked_add(7)
            .map(|length| length & !7)
            .ok_or(AtomFault::InvalidEvent)?;
        if padded > self.end - self.cursor {
            return Err(AtomFault::InvalidEvent);
        }
        self.cursor += padded;
        Ok(Some(event))
    }

    fn payload(event: &LV2AtomEvent) -> &[u8] {
        // SAFETY: next_event accepted the complete header, payload, and alignment padding
        // within this buffer before returning the borrowed event.
        unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(event)
                    .cast::<u8>()
                    .add(std::mem::size_of::<LV2AtomEvent>()),
                event.body.size as usize,
            )
        }
    }
}

#[cfg(test)]
#[path = "atoms/tests.rs"]
mod tests;
