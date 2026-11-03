use super::*;
use mixrelay_time::{Instant, ManualClock};

fn pending() -> Evidence {
    Evidence {
        directions: [DirectionEvidence::Absent; 2],
        ready: false,
        processing_failed: false,
        disconnected: false,
    }
}

#[test]
fn timeout_uses_the_complete_supplied_budget_without_real_waiting() {
    let clock = ManualClock::default();
    assert_eq!(
        wait([None, None], &clock, pending),
        Err(AdmissionError::Timeout)
    );
    assert_eq!(clock.now(), Instant::from_origin(ADMISSION_BUDGET));
}

#[test]
fn matching_tuple_at_the_deadline_is_admitted_before_timeout_and_faults() {
    let clock = ManualClock::default();
    let requested = crate::plan::tests::config("device", 2);
    let formats = crate::plan::DevicePlan::new(&requested).unwrap().formats;
    wait(formats, &clock, || {
        if clock.now() < Instant::from_origin(ADMISSION_BUDGET) {
            Evidence {
                directions: [DirectionEvidence::Unobserved; 2],
                ..pending()
            }
        } else {
            Evidence {
                directions: formats.map(DirectionEvidence::Current),
                ready: true,
                processing_failed: true,
                disconnected: true,
            }
        }
    })
    .unwrap();
    assert_eq!(clock.now(), Instant::from_origin(ADMISSION_BUDGET));
}

#[test]
fn readiness_alone_cannot_accept_wrong_or_unobserved_formats() {
    let clock = ManualClock::default();
    let formats = crate::plan::DevicePlan::new(&crate::plan::tests::config("device", 2))
        .unwrap()
        .formats;
    let mut observations = 0;
    wait(formats, &clock, || {
        observations += 1;
        let directions = match observations {
            1 => [DirectionEvidence::Absent; 2],
            2 => [DirectionEvidence::Unobserved; 2],
            3 => [DirectionEvidence::Current(None); 2],
            _ => formats.map(DirectionEvidence::Current),
        };
        Evidence {
            directions,
            ready: true,
            ..pending()
        }
    })
    .unwrap();
    assert_eq!(observations, 4);
    assert_eq!(clock.now(), Instant::from_origin(OBSERVATION_STEP * 3));
}

#[test]
fn processing_failure_and_disconnect_end_admission_without_sleeping() {
    let clock = ManualClock::default();
    assert_eq!(
        wait([None, None], &clock, || Evidence {
            processing_failed: true,
            disconnected: true,
            ..pending()
        }),
        Err(AdmissionError::ProcessingFailed)
    );
    assert_eq!(
        wait([None, None], &clock, || Evidence {
            disconnected: true,
            ..pending()
        }),
        Err(AdmissionError::Disconnected)
    );
    assert_eq!(clock.now(), Instant::from_origin(Duration::ZERO));
}
