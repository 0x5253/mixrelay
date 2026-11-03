use super::*;
use std::cell::Cell;

struct Queue {
    clock: Cell<Option<ClientPcmClock>>,
    channels: u32,
    endpoint: [u8; 16],
    retire_on_read: bool,
    silent: bool,
}
impl Queue {
    fn new() -> Self {
        Self {
            clock: Cell::new(Some(ClientPcmClock {
                incarnation: [1; 16],
                epoch: 1,
                anchor: 0,
                rate: 48_000,
                running: true,
                first: Some(0),
                through: 1024,
            })),
            channels: 2,
            endpoint: [2; 16],
            retire_on_read: false,
            silent: false,
        }
    }
}
impl ClientQueueRead for Queue {
    fn endpoint(&self) -> [u8; 16] {
        self.endpoint
    }
    fn channels(&self) -> u32 {
        self.channels
    }
    fn capture_channels(&self) -> u32 {
        2
    }
    fn capacity(&self) -> u32 {
        512
    }
    fn clock(&self) -> Option<ClientPcmClock> {
        self.clock.get()
    }
    fn read(
        &self,
        clock: ClientPcmClock,
        first: u64,
        output: &mut [f32],
        coverage: &mut [bool],
    ) -> bool {
        if !same_clock(self.clock(), clock) {
            return false;
        }
        for (frame, samples) in output.chunks_exact_mut(self.channels as usize).enumerate() {
            samples.fill(if self.silent {
                0.0
            } else {
                f32::from(u16::try_from(first + frame as u64).unwrap())
            });
        }
        coverage.fill(!self.silent);
        if self.retire_on_read {
            self.clock.set(None);
        }
        true
    }
}
#[test]
fn overrun_and_late_join_skip_only_published_frames_and_commit_once() {
    let queues = [Queue::new()];
    let mut mixer = ClientPcmMixer::default();
    assert!(mixer.window(&queues, |_| None).is_none());
    let window = mixer.window(&queues, |_| Some(900)).unwrap();
    assert_eq!(
        (window.first, window.frames, window.discontinuity),
        (512, 128, true)
    );
    assert_eq!(mixer.skipped_frames(), 512);
    assert_eq!(mixer.window(&queues, |_| Some(900)).unwrap().first, 512);
    let mut output = [0.0; 256];
    let mut scratch = [0.0; 256];
    let allocated = allocation_counter::measure(|| {
        ClientPcmMixer::mix(&queues, window, &mut output, &mut scratch).unwrap();
        mixer.commit(window);
    });
    assert_eq!((allocated.count_total, allocated.count_current), (0, 0));
    assert_eq!(&output[..4], &[512.0, 512.0, 513.0, 513.0]);
    assert_eq!(mixer.window(&queues, |_| Some(900)).unwrap().first, 640);
    let mut clock = queues[0].clock().unwrap();
    clock.through = 2048;
    queues[0].clock.set(Some(clock));
    assert_eq!(mixer.window(&queues, |_| Some(2048)).unwrap().first, 1536);
    assert_eq!(mixer.skipped_frames(), 1408);
}
#[test]
fn disagreeing_clocks_or_retirement_keep_the_uncommitted_window_silent() {
    let queues = [Queue::new(), Queue::new()];
    let mut mixer = ClientPcmMixer::default();
    let mut replaced = queues[1].clock().unwrap();
    replaced.epoch = 2;
    queues[1].clock.set(Some(replaced));
    assert!(mixer.window(&queues, |_| Some(1024)).is_none());
    queues[1].clock.set(queues[0].clock());
    let window = mixer.window(&queues, |_| Some(1024)).unwrap();
    let mut changed = Queue::new();
    changed.retire_on_read = true;
    let mut output = [9.0; 256];
    assert_eq!(
        ClientPcmMixer::mix(&[changed], window, &mut output, &mut [0.0; 256]),
        Err(MixError::Changed)
    );
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 256]);
    assert_eq!(
        mixer.window(&queues, |_| Some(1024)).unwrap().first,
        window.first
    );
}
#[test]
fn silent_client_does_not_hold_others_and_geometry_failure_clears_output() {
    let mut silent = Queue::new();
    silent.silent = true;
    let mut queues = [Queue::new(), silent];
    let window = ClientPcmMixer::default()
        .window(&queues, |_| Some(1024))
        .unwrap();
    let mut output = [0.0; 256];
    ClientPcmMixer::mix(&queues, window, &mut output, &mut [0.0; 256]).unwrap();
    assert_eq!(&output[..2], &[512.0, 512.0]);
    queues[1].endpoint = [3; 16];
    assert_eq!(
        ClientPcmMixer::mix(&queues, window, &mut output, &mut [0.0; 256]),
        Err(MixError::Geometry)
    );
    assert_eq!(output.map(f32::to_bits), [0.0_f32.to_bits(); 256]);
}
