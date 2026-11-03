use super::*;

pub(super) fn run(mode: &str) {
    // SAFETY: disable core-file output only in this disposable fault-injection child.
    unsafe {
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(libc::setrlimit(libc::RLIMIT_CORE, &raw const limit), 0);
    }
    let path = std::path::PathBuf::from(std::env::var_os(PATH).unwrap());
    let directory = path.parent().unwrap();
    let (mut audio, control) = shared_transport::worker(&path).unwrap();
    let _parent = crate::dsp_lifecycle::watch_parent(Arc::new(AtomicBool::new(false))).unwrap();
    if matches!(mode, "early_exit" | "load_hang") {
        let _load = control.header().native_operation.enter(Stage::Load);
        if mode == "early_exit" {
            std::process::exit(23);
        }
        hang();
    }
    control
        .header()
        .worker_status
        .store(WORKER_READY, Ordering::Release);
    let mut sequence = 0_u64;
    let mut source = [0.0; 8];
    while control.header().shutdown.load(Ordering::Acquire) == 0 {
        if mode == "state_hang" && control.pending_request().is_some() {
            let _state = control.header().native_operation.enter(Stage::State);
            hang();
        }
        if mode == "editor_hang" && control.header().editor_sequence.load(Ordering::Acquire) != 0 {
            let _editor = control.header().native_operation.enter(Stage::Editor);
            hang();
        }
        if audio
            .receive(|input| source.copy_from_slice(input))
            .is_none()
        {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        sequence += 1;
        let _process = control.header().dsp_operation.enter(Stage::Process);
        if sequence == 1 {
            match mode {
                "crash" => std::process::abort(),
                "panic_abort" => panic_across_native_boundary(),
                "process_hang" => hang(),
                "late" => {
                    while !directory.join("release").exists() {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                _ => {}
            }
        }
        assert!(audio.publish(|output| {
            for (destination, input) in output.iter_mut().zip(source) {
                *destination = if mode == "late" && sequence == 1 {
                    9.0
                } else {
                    input * 2.0
                };
            }
        }));
        std::fs::write(directory.join("published"), sequence.to_string()).unwrap();
    }
}

extern "C" fn panic_across_native_boundary() {
    panic!("injected non-unwinding native callback panic");
}

fn hang() -> ! {
    loop {
        thread::park();
    }
}
