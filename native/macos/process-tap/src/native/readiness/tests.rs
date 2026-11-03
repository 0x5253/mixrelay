use super::*;
use std::cell::Cell;

struct Fixture {
    phase: Cell<usize>,
    ready_at: usize,
    stream: TapFormat,
    configured: u32,
}

fn format(channels: u32) -> TapFormat {
    TapFormat {
        sample_rate: 48_000.0,
        format_id: u32::from_be_bytes(*b"lpcm"),
        flags: 9,
        bytes_per_packet: 4 * channels,
        frames_per_packet: 1,
        bytes_per_frame: 4 * channels,
        channels,
        bits: 32,
    }
}

impl Probe for Fixture {
    fn alive(&self, _: u32) -> bool {
        true
    }
    fn format(&self, _: u32) -> Result<TapFormat, TapError> {
        Ok(format(2))
    }
    fn streams(&self, _: u32) -> Result<Vec<TapFormat>, TapError> {
        Ok(if self.phase.get() >= self.ready_at {
            vec![self.stream]
        } else {
            Vec::new()
        })
    }
    fn channels(&self, _: u32) -> Option<u32> {
        Some(self.configured)
    }
    fn pause(&self) {
        self.phase.set(self.phase.get() + 1);
    }
}

#[test]
fn alive_aggregate_waits_for_real_input_geometry() {
    let fixture = Fixture {
        phase: Cell::new(0),
        ready_at: 7,
        stream: format(2),
        configured: 2,
    };
    wait(1, 2, &fixture).unwrap();
    assert_eq!(
        fixture.phase.get(),
        7,
        "alive admitted an aggregate without streams"
    );
}

#[test]
fn incomplete_or_wrong_geometry_cannot_reach_registration() {
    for (stream, configured) in [
        (format(1), 2),
        (format(2), 1),
        (
            TapFormat {
                sample_rate: 44_100.0,
                ..format(2)
            },
            2,
        ),
        (
            TapFormat {
                bytes_per_frame: 0,
                ..format(2)
            },
            2,
        ),
        (
            TapFormat {
                sample_rate: f64::NAN,
                ..format(2)
            },
            2,
        ),
    ] {
        let fixture = Fixture {
            phase: Cell::new(0),
            ready_at: 0,
            stream,
            configured,
        };
        assert!(wait(1, 2, &fixture).is_err());
        assert_eq!(fixture.phase.get(), ATTEMPTS - 1);
    }
}
