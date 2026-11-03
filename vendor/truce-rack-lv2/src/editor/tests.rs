use super::*;
use crate::ports::PortKind;

#[test]
fn float_callback_accepts_only_finite_input_control_writes() {
    let ports = [
        PortInfo {
            index: 0,
            kind: PortKind::Control { default: 0.0 },
            is_input: true,
            audio_plane: None,
        },
        PortInfo {
            index: 1,
            kind: PortKind::Control { default: 0.0 },
            is_input: false,
            audio_plane: None,
        },
    ];
    let controller = Box::new(Controller::for_ports(&ports, &[0, 0], 1));
    let opaque = (controller.as_ref() as *const Controller)
        .cast_mut()
        .cast::<c_void>();
    let value = 0.625_f32;
    ui_write_callback(opaque, 1, 4, 0, (&raw const value).cast());
    ui_write_callback(opaque, 0, 4, 1, (&raw const value).cast());
    ui_write_callback(opaque, 0, 3, 0, (&raw const value).cast());
    assert_eq!(controller.pending.peek(0), None);

    ui_write_callback(opaque, 0, 4, 0, (&raw const value).cast());
    assert_eq!(controller.pending.take(0), Some(value));
    let mut unaligned = [0_u8; 5];
    unaligned[1..].copy_from_slice(&0.75_f32.to_ne_bytes());
    ui_write_callback(opaque, 0, 4, 0, unaligned[1..].as_ptr().cast());
    assert_eq!(controller.pending.take(0), Some(0.75));
    let invalid = f32::INFINITY;
    ui_write_callback(opaque, 0, 4, 0, (&raw const invalid).cast());
    assert_eq!(controller.pending.peek(0), None);
}
