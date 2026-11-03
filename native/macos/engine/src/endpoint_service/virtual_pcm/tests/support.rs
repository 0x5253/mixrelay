//! Synthetic native time and control storage stalls around the production endpoint owner.
use super::super::*;
use mixrelay_macos_transport::{
    application_clock::ConsumerRoute,
    client_pcm::{ClientPcmSnapshot, ClientPcmWriter},
    endpoint_transport::EndpointTransportConfig,
    source_registry::content_latency::ReadTiming,
};
use std::{
    cell::{Cell, RefCell},
    io,
    os::fd::OwnedFd,
    rc::Rc,
    time::Duration,
};

type StorageHook = Box<dyn FnMut() -> io::Result<()>>;
thread_local! {
    static STORAGE: RefCell<Option<StorageHook>> = RefCell::new(None);
    static FAIL_WORKER: Cell<bool> = const { Cell::new(false) };
}
pub(super) fn fail_worker() {
    FAIL_WORKER.set(true);
}
fn storage(bytes: usize) -> io::Result<OwnedFd> {
    STORAGE.with_borrow_mut(|hook| hook.as_mut().map_or(Ok(()), |hook| hook()))?;
    super::super::test_storage::create(bytes)
}
pub(super) fn on_storage(hook: impl FnMut() -> io::Result<()> + 'static) {
    STORAGE.with_borrow_mut(|slot| *slot = Some(Box::new(hook)));
}
struct Clock;
impl mixrelay_time::MonotonicClock for Clock {
    fn now(&self) -> mixrelay_time::Instant {
        mixrelay_time::Instant::from_origin(Duration::ZERO)
    }
}

pub(super) struct ManualWorker(pub(super) pump::Pump);
impl Worker for ManualWorker {
    fn start(pump: pump::Pump) -> io::Result<Self> {
        if FAIL_WORKER.replace(false) {
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        Ok(Self(pump))
    }
    fn running(&self) -> bool {
        true
    }
    fn wake(&self) {}
    fn finish(self) {}
}
pub(super) type Owner = VirtualPcm<ManualWorker>;
pub(super) fn owner() -> Rc<Owner> {
    Rc::new(Owner::with_sources(SourceRegistry::with_clock_and_storage(
        Box::new(Clock),
        mixrelay_realtime::Timebase::current(),
        storage,
    )))
}
pub(super) fn shape(channels: u32) -> EndpointGeometry {
    EndpointGeometry {
        id: [1; 16],
        render_channels: channels,
        capture_channels: channels,
    }
}
pub(super) fn token() -> u64 {
    format!("virtual:{}", format_endpoint_id([1; 16]))
        .bytes()
        .fold(14_695_981_039_346_656_037, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(1_099_511_628_211)
        })
}
pub(super) fn word(frame: u64, channel: usize) -> f32 {
    let sample = f32::from_bits(0x3f00_0000 + u32::try_from(frame).unwrap());
    if channel.is_multiple_of(2) {
        sample
    } else {
        -sample
    }
}

pub(super) struct Native {
    pub(super) writer: ClientPcmWriter,
    pub(super) rate: u32,
    pub(super) channels: usize,
    pub(super) frames: u64,
    pub(super) anchor: u64,
    pub(super) epoch: u64,
    pub(super) now: u64,
    pub(super) running: bool,
    pub(super) allocations: u32,
}
impl Native {
    pub(super) fn new(rate: u32, channels: u32) -> Self {
        Self {
            writer: ClientPcmWriter::new(
                EndpointTransportConfig {
                    endpoint_id: [1; 16],
                    capacity_frames: QUEUE_FRAMES,
                    render_channels: channels,
                    capture_channels: channels,
                },
                42,
            )
            .unwrap(),
            rate,
            channels: channels as usize,
            frames: 0,
            anchor: 0,
            epoch: 1,
            now: Timebase::current().ticks(Duration::from_secs(1)),
            running: false,
            allocations: 0,
        }
    }
    pub(super) fn start(&mut self) {
        self.epoch += 1;
        self.anchor = self.now;
        self.frames = 0;
        self.running = true;
        assert!(
            self.writer
                .set_clock((self.epoch, self.anchor), self.rate, true, true)
        );
    }
    pub(super) fn write(&mut self) {
        let pcm: Vec<_> = (0..128 * self.channels)
            .map(|index| {
                word(
                    self.frames + (index / self.channels) as u64,
                    index % self.channels,
                )
            })
            .collect();
        assert!(self.writer.write(self.frames, &pcm, 128));
        self.frames += 128;
        self.now = self.anchor
            + Timebase::current().ticks(Duration::from_nanos(
                (self.frames + 32) * 1_000_000_000 / u64::from(self.rate),
            ));
    }
    /// A four-second preparation stall on a synthetic clock. If admission has
    /// already started native IO, HAL continues writing into its production-size queue.
    pub(super) fn slow_storage(&mut self) {
        self.allocations += 1;
        if self.running {
            for _ in 0..self.rate * 4 / 128 {
                self.write();
            }
        } else {
            self.now += Timebase::current().ticks(Duration::from_secs(4));
        }
    }
    pub(super) fn queues(&self) -> Vec<ClientPcmReader> {
        let mut snapshot = ClientPcmSnapshot::new().unwrap();
        assert!(snapshot.push(&self.writer));
        // SAFETY: the owned snapshot retains both HAL-shaped grants while decoding.
        unsafe { ClientPcmSnapshot::decode(snapshot.as_raw()) }.unwrap()
    }
}

