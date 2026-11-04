//! Device properties answered from the control snapshot, plus the identity
//! properties every device-owned object shares.
use super::{
    Address, GLOBAL, INPUT, OUTPUT,
    status::UNKNOWN,
    value::{Inline, Value, code, number, objects, pair, rate, text},
};
use crate::abi::Snapshot;

/// Class, owner and name properties shared by a device and its streams.
pub(super) fn common(snapshot: &Snapshot, object: u32, selector: u32) -> Option<Value> {
    let device = object == snapshot.device_id;
    match &selector.to_be_bytes() {
        b"bcls" => Some(code(*b"aobj")),
        b"clas" => Some(code(if device { *b"adev" } else { *b"astr" })),
        b"stdv" => Some(number(if device { 1 } else { snapshot.device_id })),
        b"lnam" => Some(text(&snapshot.name)),
        b"lmak" => Some(text(b"MixRelay")),
        _ => None,
    }
}

pub(super) fn property(snapshot: &Snapshot, address: &Address) -> Result<Value, i32> {
    let directional = address.scope == INPUT || address.scope == OUTPUT;
    let channels = if address.scope == INPUT {
        snapshot.capture_channels
    } else {
        snapshot.render_channels
    };
    match &address.selector.to_be_bytes() {
        b"ownd" => Ok(objects(
            stream_ids(snapshot, address.scope)
                .into_iter()
                .chain(control_ids(snapshot, address.scope)),
        )),
        b"stm#" => Ok(objects(stream_ids(snapshot, address.scope))),
        b"ctrl" => Ok(objects(control_ids(snapshot, address.scope))),
        b"uid " => Ok(text(&snapshot.uid)),
        b"muid" => Ok(text(b"io.mixrelay.audio.endpoint")),
        b"akin" => Ok(objects([snapshot.device_id])),
        b"clkd" | b"hidn" => Ok(number(0)),
        b"livn" => Ok(number(u32::from(snapshot.alive))),
        b"goin" => Ok(number(u32::from(snapshot.running))),
        b"dflt" | b"sflt" if directional => Ok(number(u32::from(channels > 0))),
        b"ltnc" | b"saft" if directional => Ok(number(0)),
        b"nsrt" if address.scope == GLOBAL => Ok(rate(snapshot.sample_rate)),
        b"nsr#" if address.scope == GLOBAL => Ok(rate_ranges()),
        b"cust" if address.scope == GLOBAL => Ok(custom_properties()),
        b"dch2" if directional && channels >= 2 => Ok(Value::Inline(Inline::new(pair::<4, 8>(
            1_u32.to_ne_bytes(),
            2_u32.to_ne_bytes(),
        )))),
        b"srnd" if directional => Ok(channel_layout(channels)),
        _ => Err(UNKNOWN),
    }
}

fn stream_ids(snapshot: &Snapshot, scope: u32) -> Vec<u32> {
    let mut result = Vec::with_capacity(2);
    if scope != OUTPUT && snapshot.capture_channels > 0 {
        result.push(snapshot.input_stream_id);
    }
    if scope != INPUT && snapshot.render_channels > 0 {
        result.push(snapshot.output_stream_id);
    }
    result
}

/// The volume and mute controls exist only for a device that renders.
fn control_ids(snapshot: &Snapshot, scope: u32) -> Vec<u32> {
    if scope != INPUT && snapshot.render_channels != 0 {
        vec![snapshot.device_id + 3, snapshot.device_id + 4]
    } else {
        Vec::new()
    }
}

/// Each supported rate as a degenerate `AudioValueRange`.
fn rate_ranges() -> Value {
    Value::Array(
        crate::formats::SAMPLE_RATES
            .iter()
            .flat_map(|rate| {
                let bytes = f64::from(*rate).to_ne_bytes();
                bytes.into_iter().chain(bytes)
            })
            .collect(),
        16,
    )
}

/// The `AudioServerPlugInCustomPropertyInfo` entries for the IO diagnostics.
fn custom_properties() -> Value {
    Value::Array(
        [*b"mrdi", *b"mrdo"]
            .into_iter()
            .flat_map(|selector| {
                [
                    u32::from_be_bytes(selector),
                    crate::property_data::DATA_TYPE,
                    0,
                ]
            })
            .flat_map(u32::to_ne_bytes)
            .collect(),
        12,
    )
}

fn channel_layout(channels: u32) -> Value {
    // Use explicit discrete labels. There is no fabricated speaker assignment
    // for a 7/13/33/64-channel virtual patchbay endpoint.
    let mut bytes: Vec<u8> = [0_u32, 0, channels]
        .into_iter()
        .flat_map(u32::to_ne_bytes)
        .collect();
    for index in 0..channels {
        for field in [65_536 + index, 0, 0, 0, 0] {
            bytes.extend_from_slice(&field.to_ne_bytes());
        }
    }
    Value::Struct(bytes)
}
