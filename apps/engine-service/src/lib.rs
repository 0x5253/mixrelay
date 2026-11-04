//! Composes engine domain ports with native processor, recorder and file adapters, and
//! the macOS and Linux services that own them.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use mixrelay_pcm_ring as _;

mod adapters;
pub mod control_service;
pub mod controller;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod shutdown;

// Clippy recognizes test modules only by a plain `#[cfg(test)]`, so platform and lab
// conditions stay in separate attributes.
#[cfg(test)]
#[cfg(target_os = "macos")]
#[test]
fn native_callback_and_hal_contracts() {
    mixrelay_macos_test_support::assert_native_contracts();
}
/// The service bundle identity and microphone usage text that `build.rs` links into the
/// shipped binary. Live Driver capture tests open input `IOProc`s, so TCC reads the same
/// identity from this test harness when it is the responsible process.
#[cfg(test)]
#[cfg(target_os = "macos")]
#[used]
// SAFETY: the section holds only these plist bytes, which the linker emits verbatim.
#[unsafe(link_section = "__TEXT,__info_plist")]
static SERVICE_INFO_PLIST: [u8; include_bytes!("../../../native/macos/service/Info.plist").len()] =
    *include_bytes!("../../../native/macos/service/Info.plist");
