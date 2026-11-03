use super::*;
use mixrelay_audio_io::{ChannelLayout, NativeStreamFormats, StreamSpec, VirtualEndpointSpec};
use mixrelay_time::{Instant, ManualClock, MonotonicClock as _};
use std::{
    cell::{Cell, RefCell},
    time::Duration,
};

const ID: EndpointId = [7; 16];

fn manual() -> ManualClock {
    ManualClock::default()
}

/// One HAL device that applies or rejects each request as the test scripts it.
struct Device {
    rate: Cell<u32>,
    formats: RefCell<[Option<NativeStreamFormat>; 2]>,
    /// Rates whose Perform fails natively.
    failing: Vec<u32>,
    log: RefCell<crate::settlement::Log>,
    sequence: Cell<u64>,
    requests: RefCell<Vec<Wanted>>,
    incompatible: bool,
}
impl Device {
    fn at(rate: u32) -> Self {
        Self {
            rate: Cell::new(rate),
            formats: RefCell::new([Some(format(rate, NativeSampleFormat::Float32)); 2]),
            failing: Vec::new(),
            log: RefCell::new(replayed_log()),
            sequence: Cell::new(0),
            requests: RefCell::default(),
            incompatible: false,
        }
    }
    fn settle(&self, rate: u32, formats: [u32; 2], applied: bool) {
        self.sequence.set(self.sequence.get() + 1);
        self.log.borrow_mut().record(Configured {
            incarnation: 1,
            endpoint: ID,
            sequence: self.sequence.get(),
            rate,
            formats,
            applied,
        });
    }
}
impl Native for Device {
    fn rate(&self, _: EndpointId) -> Option<u32> {
        Some(self.rate.get())
    }
    fn set_rate(&self, _: EndpointId, rate: u32) -> bool {
        self.requests.borrow_mut().push(Wanted::Rate(rate));
        let applied = !self.failing.contains(&rate);
        if applied {
            self.rate.set(rate);
        }
        self.settle(rate, [1, 1], applied);
        true
    }
    fn format(&self, _: EndpointId, render: bool) -> Option<NativeStreamFormat> {
        self.formats.borrow()[usize::from(render)]
    }
    fn set_format(&self, _: EndpointId, render: bool, wanted: NativeStreamFormat) -> bool {
        let format = code(wanted.sample_format);
        self.requests.borrow_mut().push(Wanted::Format {
            render,
            rate: wanted.sample_rate,
            format,
        });
        self.formats.borrow_mut()[usize::from(render)] = Some(wanted);
        let mut formats = [1, 1];
        formats[usize::from(render)] = format;
        self.settle(wanted.sample_rate, formats, true);
        true
    }
    fn since(&self) -> Option<Since> {
        self.log.borrow().since()
    }
    fn settled(&self, id: EndpointId, since: Since, wanted: Wanted) -> Option<Configured> {
        self.log.borrow().find(id, since, wanted)
    }
    fn incompatible(&self) -> bool {
        self.incompatible
    }
}

fn replayed_log() -> crate::settlement::Log {
    let mut log = crate::settlement::Log::default();
    log.replayed(1);
    log
}

fn format(rate: u32, sample_format: NativeSampleFormat) -> NativeStreamFormat {
    NativeStreamFormat {
        sample_rate: rate,
        channels: 2,
        sample_format,
    }
}

fn config(rate: u32) -> DriverDeviceConfig {
    let stream = StreamSpec {
        channels: 2,
        channel_layout: ChannelLayout::Stereo,
    };
    DriverDeviceConfig {
        spec: VirtualEndpointSpec {
            id: "17e59d73-42c1-4f11-a8b4-6c2d913357e0".into(),
            display_name: "Device".into(),
            render: Some(stream),
            capture: Some(stream),
            supported_sample_rates: vec![48_000, 96_000],
            default_sample_rate: rate,
        },
        render_format: Some(format(rate, NativeSampleFormat::Float32)),
        capture_format: Some(format(rate, NativeSampleFormat::Float32)),
    }
}

