use super::*;
use crate::HAL_PATH;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    os::unix::process::ExitStatusExt,
    process::ExitStatus,
};

struct Hosts {
    pids: Result<Vec<u32>, NativeError>,
    identities: VecDeque<Result<&'static str, NativeError>>,
    exits: VecDeque<Result<bool, NativeError>>,
    events: Vec<String>,
    signal_fails: bool,
}

impl Hosts {
    fn valid() -> Self {
        Self {
            pids: Ok(vec![42]),
            identities: VecDeque::from([
                Ok("generation-a"),
                Ok("generation-a"),
                Ok("generation-a"),
            ]),
            exits: VecDeque::from([Ok(true)]),
            events: Vec::new(),
            signal_fails: false,
        }
    }
}

impl HostProcesses for Hosts {
    fn candidates(&mut self) -> Result<Vec<u32>, NativeError> {
        self.events.push("discover".into());
        match &self.pids {
            Ok(pids) => Ok(pids.clone()),
            Err(_) => std::mem::replace(&mut self.pids, Ok(Vec::new())),
        }
    }
    fn identity(&mut self, pid: u32, _bundles: &[PathBuf]) -> Result<HostIdentity, NativeError> {
        self.events.push(format!("validate {pid}"));
        self.identities
            .pop_front()
            .unwrap()
            .map(|started| HostIdentity {
                started: started.to_owned(),
                command: HOST_NAME.to_owned(),
            })
    }
    fn signal(&mut self, pid: u32, signal: &'static str) -> Result<(), NativeError> {
        self.events.push(format!("signal {pid} {signal}"));
        if self.signal_fails {
            Err(failure("signal failed"))
        } else {
            Ok(())
        }
    }
    fn wait_for_exit(&mut self, pid: u32) -> Result<bool, NativeError> {
        self.events.push(format!("wait {pid}"));
        self.exits.pop_front().unwrap()
    }
}

