use std::{cell::RefCell, sync::atomic::AtomicUsize};

use super::*;
use crate::midi::state::Learn;
use mixrelay_midi_control::{Action, Control, Device, Kind, Mode, mailbox::Mailbox};
use mixrelay_time::ManualClock;
use serde_json::{Value, json};

/// The editor-facing half of the worker, owned by the test.
struct Shell {
    state: Mutex<State>,
    stop: AtomicBool,
    gate: tokio::sync::Mutex<()>,
    clock: ManualClock,
}

impl Shell {
    fn new() -> Self {
        Self::with_state(crate::midi::tests::memory_state())
    }

    fn with_state(state: State) -> Self {
        Self {
            state: Mutex::new(state),
            stop: AtomicBool::new(false),
            gate: tokio::sync::Mutex::new(()),
            clock: ManualClock::default(),
        }
    }

    fn shared(&self) -> Shared<'_> {
        Shared {
            state: &self.state,
            stop: &self.stop,
            write_gate: &self.gate,
        }
    }

    fn bind(&self, bindings: Vec<Binding>) {
        lock(&self.state).store.save(bindings).unwrap();
    }

    fn after(&self, milliseconds: u64) {
        self.clock.advance(Duration::from_millis(milliseconds));
    }
}

/// Connected devices, or `None` while the MIDI backend is unavailable.
type Connected = Arc<Mutex<Option<Vec<Device>>>>;

/// Opens a port per connected device and keeps it until the device disappears.
struct FakeInputs {
    connected: Connected,
    scans: Arc<AtomicUsize>,
    next_token: u32,
}

impl FakeInputs {
    fn new(devices: Vec<Device>) -> (Self, Connected, Arc<AtomicUsize>) {
        let connected = Arc::new(Mutex::new(Some(devices)));
        let scans = Arc::default();
        let inputs = Self {
            connected: Arc::clone(&connected),
            scans: Arc::clone(&scans),
            next_token: 0,
        };
        (inputs, connected, scans)
    }
}

impl Inputs for FakeInputs {
    type Connection = ();

    fn reconcile(
        &mut self,
        ports: &mut BTreeMap<String, Port<()>>,
        _capture: &Arc<LearnCapture>,
        sequence: &Arc<InputSequence>,
    ) -> Result<(), NativeError> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        let Some(devices) = lock(&self.connected).clone() else {
            ports.clear();
            return Err(NativeError::Ambiguous);
        };
        ports.retain(|id, _| devices.iter().any(|device| &device.id == id));
        for device in devices {
            ports.entry(device.id.clone()).or_insert_with(|| {
                self.next_token += 1;
                let mailbox = Arc::new(Mailbox::new(Arc::clone(sequence)));
                native::port_fixture::port(device, mailbox, self.next_token)
            });
        }
        Ok(())
    }
}

/// Records what the worker sends; hooks let a test change state mid-dispatch.
#[derive(Default)]
struct FakeEngine<'a> {
    snapshot: RefCell<Option<Value>>,
    rejects: bool,
    sent: RefCell<Vec<(Action, Option<f32>)>>,
    on_snapshot: Option<Box<dyn Fn() + 'a>>,
    on_act: Option<Box<dyn Fn() + 'a>>,
}

impl FakeEngine<'_> {
    fn with_snapshot(snapshot: Value) -> Self {
        Self {
            snapshot: RefCell::new(Some(snapshot)),
            ..Self::default()
        }
    }

    fn sent(&self) -> Vec<(Action, Option<f32>)> {
        self.sent.borrow().clone()
    }
}

impl Engine for FakeEngine<'_> {
    fn snapshot(&self) -> Option<Value> {
        if let Some(hook) = &self.on_snapshot {
            hook();
        }
        self.snapshot.borrow().clone()
    }

    fn act(
        &self,
        binding: &Binding,
        gain: Option<(f32, Option<f32>)>,
        _admission: &Admission<'_>,
    ) -> bool {
        self.sent
            .borrow_mut()
            .push((binding.action, gain.map(|(value, _)| value)));
        if let Some(hook) = &self.on_act {
            hook();
        }
        !self.rejects
    }
}

fn controller() -> Device {
    Device {
        id: "controller".into(),
        name: "Controller".into(),
    }
}

fn binding(action: Action, kind: Kind, number: u8) -> Binding {
    Binding {
        control: Control {
            device: controller(),
            channel: 0,
            kind,
            number,
        },
        target: mixrelay_midi_control::Target {
            graph_id: "main".into(),
            node_id: "gain".into(),
        },
        action,
        mode: Mode::Absolute,
        step_db: 1.0,
        graph_name: "Programme".into(),
        node_name: "Microphone".into(),
        min_db: -60.0,
        max_db: 12.0,
    }
}

