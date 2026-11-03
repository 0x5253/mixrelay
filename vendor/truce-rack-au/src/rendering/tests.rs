//! Current-block bounds and registered callback lifetime contracts.

use super::{
    RenderContext, RenderState, audio_timestamp, refresh_render_inputs, render_input_callback,
};
use crate::host_transport::{host_beat_and_tempo, update_host_transport};
use objc2_audio_toolbox::{AudioUnitRenderActionFlags, kAudioUnitErr_CannotDoInCurrentContext};
use objc2_core_audio_types::{AudioBuffer as NativeBuffer, AudioBufferList};
use std::ptr::{self, NonNull};
use truce_rack_core::{
    buffer::{AudioBuffer, BusRange},
    transport::TransportInfo,
};

fn input_pull(ctx: &mut RenderContext, frames: u32, output: &mut [f32]) -> (i32, *const f32) {
    let mut list = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [NativeBuffer {
            mNumberChannels: 1,
            mDataByteSize: u32::try_from(std::mem::size_of_val(output)).unwrap(),
            mData: output.as_mut_ptr().cast(),
        }],
    };
    let mut flags = AudioUnitRenderActionFlags::empty();
    let mut timestamp = audio_timestamp(0.0);
    // SAFETY: a live retained context and a fully initialized one-channel native list.
    let status = unsafe {
        render_input_callback(
            NonNull::from(ctx).cast(),
            NonNull::from(&mut flags),
            NonNull::from(&mut timestamp),
            0,
            frames,
            &raw mut list,
        )
    };
    (status, list.mBuffers[0].mData.cast_const().cast())
}

#[test]
fn callback_rejects_requests_past_the_current_short_block() {
    let mut state = RenderState::default();
    state.prepare(&[1], &[1], 512).unwrap();
    let input = [0.1_f32, 0.2, 0.3, 0.4];
    let inputs = [&input[..]];
    let mut outputs = [];
    let ranges = [BusRange::new(0, 1)];
    let buffer = AudioBuffer::new(&inputs, &mut outputs, 4, &ranges, &[]);
    let ctx = state.render_ctx.as_deref_mut().unwrap();
    assert!(refresh_render_inputs(Some(ctx), &buffer));
    let mut native_output = [1.0; 8];
    let (status, pointer) = input_pull(ctx, 8, &mut native_output);
    assert_ne!(
        status, 0,
        "a native pull cannot expose samples beyond the current host block"
    );
    assert_eq!(pointer, native_output.as_ptr());
    assert!(native_output.iter().all(|sample| sample.to_bits() == 0));
    let allocations = allocation_counter::measure(|| {
        let (status, pointer) = input_pull(ctx, 4, &mut native_output[..4]);
        assert_eq!(status, 0);
        // SAFETY: the successful pull points at the four-element input still borrowed above.
        assert_eq!(unsafe { std::slice::from_raw_parts(pointer, 4) }, input);
    });
    assert_eq!(allocations.count_total, 0);
}

#[test]
fn suspended_registration_keeps_refcons_alive_without_exposing_old_pcm() {
    let mut state = RenderState::default();
    state.prepare(&[1], &[1], 512).unwrap();
    let input = [0.25; 4];
    let ctx = state.render_ctx.as_deref_mut().unwrap();
    ctx.input_planes[0] = input.as_ptr();
    ctx.frames = input.len();
    let input_refcon = ptr::from_ref(ctx);
    let transport = state.host_transport.as_deref_mut().unwrap();
    update_host_transport(transport, Some(TransportInfo::default()), 48_000.0, 0.0);
    let transport_refcon = ptr::from_ref(transport);
    state.suspend();
    // Check ownership before dereferencing either recorded refcon, including on the old path.
    assert_eq!(
        state.render_ctx.as_deref().map(ptr::from_ref),
        Some(input_refcon)
    );
    assert_eq!(
        state.host_transport.as_deref().map(ptr::from_ref),
        Some(transport_refcon)
    );
    let mut native_output = [1.0; 4];
    let ctx = state.render_ctx.as_deref_mut().unwrap();
    let (status, _) = input_pull(ctx, 4, &mut native_output);
    assert_ne!(status, 0);
    assert!(native_output.iter().all(|sample| sample.to_bits() == 0));
    // SAFETY: suspend retained the registered allocation, and callbacks are serialized.
    assert_eq!(
        unsafe {
            host_beat_and_tempo(
                transport_refcon.cast_mut().cast(),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        },
        kAudioUnitErr_CannotDoInCurrentContext
    );
    state.prepare(&[1, 1], &[2], 128).unwrap();
    assert_eq!(
        state.render_ctx.as_deref().map(ptr::from_ref),
        Some(input_refcon)
    );
    assert_eq!(
        state.host_transport.as_deref().map(ptr::from_ref),
        Some(transport_refcon)
    );
}
