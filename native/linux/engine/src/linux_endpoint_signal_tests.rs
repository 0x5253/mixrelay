#![cfg(feature = "native-lab")]
use super::*;

// OS-side playback into the exported sink, independent of the graph writer.
// This test producer is not a timing benchmark or a device-clock implementation.
pub(super) struct Playback {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Playback {
    pub(super) fn new(id: &str, value: f32) -> Self {
        let name = format!("{}.render", native_name(&id.into()));
        let serial = inventory()[&name].clone();
        let mut stream = NativeStream::open_format(
            &mixrelay_linux_pipewire::StreamOwners::default(),
            "lab.os-playback",
            "Owned OS playback",
            StreamRole::AutomaticDeviceOutput(&serial),
            32,
            super::super::super::native_format(render_format()),
        )
        .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            let pcm = vec![value; 128 * 32];
            while !stopped.load(Ordering::Acquire) {
                if stream.write(&pcm) == 0 {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // Always drain the owned producer, including an assertion unwind.
            let result = thread.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

fn request(enabled: Option<bool>, muted: Option<bool>, gain_db: Option<f32>) -> NodeControlRequest {
    NodeControlRequest {
        expected_revision: 0,
        enabled,
        muted,
        gain_db,
    }
}

#[test]
fn native_device_signal_sums_os_playback_and_graph_controls() {
    let _driver_devices = DriverFixture::new();
    let session = Session::new();
    let _fixture = fixture();
    let graph_a = session.controller.graph_catalog().unwrap().active_id;
    session.apply(input_graph());
    let graph_b = session.catalog(GraphCatalogAction::Create {
        name: "Reader".into(),
    });
    session.apply(forwarding_graph());
    let mut direct = reader(A);
    let mut forwarded = reader(B);
    let nodes = inventory();
    let playback = Playback::new(A, 0.125);
    assert_signal(&mut direct, 0.125, 1.0);
    assert_signal(&mut forwarded, 0.125, 1.0);

    session.control(&graph_a, "input", request(None, None, Some(-6.020_6)));
    assert_signal(&mut forwarded, 0.125, 0.5);
    session.control(&graph_a, "input", request(None, Some(true), None));
    assert_signal(&mut forwarded, 0.125, 0.0);
    session.control(&graph_a, "input", request(None, Some(false), Some(0.0)));
    assert_signal(&mut forwarded, 0.125, 1.0);

    // OFF is graph-local: another graph and the OS retain their canonical bus.
    session.control(&graph_b, "a", request(Some(false), None, None));
    assert_signal(&mut forwarded, 0.0, 0.0);
    assert_signal(&mut direct, 0.125, 1.0);
    assert_eq!(inventory(), nodes);
    session.control(&graph_b, "a", request(Some(true), None, None));
    assert_signal(&mut forwarded, 0.125, 1.0);
    assert_eq!(inventory(), nodes);
    drop(playback);
    assert_pcm(&mut direct);
    assert_pcm(&mut forwarded);
    println!(
        "canonical_mix=OS+graph; native_channels=32; gain=mute=off=on=passed; virtual_reopens=0"
    );
}