fn run(upgrade: bool, hosts: &mut Hosts, ready_fails_once: bool) -> Vec<&'static str> {
    let events = RefCell::new(Vec::new());
    let mut failed = false;
    let bundles = [PathBuf::from(HAL_PATH)];
    let captured = upgrade
        .then(|| capture_hosts(hosts, &bundles).ok())
        .flatten();
    reload_with_readiness(
        captured.as_ref(),
        &bundles,
        hosts,
        || {
            events.borrow_mut().push("full");
            Ok(())
        },
        || {
            events.borrow_mut().push("ready");
            if ready_fails_once && !failed {
                failed = true;
                Err(failure("not ready"))
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    events.into_inner()
}

#[test]
fn upgrade_restarts_valid_host_and_fresh_install_never_queries_processes() {
    let mut hosts = Hosts::valid();
    assert_eq!(run(true, &mut hosts, false), ["ready"]);
    assert_eq!(
        hosts.events,
        [
            "discover",
            "validate 42",
            "discover",
            "validate 42",
            "validate 42",
            "signal 42 -TERM",
            "wait 42"
        ]
    );
    let mut hosts = Hosts::valid();
    assert_eq!(run(false, &mut hosts, false), ["full", "ready"]);
    assert!(hosts.events.is_empty());
}

#[test]
fn absent_or_uncertain_hosts_fall_back_without_signalling() {
    for pids in [Ok(Vec::new()), Err(failure("discovery failed"))] {
        let mut hosts = Hosts::valid();
        hosts.pids = pids;
        assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
        assert_eq!(hosts.events, ["discover"]);
    }
    let mut hosts = Hosts::valid();
    hosts.identities[0] = Err(failure("mapping mismatch"));
    assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
    assert_eq!(hosts.events, ["discover", "validate 42"]);
}

#[test]
fn every_candidate_is_validated_before_signalling_any_host() {
    let mut hosts = Hosts::valid();
    hosts.pids = Ok(vec![42, 43]);
    hosts.identities[1] = Err(failure("second mapping mismatch"));
    assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
    assert_eq!(hosts.events, ["discover", "validate 42", "validate 43"]);
    let mut hosts = Hosts::valid();
    hosts.pids = Ok(vec![42, 43]);
    hosts.identities = VecDeque::from([
        Ok("generation-a"),
        Ok("generation-a"),
        Ok("generation-a"),
        Err(failure("second mapping changed after commit")),
    ]);
    assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
    assert!(!hosts.events.iter().any(|event| event.starts_with("signal")));
    let mut hosts = Hosts::valid();
    hosts.pids = Ok(vec![42, 43]);
    hosts.identities = VecDeque::from([const { Ok("generation-a") }; 6]);
    hosts.exits.push_back(Ok(true));
    assert_eq!(run(true, &mut hosts, false), ["ready"]);
    assert_eq!(
        hosts.events,
        [
            "discover",
            "validate 42",
            "validate 43",
            "discover",
            "validate 42",
            "validate 43",
            "validate 42",
            "signal 42 -TERM",
            "wait 42",
            "validate 43",
            "signal 43 -TERM",
            "wait 43"
        ]
    );
}

#[test]
fn ignored_term_revalidates_before_kill_and_confirms_exit() {
    let mut hosts = Hosts::valid();
    hosts.identities.push_back(Ok("generation-a"));
    hosts.exits = VecDeque::from([Ok(false), Ok(true)]);
    assert_eq!(run(true, &mut hosts, false), ["ready"]);
    assert_eq!(
        hosts.events,
        [
            "discover",
            "validate 42",
            "discover",
            "validate 42",
            "validate 42",
            "signal 42 -TERM",
            "wait 42",
            "validate 42",
            "signal 42 -KILL",
            "wait 42"
        ]
    );
}

#[test]
fn pid_reuse_or_failed_identity_recheck_never_signals_the_replacement() {
    // Batch validation, immediately before TERM, and immediately before KILL.
    for recheck in [1, 2, 3] {
        for identity in [Ok("generation-b"), Err(failure("replaced host"))] {
            let mut hosts = Hosts::valid();
            if recheck < 3 {
                hosts.identities[recheck] = identity;
            } else {
                hosts.identities.push_back(identity);
                hosts.exits[0] = Ok(false);
            }
            assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
            assert!(!hosts.events.iter().any(|event| event.contains("-KILL")));
            assert_eq!(
                hosts.events.iter().any(|event| event.contains("-TERM")),
                recheck == 3
            );
        }
    }
}

#[test]
fn signal_failure_exit_query_failure_or_unconfirmed_termination_falls_back() {
    for mode in ["signal", "query", "timeout"] {
        let mut hosts = Hosts::valid();
        match mode {
            "signal" => hosts.signal_fails = true,
            "query" => hosts.exits[0] = Err(failure("exit query failed")),
            _ => {
                hosts.identities.push_back(Ok("generation-a"));
                hosts.exits = VecDeque::from([Ok(false), Ok(false)]);
            }
        }
        assert_eq!(run(true, &mut hosts, false), ["full", "ready"]);
    }
}

#[test]
fn targeted_readiness_failure_reloads_full_and_repeats_readiness() {
    assert_eq!(
        run(true, &mut Hosts::valid(), true),
        ["ready", "full", "ready"]
    );
}

#[test]
fn full_reload_and_final_readiness_errors_remain_transaction_failures() {
    let mut hosts = Hosts::valid();
    assert!(
        reload_with_readiness(
            None,
            &[],
            &mut hosts,
            || Err(failure("reload failed")),
            || panic!("readiness after failed reload")
        )
        .is_err()
    );
    let bundles = [PathBuf::from(HAL_PATH)];
    let captured = capture_hosts(&mut hosts, &bundles).unwrap();
    assert!(
        reload_with_readiness(
            Some(&captured),
            &bundles,
            &mut hosts,
            || Ok(()),
            || Err(failure("not ready"))
        )
        .is_err()
    );
}

fn output(code: i32, stdout: &str) -> ChildOutput {
    ChildOutput {
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn identity_output() -> String {
    format!("Wed Oct 14 12:34:56 2026 {HOST_NAME}\n")
}

fn mapping_output() -> String {
    format!("p42\nn{HAL_PATH}/Contents/MacOS/MixRelayHAL\n")
}

struct TransactionFixture {
    _owner: mixrelay_test_support::FixtureDir,
    adapter: crate::FilesystemAdapter,
    request: crate::TransactionRequest,
    snapshot: crate::FilesystemSnapshot,
    staged: crate::FilesystemStaged,
}

impl TransactionFixture {
    fn new() -> Self {
        use std::{fs, os::unix::fs::PermissionsExt};
        let owner = mixrelay_test_support::FixtureDir::new("host-reload-transaction").unwrap();
        let layout = crate::FilesystemLayout::new(owner.path().to_path_buf()).unwrap();
        fs::create_dir_all(owner.join(crate::LIFECYCLE_DIRECTORY.trim_start_matches('/'))).unwrap();
        let app = owner.join("sources/app");
        let hal = owner.join("sources/hal");
        let driver = owner.join("sources/driver");
        let plist = owner.join("sources/launchd");
        let remover = owner.join("sources/remover");
        let removal_plist = owner.join("sources/removal-launchd");
        let engine = owner.join("sources/engine");
        let engine_plist = owner.join("sources/engine-launchd");
        for (path, bytes) in [
            (app.join("Contents/MacOS/mixrelay-desktop"), "new-app"),
            (app.join("Contents/Info.plist"), "app-info"),
            (hal.join("Contents/MacOS/MixRelayHAL"), "new-hal"),
            (hal.join("Contents/Info.plist"), "hal-info"),
            (driver.clone(), "new-driver"),
            (plist.clone(), "new-plist"),
            (
                remover.join("Contents/MacOS/mixrelay-driver-uninstaller"),
                "new-remover",
            ),
            (remover.join("Contents/Info.plist"), "remover-info"),
            (removal_plist.clone(), "new-removal-plist"),
            (engine.clone(), "new-engine"),
            (engine_plist.clone(), "new-engine-plist"),
            (
                layout
                    .target_path(crate::OwnedTarget::Hal)
                    .join("Contents/MacOS/MixRelayHAL"),
                "old-hal",
            ),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let request = crate::TransactionRequest::install(crate::InstallSources::new(
            app,
            hal,
            (driver, plist),
            (remover, removal_plist),
            (engine, engine_plist),
        ));
        let adapter = crate::FilesystemAdapter::new(layout);
        let snapshot = adapter.preflight(&request).unwrap();
        let staged = adapter.stage(&request, &snapshot).unwrap();
        Self {
            _owner: owner,
            adapter,
            request,
            snapshot,
            staged,
        }
    }

    fn paths(&self) -> [PathBuf; 2] {
        [
            self.adapter.layout().target_path(crate::OwnedTarget::Hal),
            self.staged.backup_path(crate::OwnedTarget::Hal),
        ]
    }

    fn commit(&self) -> crate::FilesystemCommitted {
        let committed = self
            .adapter
            .commit(&self.request, &self.snapshot, &self.staged)
            .unwrap();
        let [original, backup] = self.paths();
        assert_eq!(
            std::fs::read_to_string(original.join("Contents/MacOS/MixRelayHAL")).unwrap(),
            "new-hal"
        );
        assert_eq!(
            std::fs::read_to_string(backup.join("Contents/MacOS/MixRelayHAL")).unwrap(),
            "old-hal"
        );
        committed
    }
}

#[test]
fn real_commit_accepts_only_captured_generations_at_exact_transaction_paths() {
    for scenario in [
        "backup",
        "original",
        "unrelated",
        "other-transaction",
        "new-pid",
        "extra-pid",
        "reuse",
        "comm",
    ] {
        let fixture = TransactionFixture::new();
        let paths = fixture.paths();
        let mapping = RefCell::new(paths[0].join("Contents/MacOS/MixRelayHAL"));
        let discovery = RefCell::new("42\n".to_owned());
        let identity = RefCell::new(identity_output());
        let signals = RefCell::new(Vec::new());
        let mut hosts = CommandHosts(|program, args: &[&str]| {
            Ok(match program {
                PGREP => output(0, &discovery.borrow()),
                PS if args.last() == Some(&"pid=") => output(1, ""),
                PS => output(0, &identity.borrow()),
                LSOF => output(0, &format!("p42\nn{}\n", mapping.borrow().display())),
                KILL => {
                    signals.borrow_mut().push(args.join(" "));
                    output(0, "")
                }
                _ => panic!("unexpected command"),
            })
        });
        // Snapshot verifies only the installed bundle, before the actual rename.
        let captured = capture_hosts(&mut hosts, &paths[..1]).unwrap();
        fixture.commit();
        *mapping.borrow_mut() = paths[1].join("Contents/MacOS/MixRelayHAL");
        match scenario {
            "original" => *mapping.borrow_mut() = paths[0].join("Contents/MacOS/MixRelayHAL"),
            "unrelated" => {
                *mapping.borrow_mut() =
                    PathBuf::from("/unrelated/MixRelayHAL.driver/Contents/MacOS/MixRelayHAL");
            }
            "other-transaction" => {
                *mapping.borrow_mut() = paths[1]
                    .parent()
                    .unwrap()
                    .join("other/hal/Contents/MacOS/MixRelayHAL");
            }
            "new-pid" => *discovery.borrow_mut() = "43\n".to_owned(),
            "extra-pid" => *discovery.borrow_mut() = "42\n43\n".to_owned(),
            "reuse" => *identity.borrow_mut() = format!("Wed Oct 14 12:34:57 2026 {HOST_NAME}\n"),
            "comm" => *identity.borrow_mut() = "Wed Oct 14 12:34:56 2026 coreaudiod\n".to_owned(),
            _ => {}
        }
        let full = Cell::new(false);
        reload_with_readiness(
            Some(&captured),
            &paths,
            &mut hosts,
            || {
                full.set(true);
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        let accepted = matches!(scenario, "backup" | "original");
        assert_eq!(full.get(), !accepted, "{scenario}");
        assert_eq!(
            *signals.borrow(),
            if accepted {
                vec!["-TERM 42".to_owned()]
            } else {
                vec![]
            },
            "{scenario}"
        );
        fixture.adapter.cleanup(&fixture.staged).unwrap();
    }
}

#[test]
fn backup_mapping_cannot_authorize_a_preflight_capture() {
    let fixture = TransactionFixture::new();
    let paths = fixture.paths();
    let mut hosts = CommandHosts(|program, _: &[&str]| {
        Ok(match program {
            PGREP => output(0, "42\n"),
            PS => output(0, &identity_output()),
            LSOF => output(
                0,
                &format!(
                    "n{}\n",
                    paths[1].join("Contents/MacOS/MixRelayHAL").display()
                ),
            ),
            _ => panic!("capture must not signal"),
        })
    });
    assert!(capture_hosts(&mut hosts, &paths[..1]).is_err());
    fixture.adapter.cleanup(&fixture.staged).unwrap();
}

#[test]
fn recovery_captures_before_real_file_rollback_and_revalidates_after_it() {
    for scenario in [
        "new-at-original",
        "old-at-backup",
        "deleted-new",
        "new-pid",
        "reuse",
    ] {
        let fixture = TransactionFixture::new();
        let paths = fixture.paths();
        let committed = fixture.commit();
        let mapping = RefCell::new(
            paths[usize::from(scenario == "old-at-backup")]
                .join("Contents/MacOS/MixRelayHAL")
                .display()
                .to_string(),
        );
        let signals = Cell::new(0);
        let discovery = RefCell::new("42\n".to_owned());
        let identity = RefCell::new(identity_output());
        let mut hosts = CommandHosts(|program, args: &[&str]| {
            Ok(match program {
                PGREP => output(0, &discovery.borrow()),
                PS if args.last() == Some(&"pid=") => output(1, ""),
                PS => output(0, &identity.borrow()),
                LSOF => output(0, &format!("n{}\n", mapping.borrow())),
                KILL => {
                    signals.set(signals.get() + 1);
                    output(0, "")
                }
                _ => panic!("unexpected command"),
            })
        });
        let captured = capture_hosts(&mut hosts, &paths).unwrap();
        fixture
            .adapter
            .rollback(
                &fixture.request,
                &fixture.snapshot,
                &fixture.staged,
                Some(&committed),
            )
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(paths[0].join("Contents/MacOS/MixRelayHAL")).unwrap(),
            "old-hal"
        );
        *mapping.borrow_mut() = format!(
            "{}{}",
            paths[0].join("Contents/MacOS/MixRelayHAL").display(),
            if scenario == "deleted-new" {
                " (deleted)"
            } else {
                ""
            }
        );
        if scenario == "new-pid" {
            *discovery.borrow_mut() = "43\n".to_owned();
        } else if scenario == "reuse" {
            *identity.borrow_mut() = format!("Wed Oct 14 12:34:57 2026 {HOST_NAME}\n");
        }
        let full = Cell::new(false);
        reload_with_readiness(
            Some(&captured),
            &paths,
            &mut hosts,
            || {
                full.set(true);
                Ok(())
            },
            || Ok(()),
        )
        .unwrap();
        let accepted = matches!(scenario, "new-at-original" | "old-at-backup");
        assert_eq!(full.get(), !accepted, "{scenario}");
        assert_eq!(signals.get(), u32::from(accepted), "{scenario}");
        fixture.adapter.cleanup(&fixture.staged).unwrap();
    }
}

#[test]
fn command_adapter_uses_absolute_paths_and_exact_name_and_mapping() {
    let mut steps = VecDeque::from([
        (PGREP, vec!["-x", "-f", HOST_PATTERN], output(0, "42\n")),
        (
            PS,
            vec!["-p", "42", "-o", "lstart=,comm="],
            output(0, &identity_output()),
        ),
        (
            LSOF,
            vec!["-a", "-p", "42", "-d", "txt", "-Fn"],
            output(0, &mapping_output()),
        ),
        (KILL, vec!["-TERM", "42"], output(0, "")),
    ]);
    let mut hosts = CommandHosts(|program, args: &[&str]| {
        let (expected, arguments, output) = steps.pop_front().unwrap();
        assert_eq!(program, expected);
        assert_eq!(args, arguments);
        Ok(output)
    });
    assert_eq!(hosts.candidates().unwrap(), [42]);
    assert_eq!(
        hosts.identity(42, &[PathBuf::from(HAL_PATH)]).unwrap(),
        HostIdentity {
            started: "Wed Oct 14 12:34:56 2026".to_owned(),
            command: HOST_NAME.to_owned()
        }
    );
    hosts.signal(42, "-TERM").unwrap();
    assert!(steps.is_empty());
}

#[test]
fn discovery_and_identity_reject_malformed_or_ambiguous_output() {
    for text in ["bad", "0", "42\n42", "42 extra"] {
        assert!(
            CommandHosts(|_, _: &[&str]| Ok(output(0, text)))
                .candidates()
                .is_err()
        );
    }
    // pgrep exits 1 without output when no host is loaded; anything else fails closed.
    assert!(
        CommandHosts(|_, _: &[&str]| Ok(output(1, "")))
            .candidates()
            .unwrap()
            .is_empty()
    );
    assert!(
        CommandHosts(|_, _: &[&str]| Ok(output(2, "")))
            .candidates()
            .is_err()
    );
    for text in [
        "short",
        "Wed Oct 14 12:34:56 2026 coreaudiod",
        "Wed Oct 14 12:34:56 2026 Core Audio Driver (MixRelayHAL.driver) ",
        "ééééééééééé界",
    ] {
        assert!(
            CommandHosts(|_, _: &[&str]| Ok(output(0, text)))
                .identity(42, &[PathBuf::from(HAL_PATH)])
                .is_err()
        );
    }
    for mapping in [
        "",
        "n/other/Contents/MacOS/MixRelayHAL",
        &format!("n{HAL_PATH}/Contents/MacOS/MixRelayHAL (deleted)"),
        &format!("n{HAL_PATH}.backup/Contents/MacOS/MixRelayHAL"),
    ] {
        let identity = identity_output();
        let mut hosts = CommandHosts(|program, _: &[&str]| {
            Ok(output(0, if program == PS { &identity } else { mapping }))
        });
        assert!(hosts.identity(42, &[PathBuf::from(HAL_PATH)]).is_err());
    }
}

#[test]
fn command_errors_and_unexpected_signal_output_fail_closed() {
    for value in [
        ChildOutput {
            stdout: vec![0xff],
            ..output(0, "")
        },
        ChildOutput {
            stderr: b"warning".to_vec(),
            ..output(0, "")
        },
        output(1, ""),
    ] {
        assert!(successful_text(&value).is_err());
    }
    assert!(
        CommandHosts(|_, _: &[&str]| Err(failure("spawn failed")))
            .candidates()
            .is_err()
    );
    assert!(
        CommandHosts(|_, _: &[&str]| Ok(output(0, "unexpected")))
            .signal(42, "-TERM")
            .is_err()
    );
    for pid in ["", "0", "1", "-1", "2147483648", "4294967296", "42 extra"] {
        assert!(parse_pid(pid).is_err());
    }
    assert_eq!(parse_pid("42").unwrap(), 42);
}

#[test]
fn exit_confirmation_is_bounded_and_never_accepts_uncertainty_as_exit() {
    let clock = Cell::new(Instant::now());
    let polls = Cell::new(0);
    let mut command = |program, args: &[&str]| {
        assert_eq!(program, PS);
        assert_eq!(args, ["-p", "42", "-o", "pid="]);
        polls.set(polls.get() + 1);
        Ok(output(0, "42\n"))
    };
    assert!(
        !wait_for_exit(
            &mut command,
            42,
            || clock.get(),
            |duration| clock.set(clock.get() + duration)
        )
        .unwrap()
    );
    assert_eq!(polls.get(), 31);
    assert!(
        wait_for_exit(
            &mut |_, _: &[&str]| Ok(output(1, "")),
            42,
            Instant::now,
            |_| panic!("already exited")
        )
        .unwrap()
    );
    for value in [
        output(0, "43"),
        output(0, ""),
        output(2, ""),
        ChildOutput {
            stderr: b"denied".to_vec(),
            ..output(1, "")
        },
    ] {
        let mut value = Some(value);
        assert!(
            wait_for_exit(
                &mut |_, _: &[&str]| Ok(value.take().unwrap()),
                42,
                Instant::now,
                |_| panic!("error cannot be retried")
            )
            .is_err()
        );
    }
    assert!(
        wait_for_exit(
            &mut |_, _: &[&str]| Err(failure("spawn failed")),
            42,
            Instant::now,
            |_| panic!("query failed")
        )
        .is_err()
    );
}
