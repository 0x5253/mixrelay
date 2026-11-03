//! Deliver supported native MIDI messages into the prepared host event sink.

use truce_rack_core::events::{Event as RackEvent, EventBody, EventList, MidiData};
use vst3::Steinberg::Vst::{Event, Event_::EventTypes_};

pub(super) fn valid(event: Event) -> bool {
    body(event).is_ok()
}

pub(super) fn forward(event: Event, output: &mut EventList) -> bool {
    let body = match body(event) {
        Ok(Some(body)) => body,
        Ok(None) => return true,
        Err(()) => return false,
    };
    let Ok(sample_offset) = u32::try_from(event.sampleOffset) else {
        return false;
    };
    output.try_push(RackEvent {
        sample_offset,
        body: EventBody::Midi(body),
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite normalized velocity is rounded within the u8 MIDI range 0..=127"
)]
fn velocity(value: f32) -> Option<u8> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return None;
    }
    // Native normalized velocity maps to MIDI 1.0's nearest integer value.
    Some((value * 127.0).round() as u8)
}

fn body(event: Event) -> Result<Option<MidiData>, ()> {
    if event.busIndex != 0 {
        return Err(());
    }
    let coordinates = |channel: i16, pitch: i16| {
        let channel = u8::try_from(channel)
            .ok()
            .filter(|value| *value < 16)
            .ok_or(())?;
        let pitch = u8::try_from(pitch)
            .ok()
            .filter(|value| *value < 128)
            .ok_or(())?;
        Ok::<_, ()>((channel, pitch))
    };
    // SAFETY: the event type selects the matching fixed-size union member. No
    // payload pointer is followed, including for unsupported native event types.
    let body = unsafe {
        match vst3::Steinberg::Vst::Event_::EventTypes::from(event.r#type) {
            EventTypes_::kNoteOnEvent => {
                let note = event.__field0.noteOn;
                let (channel, note_number) = coordinates(note.channel, note.pitch)?;
                MidiData::NoteOn {
                    channel,
                    note: note_number,
                    velocity: velocity(note.velocity).ok_or(())?,
                }
            }
            EventTypes_::kNoteOffEvent => {
                let note = event.__field0.noteOff;
                let (channel, note_number) = coordinates(note.channel, note.pitch)?;
                MidiData::NoteOff {
                    channel,
                    note: note_number,
                    velocity: velocity(note.velocity).ok_or(())?,
                }
            }
            EventTypes_::kPolyPressureEvent => {
                let note = event.__field0.polyPressure;
                let (channel, note_number) = coordinates(note.channel, note.pitch)?;
                MidiData::PolyAftertouch {
                    channel,
                    note: note_number,
                    pressure: velocity(note.pressure).ok_or(())?,
                }
            }
            _ => return Ok(None),
        }
    };
    Ok(Some(body))
}
