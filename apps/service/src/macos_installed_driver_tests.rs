//! Explicit installed Driver observation composed with the native PCM contract probe.
use mixrelay_driver_adapters::DriverClient;

#[test]
#[ignore = "explicit installed Driver, exclusive PCM path and soak duration required"]
fn installed_driver_pcm_soak() {
    let client = DriverClient::new().expect("installed Driver control client");
    mixrelay_macos_engine::native_lab::installed_driver_pcm_soak(|| {
        client
            .snapshot_blocking()
            .expect("read-only actual Driver snapshot")
    });
}