/// A running graph whose bound node accepts volume at 0 dB.
fn volume_ready() -> Value {
    json!({"revision":1,"activeRevision":1,"enginePower":{"enabled":true,"transitioning":false},
        "graphs":[{"id":"main","name":"Programme","nodes":[
            {"nodeId":"gain","label":"Microphone","gainDb":0,"control":{"enabled":true}}]}]})
}

fn receive<I: Inputs, E, C>(worker: &Worker<'_, I, E, C>, bytes: &[u8]) {
    worker.ports["controller"].mailbox.receive(bytes);
}

#[test]
fn inputs_and_the_engine_snapshot_are_rescanned_every_second() {
    let shell = Shell::new();
    let engine = FakeEngine::with_snapshot(json!({"graphs":[]}));
    let (inputs, _, scans) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    assert_eq!(scans.load(Ordering::Relaxed), 1);
    {
        let state = lock(&shell.state);
        assert_eq!(state.devices, [controller()]);
        assert_eq!(state.ports, [(1, controller())]);
        assert_eq!(state.mailboxes.len(), 1);
        assert_eq!(state.snapshot, json!({"graphs":[]}));
        assert_eq!(state.unavailable, None);
    }
    shell.after(999);
    worker.tick();
    assert_eq!(scans.load(Ordering::Relaxed), 1);
    *engine.snapshot.borrow_mut() = None;
    shell.after(1);
    worker.tick();
    assert_eq!(scans.load(Ordering::Relaxed), 2);
    assert_eq!(lock(&shell.state).snapshot, Value::Null);
}

#[test]
fn an_unavailable_midi_backend_is_reported_until_inputs_return() {
    let shell = Shell::new();
    let engine = FakeEngine::default();
    let (inputs, connected, _) = FakeInputs::new(Vec::new());
    *lock(&connected) = None;
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    {
        let state = lock(&shell.state);
        assert!(state.devices.is_empty());
        assert_eq!(
            state.unavailable.as_deref(),
            Some("MIDI input names are ambiguous; rename the duplicate client or port")
        );
    }
    *lock(&connected) = Some(vec![controller()]);
    shell.after(1000);
    worker.tick();
    let state = lock(&shell.state);
    assert_eq!(state.devices, [controller()]);
    assert_eq!(state.unavailable, None);
}

#[test]
fn unreadable_preferences_stay_reported_while_inputs_work() {
    #[derive(Debug)]
    struct Corrupt;
    impl mixrelay_desktop_core::recording::PreferencesRepository for Corrupt {
        fn read(&self) -> std::io::Result<Option<Vec<u8>>> {
            Ok(Some(b"corrupt".to_vec()))
        }
        fn replace(&self, _: &[u8]) -> std::io::Result<()> {
            panic!("unreadable preferences must not be replaced")
        }
    }
    let shell = Shell::with_state(State::with_repository(Arc::new(Corrupt)));
    let engine = FakeEngine::with_snapshot(volume_ready());
    let (inputs, _, _) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    let state = lock(&shell.state);
    assert_eq!(state.devices, [controller()]);
    assert!(
        state
            .unavailable
            .as_deref()
            .is_some_and(|message| message.starts_with("MIDI preferences unavailable"))
    );
}

#[test]
fn a_bound_press_reaches_the_engine_once_and_map_mode_holds_input_back() {
    let shell = Shell::new();
    shell.bind(vec![binding(Action::Record, Kind::Note, 60)]);
    let engine = FakeEngine::default();
    let (inputs, _, _) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    receive(&worker, &[0x90, 60, 100]);
    shell.after(5);
    worker.tick();
    worker.tick();
    assert_eq!(engine.sent(), [(Action::Record, None)]);

    lock(&shell.state).map_mode = true;
    receive(&worker, &[0x90, 60, 100]);
    shell.after(5);
    worker.tick();
    assert_eq!(engine.sent().len(), 1);
}

#[test]
fn a_reconnected_controller_must_pick_up_its_fader_again() {
    let shell = Shell::new();
    shell.bind(vec![binding(Action::Volume, Kind::Cc, 7)]);
    let engine = FakeEngine::with_snapshot(volume_ready());
    let (inputs, connected, _) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    // 106 is 0 dB: the fader meets the node's level and takes over.
    receive(&worker, &[0xb0, 7, 106]);
    shell.after(5);
    worker.tick();
    assert_eq!(engine.sent(), [(Action::Volume, Some(0.0))]);

    *lock(&connected) = Some(Vec::new());
    shell.after(1000);
    worker.tick();
    assert!(lock(&shell.state).devices.is_empty());
    *lock(&connected) = Some(vec![controller()]);
    shell.after(1000);
    worker.tick();
    // A jump to -60 dB would follow a picked-up fader; after reconnecting it must not.
    receive(&worker, &[0xb0, 7, 0]);
    shell.after(1000);
    worker.tick();
    assert_eq!(engine.sent().len(), 1);
}