#[test]
fn a_hal_requested_replacement_keeps_a_daw_chosen_rate() {
    // The service cached 48 kHz; a DAW has since moved the device to 96 kHz.
    let device = Device::at(96_000);
    let cached = config(48_000);
    let clock = manual();
    assert!(apply(&device, &clock, Origin::Replacement, &[(ID, &cached)]).is_ok());
    let previous = [Previous {
        id: ID,
        formats: NativeDeviceFormats::default(),
        rate: Some(48_000),
    }];
    assert!(restore(&device, &clock, Origin::Replacement, &previous).is_ok());
    assert!(device.requests.borrow().is_empty());
    assert_eq!(device.rate.get(), 96_000);
    // A manager transaction does converge on its intent.
    assert!(apply(&device, &clock, Origin::Manager, &[(ID, &cached)]).is_ok());
    assert_eq!(device.rate.get(), 48_000);
}

#[test]
fn requests_follow_hal_readback_rather_than_cached_intent() {
    // A Perform reported pending may settle at 96 kHz while the service still caches
    // 48 kHz; asking for 48 kHz again must reach the HAL, and nothing already held does.
    let device = Device::at(96_000);
    let clock = manual();
    assert!(apply(&device, &clock, Origin::Manager, &[(ID, &config(48_000))]).is_ok());
    assert!(apply(&device, &clock, Origin::Manager, &[(ID, &config(48_000))]).is_ok());
    assert_eq!(
        *device.requests.borrow(),
        [
            Wanted::Rate(48_000),
            Wanted::Format {
                render: false,
                rate: 48_000,
                format: 1
            },
            Wanted::Format {
                render: true,
                rate: 48_000,
                format: 1
            },
        ]
    );
}

#[test]
fn a_native_rate_failure_is_rejected_promptly() {
    let mut device = Device::at(48_000);
    device.failing.push(96_000);
    let clock = manual();
    let result = apply(&device, &clock, Origin::Manager, &[(ID, &config(96_000))]);
    assert!(
        matches!(&result, Err(Failure::Rejected(DriverError::Backend(message))) if message == "HAL rejected the configuration"),
        "{result:?}"
    );
    assert!(
        clock
            .now()
            .duration_since(Instant::from_origin(Duration::ZERO))
            < Duration::from_secs(1)
    );
}

#[test]
fn an_earlier_rejection_of_another_value_does_not_reject_a_new_request() {
    let device = Device::at(48_000);
    // An earlier, aborted request for 88.2 kHz settled before this one was issued.
    device.settle(88_200, [1, 1], false);
    let clock = manual();
    assert!(apply(&device, &clock, Origin::Manager, &[(ID, &config(96_000))]).is_ok());
    assert_eq!(device.rate.get(), 96_000);
}

#[test]
fn an_incompatible_hal_fails_the_request_at_once() {
    let mut device = Device::at(48_000);
    device.failing.push(96_000);
    device.incompatible = true;
    let clock = manual();
    let result = apply(&device, &clock, Origin::Manager, &[(ID, &config(96_000))]);
    assert!(matches!(result, Err(Failure::Incompatible)), "{result:?}");
    assert_eq!(
        clock
            .now()
            .duration_since(Instant::from_origin(Duration::ZERO)),
        Duration::ZERO
    );
}

#[test]
fn restore_converges_rate_and_both_directions_from_readback() {
    let device = Device::at(96_000);
    let clock = manual();
    let previous = [Previous {
        id: ID,
        formats: NativeDeviceFormats {
            render: NativeStreamFormats {
                current: Some(format(48_000, NativeSampleFormat::Signed24)),
                supported: Vec::new(),
            },
            capture: NativeStreamFormats::default(),
        },
        rate: Some(48_000),
    }];
    assert!(restore(&device, &clock, Origin::Manager, &previous).is_ok());
    assert_eq!(device.rate.get(), 48_000);
    assert_eq!(
        device.formats.borrow()[1],
        Some(format(48_000, NativeSampleFormat::Signed24))
    );
}
