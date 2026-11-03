use super::*;
use mixrelay_time::{ManualClock, MonotonicClock};
#[test]
fn only_stable_requested_state_completes_and_polling_keeps_the_original_deadline() {
    let clock = ManualClock::default();
    let acknowledgement = PowerAcknowledgement::new(false, clock.now());
    let power = |enabled, transitioning| EnginePowerStatus {
        enabled,
        transitioning,
        pause_reason: None,
        unsaved: false,
    };
    assert!(matches!(
        acknowledgement.observe(power(false, true), clock.now()),
        PowerProgress::Waiting(_)
    ));
    clock.advance(Duration::from_secs(29));
    assert!(matches!(
        acknowledgement.observe(power(true, false), clock.now()),
        PowerProgress::Waiting(_)
    ));
    clock.advance(Duration::from_secs(1));
    assert_eq!(
        acknowledgement.observe(power(true, false), clock.now()),
        PowerProgress::Expired
    );
    assert_eq!(
        acknowledgement.observe(power(false, false), clock.now()),
        PowerProgress::Complete
    );
}
