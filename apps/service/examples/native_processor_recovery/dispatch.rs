use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{mpsc, mpsc::RecvTimeoutError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use mixrelay_processor_host::{
    PLUGIN_PROBE_ARGUMENT, PLUGIN_SCAN_ARGUMENT, PLUGIN_WORKER_ARGUMENT, RackFormat, RackLocator,
};
use truce_rack_core::scanner::PluginScanner;

pub(super) const FIXTURE_ID: &str = "io.mixrelay.test.native-clap-parameters-latency";
pub(super) const FIXTURE_ENV: &str = "MIXRELAY_RECOVERY_FIXTURE";
pub(super) const PRIVATE_ENV: &str = "MIXRELAY_RECOVERY_PRIVATE_DIR";
pub(super) const CONFIG_ENV: &str = "MIXRELAY_CONFIG_DIR";
const QUALIFICATION_TIMEOUT: Duration = Duration::from_secs(90);
const PARENT_TIMEOUT: Duration = Duration::from_secs(95);

pub(super) fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next();
    let arguments: Vec<_> = args.collect();
    match command.as_deref() {
        Some("--run") => launch(&arguments),
        Some("--qualify") => qualify(&arguments),
        Some(PLUGIN_SCAN_ARGUMENT) => scan(&arguments),
        Some(PLUGIN_PROBE_ARGUMENT) => probe(&arguments),
        Some(PLUGIN_WORKER_ARGUMENT) => worker(&arguments),
        _ => Err("explicit --run <native-clap-fixture.clap> is required".into()),
    }
}

fn launch(args: &[String]) -> Result<(), String> {
    if args.len() != 1 {
        return Err("--run requires exactly one fixture path".into());
    }
    let fixture = canonical_fixture(Path::new(&args[0]))?;
    let private = mixrelay_test_support::FixtureDir::new("native-processor-recovery")
        .map_err(|error| error.to_string())?;
    let config = private.path().join("config");
    std::fs::create_dir(&config).map_err(|error| error.to_string())?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut child = Command::new(executable)
        .arg("--qualify")
        .env(FIXTURE_ENV, &fixture)
        .env(PRIVATE_ENV, private.path())
        .env(CONFIG_ENV, config)
        .spawn()
        .map_err(|error| format!("qualification child: {error}"))?;
    wait_child(&mut child)
}

fn wait_child(child: &mut Child) -> Result<(), String> {
    let deadline = Instant::now() + PARENT_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("qualification child exited {status}"))
            };
        }
        if Instant::now() >= deadline {
            child.kill().map_err(|error| error.to_string())?;
            let status = child.wait().map_err(|error| error.to_string())?;
            return Err(format!(
                "qualification exceeded {PARENT_TIMEOUT:?}: {status}"
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn qualify(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("unexpected --qualify arguments".into());
    }
    let _deadline = Deadline::start()?;
    super::qualification::run()
}

fn scan(args: &[String]) -> Result<(), String> {
    let [format, path] = args else {
        return Err("scanner requires format and one explicit path".into());
    };
    let format = RackFormat::parse(format)?;
    if format != RackFormat::Clap {
        println!("{{\"locators\":[],\"pathFailures\":[]}}");
        return Ok(());
    }
    let fixture = expected_fixture()?;
    let path = canonical_fixture(Path::new(path))?;
    if path != fixture {
        return Err("scanner path differs from the opted-in fixture".into());
    }
    let infos = truce_rack_clap::ClapScanner::new()
        .scan_path(&path)
        .map_err(|error| error.to_string())?;
    if infos.len() != 1 || infos[0].unique_id != FIXTURE_ID {
        return Err("exactly one matching native fixture descriptor is required".into());
    }
    let info = &infos[0];
    println!(
        "{}",
        serde_json::json!({
            "locators": [{
                "format": "clap",
                "uniqueId": info.unique_id,
                "path": info.path,
                "name": info.name,
                "vendor": info.vendor,
                "version": info.version,
                "category": "effect",
                "hasEditor": info.has_editor,
                "acceptsMidi": info.accepts_midi,
            }],
            "pathFailures": []
        })
    );
    Ok(())
}

fn probe(args: &[String]) -> Result<(), String> {
    let [encoded] = args else {
        return Err("probe requires exactly one locator".into());
    };
    let locator = RackLocator::decode(encoded)?;
    require_fixture(&locator)?;
    mixrelay_processor_host::run_plugin_probe(locator)
}

fn worker(args: &[String]) -> Result<(), String> {
    let encoded = args.first().ok_or("worker is missing its locator")?;
    let locator = RackLocator::decode(encoded)?;
    require_fixture(&locator)?;
    let instrumentation = super::worker::WorkerInstrumentation::start(args)?;
    let result = mixrelay_processor_host::run_plugin_worker(args);
    drop(instrumentation);
    result
}

fn require_fixture(locator: &RackLocator) -> Result<(), String> {
    if locator.format != RackFormat::Clap
        || locator.unique_id != FIXTURE_ID
        || canonical_fixture(&locator.path)? != expected_fixture()?
    {
        return Err("qualification refuses a non-fixture processor".into());
    }
    Ok(())
}

fn expected_fixture() -> Result<PathBuf, String> {
    let path = std::env::var_os(FIXTURE_ENV)
        .map(PathBuf::from)
        .ok_or("qualification fixture path is not configured")?;
    canonical_fixture(&path)
}

fn canonical_fixture(path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if canonical.extension().and_then(|value| value.to_str()) != Some("clap") {
        return Err("fixture must be the explicit .clap bundle or file".into());
    }
    Ok(canonical)
}

struct Deadline {
    cancel: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl Deadline {
    fn start() -> Result<Self, String> {
        let (cancel, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("native-recovery-deadline".into())
            .spawn(move || wait_deadline(&receiver))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            cancel,
            worker: Some(worker),
        })
    }
}

fn wait_deadline(receiver: &mpsc::Receiver<()>) {
    if matches!(
        receiver.recv_timeout(QUALIFICATION_TIMEOUT),
        Err(RecvTimeoutError::Timeout)
    ) {
        // Process exit closes every owned worker stdin lifetime pipe.
        std::process::exit(124);
    }
}

impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
