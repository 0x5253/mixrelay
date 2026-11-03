//! Translate supported MIDI messages without following payload pointers.

use truce_rack_core::events::{Event as RackEvent, EventBody, MidiData};
use vst3::Steinberg::Vst::{
    Event, Event_::EventTypes_, Event__type0, NoteOffEvent, NoteOnEvent, PolyPressureEvent,
};

fn valid(channel: u8, note: u8, value: u8) -> Result<(), ()> {
    if channel < 16 && note < 128 && value < 128 {
        Ok(())
    } else {
        Err(())
    }
}

fn header(kind: vst3::Steinberg::Vst::Event_::EventTypes, offset: i32) -> Event {
    Event {
        busIndex: 0,
        sampleOffset: offset,
        ppqPosition: 0.0,
        flags: 0,
        r#type: u16::try_from(kind).unwrap_or_default(),
        __field0: Event__type0 {
            noteOn: NoteOnEvent {
                channel: 0,
                pitch: 0,
                tuning: 0.0,
                velocity: 0.0,
                length: 0,
                noteId: -1,
            },
        },
    }
}

pub(super) fn translate(event: &RackEvent, frames: usize) -> Result<Option<Event>, ()> {
    let EventBody::Midi(body) = event.body else {
        return Ok(None);
    };
    let offset = i32::try_from(event.sample_offset).map_err(|_| ())?;
    let mut native = match body {
        MidiData::NoteOn {
            channel,
            note,
            velocity,
        } => {
            valid(channel, note, velocity)?;
            let mut native = header(EventTypes_::kNoteOnEvent, offset);
            native.__field0 = Event__type0 {
                noteOn: NoteOnEvent {
                    channel: i16::from(channel),
                    pitch: i16::from(note),
                    tuning: 0.0,
                    velocity: f32::from(velocity) / 127.0,
                    length: 0,
                    noteId: -1,
                },
            };
            native
        }
        MidiData::NoteOff {
            channel,
            note,
            velocity,
        } => {
            valid(channel, note, velocity)?;
            let mut native = header(EventTypes_::kNoteOffEvent, offset);
            native.__field0 = Event__type0 {
                noteOff: NoteOffEvent {
                    channel: i16::from(channel),
                    pitch: i16::from(note),
                    tuning: 0.0,
                    velocity: f32::from(velocity) / 127.0,
                    noteId: -1,
                },
            };
            native
        }
        MidiData::PolyAftertouch {
            channel,
            note,
            pressure,
        } => {
            valid(channel, note, pressure)?;
            let mut native = header(EventTypes_::kPolyPressureEvent, offset);
            native.__field0 = Event__type0 {
                polyPressure: PolyPressureEvent {
                    channel: i16::from(channel),
                    pitch: i16::from(note),
                    pressure: f32::from(pressure) / 127.0,
                    noteId: -1,
                },
            };
            native
        }
        // Other MIDI messages require the optional IMidiMapping contract.
        _ => return Ok(None),
    };
    if usize::try_from(offset).map_err(|_| ())? >= frames {
        return Err(());
    }
    native.sampleOffset = offset;
    Ok(Some(native))
}
