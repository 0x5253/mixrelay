use super::*;
use mixrelay_test_support::FixtureDir;

#[test]
fn each_os_uses_its_per_user_cache_without_fallback() {
    let base = FixtureDir::new("processor-cache-layout").unwrap();
    for (os, key, suffix) in [
        ("macos", "HOME", "Library/Caches/MixRelay"),
        ("linux", "HOME", ".cache/MixRelay"),
    ] {
        assert_eq!(
            cache_directory_for(os, |name| (name == key)
                .then(|| base.as_os_str().to_owned()))
            .unwrap(),
            base.join(suffix)
        );
        assert_eq!(
            cache_directory_for(os, |_| None).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            cache_directory_for(os, |name| (name == key).then(|| OsString::from("relative")))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }
    assert_eq!(
        cache_directory_for("linux", |key| (key == "XDG_CACHE_HOME")
            .then(|| base.as_os_str().to_owned()))
        .unwrap(),
        base.join("MixRelay")
    );
    assert_eq!(
        cache_directory_for("linux", |key| Some(if key == "XDG_CACHE_HOME" {
            OsString::new()
        } else {
            base.as_os_str().to_owned()
        }))
        .unwrap(),
        base.join(".cache/MixRelay")
    );
    assert!(cache_directory_for("linux", |_| Some(OsString::from("relative"))).is_err());
    assert_eq!(
        cache_directory_for("unknown", |_| None).unwrap_err().kind(),
        io::ErrorKind::Unsupported
    );
}

#[test]
fn owned_namespaces_are_unique_private_and_removed_after_the_last_owner() {
    let parent = FixtureDir::new("processor-storage").unwrap();
    let first = WorkerPaths::new_in(&parent).unwrap();
    let second = WorkerPaths::new_in(&parent).unwrap();
    assert_ne!(first.shared(), second.shared());
    assert_eq!(first.shared().parent(), first.config().parent());
    let directory = first.shared().parent().unwrap().to_owned();
    let retained = first.directory();
    drop(first);
    assert!(directory.is_dir());
    drop(retained);
    assert!(!directory.exists());
    let owned = FixtureDir::new("processor-private-cache").unwrap();
    let directory = owned.join("workers");
    private_directory(&directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        private_directory(&directory).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let link = owned.join("link");
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        assert_eq!(
            private_directory(&link).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    let file = owned.join("file");
    fs::write(&file, b"existing").unwrap();
    assert!(private_directory(&file).is_err());
}

#[test]
fn retained_latency_mapping_keeps_the_namespace_until_its_last_view_closes() {
    let parent = FixtureDir::new("processor-reader-path").unwrap();
    let paths = WorkerPaths::new_in(&parent).unwrap();
    let directory = paths.shared().parent().unwrap().to_path_buf();
    let shared_path = paths.shared().to_path_buf();
    let (audio, mailbox) = crate::shared_transport::create_owned(
        &paths,
        crate::AudioLayout {
            inputs: 2,
            outputs: 2,
            block_frames: 128,
        },
        crate::instance_id::InstanceId::new(1).unwrap(),
    )
    .unwrap();
    let control = Arc::new(crate::control::HostedProcessorControl::native(
        mailbox,
        Arc::new(mixrelay_time::ManualClock::default()),
    ));
    let reader: Arc<dyn mixrelay_audio_engine::HostedLatencySource> = control.clone();
    drop(paths);
    drop(audio);
    drop(control);
    assert!(shared_path.is_file());
    assert!(directory.is_dir());
    drop(reader);
    assert!(!directory.exists());
}