pub(super) struct Fixture {
    pub(super) owner: Rc<Owner>,
    pub(super) native: Rc<RefCell<Native>>,
}
impl Fixture {
    pub(super) fn new(rate: u32, channels: u32) -> Self {
        let native = Rc::new(RefCell::new(Native::new(rate, channels)));
        let observed = Rc::clone(&native);
        on_storage(move || {
            observed.borrow_mut().slow_storage();
            Ok(())
        });
        Self {
            owner: owner(),
            native,
        }
    }
    pub(super) fn prepare(&self) {
        let channels = u32::try_from(self.native.borrow().channels).unwrap();
        self.owner
            .configure(&[shape(channels)], host_time())
            .unwrap();
    }
    pub(super) fn admit(&self) {
        self.admit_at(host_time());
    }
    pub(super) fn admit_at(&self, observed: u64) {
        self.native.borrow_mut().start();
        let queues = self.native.borrow().queues();
        self.owner.replace(queues, observed, host_time()).unwrap();
        self.activate();
    }
    pub(super) fn activate(&self) {
        self.update();
        assert!(
            self.owner
                .sources
                .set_consumer_route(token(), ConsumerRoute::Routed { head: true })
        );
        let _ = self
            .owner
            .sources
            .source_time(token(), self.native.borrow().rate, 128);
        assert!(self.owner.sources.set_clock_master(token(), true));
    }
    pub(super) fn update(&self) {
        let mut endpoints = self.owner.endpoints.lock();
        endpoints
            .get_mut(&[1; 16])
            .unwrap()
            .worker
            .as_mut()
            .unwrap()
            .0
            .update();
    }
    pub(super) fn cycle(&self) {
        let now = self.native.borrow().now;
        let mut endpoints = self.owner.endpoints.lock();
        let pump = &mut endpoints
            .get_mut(&[1; 16])
            .unwrap()
            .worker
            .as_mut()
            .unwrap()
            .0;
        pump.cycle(
            now,
            Timebase::current(),
            &scheduling::Scheduling::standard(),
        )
        .unwrap();
    }
    pub(super) fn play(&self, frames: u32) {
        for _ in 0..frames / 128 {
            self.native.borrow_mut().write();
            self.cycle();
        }
    }
    pub(super) fn verify(&self) {
        let native = self.native.borrow();
        let mut output = vec![0.0; 128 * native.channels];
        for first in (0..native.frames).step_by(128) {
            let first_nanos = Timebase::current().nanoseconds(native.anchor)
                + first * 1_000_000_000 / u64::from(native.rate);
            self.owner
                .sources
                .read_audio_timed(
                    token(),
                    &mut output,
                    128,
                    ReadTiming {
                        rate: native.rate,
                        first_nanos: Some(first_nanos),
                        host_ticks: None,
                    },
                )
                .unwrap_or_else(|miss| panic!("lost native frame {first}: {miss}"));
            for (index, &sample) in output.iter().enumerate() {
                assert_eq!(
                    sample.to_bits(),
                    word(
                        first + (index / native.channels) as u64,
                        index % native.channels
                    )
                    .to_bits(),
                    "native frame {} channel {}",
                    first + (index / native.channels) as u64,
                    index % native.channels
                );
            }
        }
        let progress = self.owner.sources.pcm_progress(token()).unwrap();
        assert_eq!(
            progress.published_frames, native.frames,
            "the source lost native PCM"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.owner.clear();
        STORAGE.with_borrow_mut(Option::take);
    }
}
