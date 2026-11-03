use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

use mixrelay_processor_host::{
    PLUGIN_PROBE_ARGUMENT, PLUGIN_SCAN_ARGUMENT, PLUGIN_WORKER_ARGUMENT, RackFormat, RackLocator,
};
use truce_rack_core::scanner::PluginScanner;

pub(super) const FIXTURE_ID: &str = "io.mixrelay.test.native-clap-parameters-latency";

pub(super) fn run() -> Result<(), String> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--run") => {
            let path = arguments.next().ok_or("missing fixture path")?;
            if arguments.next().is_some() {
                return Err("unexpected qualification arguments".into());
            }
            let _deadline = QualificationDeadline::start()?;
            super::qualification::run(Path::new(&path))
        }
        Some(PLUGIN_SCAN_ARGUMENT) => scan(&arguments.collect::<Vec<_>>()),
        Some(PLUGIN_PROBE_ARGUMENT) => {
            let locator = RackLocator::decode(&arguments.next().ok_or("missing locator")?)?;
            require_fixture(&locator)?;
            mixrelay_processor_host::run_plugin_probe(locator)
        }
        Some(PLUGIN_WORKER_ARGUMENT) => {
            let values = arguments.collect::<Vec<_>>();
            let locator = RackLocator::decode(values.first().ok_or("missing locator")?)?;
            require_fixture(&locator)?;
            mixrelay_processor_host::run_plugin_worker(&values)
        }
        _ => Err("explicit --run <fixture.clap> opt-in is required".into()),
    }
}

struct QualificationDeadline {
    cancel: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl QualificationDeadline {
    fn start() -> Result<Self, String> {
        let (cancel, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("qualification-deadline".into())
            .spawn(move || {
                if receiver.recv_timeout(Duration::from_secs(90))
                    == Err(mpsc::RecvTimeoutError::Timeout)
                {
                    // Exiting closes each worker's owned stdin lifetime pipe, including when
                    // vendor code hangs before the first PCM callback can be processed.
                    std::process::exit(124);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            cancel,
            worker: Some(worker),
        })
    }
}

impl Drop for QualificationDeadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn require_fixture(locator: &RackLocator) -> Result<(), String> {
    if locator.unique_id == FIXTURE_ID && matches!(locator.format, RackFormat::Clap) {
        Ok(())
    } else {
        Err("qualification refuses a non-fixture plug-in".into())
    }
}

fn scan(arguments: &[String]) -> Result<(), String> {
    if arguments.first().map(String::as_str) != Some("clap") {
        println!("{{\"locators\":[],\"pathFailures\":[]}}");
        return Ok(());
    }
    if arguments.len() != 2 {
        return Err("fixture scanner requires exactly one explicit path".into());
    }
    let path = PathBuf::from(&arguments[1])
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if path.extension().and_then(|value| value.to_str()) != Some("clap") {
        return Err("fixture path must be a .clap file or bundle".into());
    }
    let infos = truce_rack_clap::ClapScanner::new()
        .scan_path(&path)
        .map_err(|error| error.to_string())?;
    if infos.len() != 1 || infos[0].unique_id != FIXTURE_ID {
        return Err("exactly one matching native fixture descriptor is required".into());
    }
    let info = &infos[0];
    let report = serde_json::json!({
        "locators": [{
            "format": "clap", "uniqueId": info.unique_id, "path": info.path,
            "name": info.name, "vendor": info.vendor, "version": info.version,
            "category": "effect", "hasEditor": false, "acceptsMidi": false
        }],
        "pathFailures": []
    });
    println!("{report}");
    Ok(())
}