#[test]
fn a_rejected_gain_rescans_at_once_and_requires_pickup_again() {
    let shell = Shell::new();
    shell.bind(vec![binding(Action::Volume, Kind::Cc, 7)]);
    let engine = FakeEngine {
        rejects: true,
        ..FakeEngine::with_snapshot(volume_ready())
    };
    let (inputs, _, scans) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    receive(&worker, &[0xb0, 7, 106]);
    shell.after(5);
    worker.tick();
    assert_eq!(engine.sent(), [(Action::Volume, Some(0.0))]);
    shell.after(5);
    worker.tick();
    assert_eq!(scans.load(Ordering::Relaxed), 2);
    receive(&worker, &[0xb0, 7, 0]);
    shell.after(100);
    worker.tick();
    assert_eq!(engine.sent().len(), 1);
}

#[test]
fn cancelling_learn_drops_a_pending_gain_and_requires_pickup_again() {
    let shell = Shell::new();
    shell.bind(vec![binding(Action::Volume, Kind::Cc, 7)]);
    let engine = FakeEngine::with_snapshot(volume_ready());
    let (inputs, _, _) = FakeInputs::new(vec![controller()]);
    let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
    worker.tick();
    receive(&worker, &[0xb0, 7, 106]);
    worker.tick();
    // The next value waits out the gain rate bound.
    receive(&worker, &[0xb0, 7, 110]);
    shell.after(5);
    worker.tick();
    assert_eq!(engine.sent(), [(Action::Volume, Some(0.0))]);

    lock(&shell.state).clear_input();
    shell.after(40);
    worker.tick();
    receive(&worker, &[0xb0, 7, 0]);
    shell.after(40);
    worker.tick();
    assert_eq!(engine.sent(), [(Action::Volume, Some(0.0))]);
}

#[test]
fn a_dispatch_that_changes_bindings_or_learn_discards_the_rest_of_the_batch() {
    for change in 0..4 {
        let shell = Shell::new();
        let mute = binding(Action::Mute, Kind::Cc, 7);
        let record = binding(Action::Record, Kind::Cc, 8);
        shell.bind(vec![mute, record.clone()]);
        let engine = FakeEngine {
            on_act: Some(Box::new(|| {
                let mut state = lock(&shell.state);
                match change {
                    0 => state.store.save(vec![]).unwrap(),
                    1 => {
                        let mut replaced = record.clone();
                        replaced.action = Action::Stop;
                        state.store.save(vec![replaced]).unwrap();
                    }
                    2 => {
                        state.clear_input();
                        state.learn = Learn::Idle;
                        state.map_mode = false;
                    }
                    _ => {
                        state.learn = Learn::Waiting;
                        state.map_mode = true;
                    }
                }
            })),
            ..FakeEngine::default()
        };
        let (inputs, _, _) = FakeInputs::new(vec![controller()]);
        let mut worker = Worker::new(shell.shared(), inputs, &engine, &shell.clock);
        worker.tick();
        for bytes in [[0xb0, 7, 127], [0xb0, 7, 0], [0xb0, 7, 127], [0xb0, 8, 127]] {
            receive(&worker, &bytes);
        }
        shell.after(5);
        worker.tick();
        assert_eq!(engine.sent(), [(Action::Mute, None)], "change {change}");
    }
}

#[test]
fn run_ticks_on_the_injected_clock_until_shutdown() {
    let shell = Shell::new();
    let scans = AtomicUsize::new(0);
    let engine = FakeEngine {
        on_snapshot: Some(Box::new(|| {
            if scans.fetch_add(1, Ordering::Relaxed) == 2 {
                shell.stop.store(true, Ordering::Release);
            }
        })),
        ..FakeEngine::default()
    };
    let (inputs, _, _) = FakeInputs::new(vec![controller()]);
    let start = shell.clock.now();
    Worker::new(shell.shared(), inputs, &engine, &shell.clock).run();
    // Scans at 0, 1000 and 2000 ms; the third requests shutdown before one more wait.
    assert_eq!(
        shell.clock.now().duration_since(start),
        Duration::from_millis(2005)
    );
}
