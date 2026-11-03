use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

fn uid_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("metadata").uid()
}

#[tokio::test]
async fn owned_socket_refuses_live_replacement_and_cleans_up() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let directory = fixture.join("n/c");
    let uid = uid_of(fixture.path());
    let (listener, owner) = bind(&directory, ENGINE_SOCKET, uid).await.expect("bind");
    assert_eq!(
        std::fs::metadata(&directory).expect("directory").mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(owner.path()).expect("socket").mode() & 0o777,
        0o600
    );
    assert_eq!(
        bind(&directory, ENGINE_SOCKET, uid)
            .await
            .expect_err("live rejection")
            .kind(),
        io::ErrorKind::AddrInUse
    );
    drop(listener);
    // A dead listener's inode is recoverable, but the old guard cannot delete its replacement.
    let (replacement, next) = bind(&directory, ENGINE_SOCKET, uid)
        .await
        .expect("stale recovery");
    drop(owner);
    assert!(next.path().exists());
    drop(replacement);
    let path = next.path().to_path_buf();
    drop(next);
    assert!(!path.exists());
}

#[tokio::test]
async fn unsafe_directories_and_non_socket_targets_are_not_overwritten() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    // Exercise the unsafe mode explicitly; a private lab's inherited umask
    // otherwise creates an already-safe 0700 fixture directory.
    std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o755))
        .expect("unsafe fixture mode");
    let uid = uid_of(fixture.path());
    assert_eq!(
        bind(fixture.path(), ENGINE_SOCKET, uid)
            .await
            .expect_err("permissions")
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    let directory = fixture.join("c");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .expect("directory");
    let path = directory.join(ENGINE_SOCKET);
    std::fs::write(&path, b"user data").expect("file");
    assert!(bind(&directory, ENGINE_SOCKET, uid).await.is_err());
    assert_eq!(std::fs::read(path).expect("retained file"), b"user data");
    let link = fixture.join("link");
    std::os::unix::fs::symlink(&directory, &link).expect("symlink");
    assert!(bind(&link, ENGINE_SOCKET, uid).await.is_err());
    // Without its lock beside the socket, a service does not start.
    let locked = fixture.join("l");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&locked)
        .expect("directory");
    std::fs::create_dir(locked.join(format!("{ENGINE_SOCKET}.lock"))).expect("blocked lock");
    assert!(bind(&locked, ENGINE_SOCKET, uid).await.is_err());
    // A path longer than a socket address holds cannot be bound.
    let long = fixture.join("x".repeat(120));
    assert!(bind(&long, ENGINE_SOCKET, uid).await.is_err());
    // A directory cannot be created where a file stands.
    let blocked = fixture.join("f");
    std::fs::write(&blocked, b"").expect("file");
    assert!(bind(&blocked.join("i"), ENGINE_SOCKET, uid).await.is_err());
}

#[tokio::test]
async fn connections_from_other_users_never_reach_the_service() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let directory = fixture.join("c");
    let uid = uid_of(fixture.path());
    let (listener, socket) = bind(&directory, DRIVER_SOCKET, uid).await.expect("bind");
    let path = socket.path().to_path_buf();
    // Nobody is allowed: the connection is closed before any request is read.
    let mut refusing = AuthorizedListener::new(listener, socket, Vec::new());
    let _client = UnixStream::connect(&path).await.expect("connect");
    assert!(
        tokio::time::timeout(
            Duration::from_millis(100),
            axum::serve::Listener::accept(&mut refusing)
        )
        .await
        .is_err()
    );
    drop(refusing);
    let (listener, socket) = bind(&directory, DRIVER_SOCKET, uid).await.expect("rebind");
    let mut admitting = AuthorizedListener::new(listener, socket, vec![current_uid()]);
    assert_eq!(admitting.path(), path);
    assert_eq!(
        axum::serve::Listener::local_addr(&admitting).unwrap(),
        Peer {
            uid: current_uid(),
            audit_token: None
        }
    );
    let _client = UnixStream::connect(&path).await.expect("connect");
    let (stream, peer) = axum::serve::Listener::accept(&mut admitting).await;
    assert!(peer_allowed(&stream, &[current_uid()]));
    assert_eq!(peer.uid, current_uid());
    // The audit token names this very process on macOS.
    assert_eq!(peer.audit_token.is_some(), cfg!(target_os = "macos"));
}

#[test]
fn socket_directories_follow_the_override_then_the_platform_location() {
    assert_eq!(
        directory_from(Some("/tmp/ipc".into()), None, None).unwrap(),
        PathBuf::from("/tmp/ipc")
    );
    assert!(directory_from(Some("relative".into()), None, None).is_err());
    let session = directory_from(None, Some("/Users/a".into()), Some("/run/user/1".into()));
    #[cfg(target_os = "macos")]
    {
        assert_eq!(
            session.unwrap(),
            PathBuf::from("/Users/a/Library/Application Support/MixRelay/ipc")
        );
        assert!(directory_from(None, None, None).is_err());
        assert!(directory_from(None, Some("home".into()), None).is_err());
    }
    #[cfg(not(target_os = "macos"))]
    {
        assert_eq!(session.unwrap(), PathBuf::from("/run/user/1/mixrelay"));
        assert!(directory_from(None, None, None).is_err());
        assert!(directory_from(None, None, Some("run".into())).is_err());
    }
    // The process environment decides both; whatever it holds, they agree with the rules.
    let variable = |name| std::env::var_os(name).map(PathBuf::from);
    assert_eq!(
        socket_directory().ok(),
        directory_from(
            variable(DIRECTORY_VARIABLE),
            variable("HOME"),
            variable("XDG_RUNTIME_DIR")
        )
        .ok()
    );
    assert_eq!(
        session_socket_directory().ok(),
        directory_from(None, variable("HOME"), variable("XDG_RUNTIME_DIR")).ok()
    );
    assert_eq!(current_uid(), uid_of(Path::new(env!("CARGO_MANIFEST_DIR"))));
}

#[tokio::test]
async fn ancestors_writable_by_others_are_refused_unless_root_sticky() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let uid = uid_of(fixture.path());
    let open = fixture.join("o");
    std::fs::create_dir(&open).expect("directory");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).expect("chmod");
    let error = bind(&open.join("c"), ENGINE_SOCKET, uid)
        .await
        .expect_err("writable ancestor");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    // The system temporary directory is root-owned and sticky: others cannot move entries.
    let temporary = std::env::temp_dir().join(format!("mixrelay-ipc-{}", std::process::id()));
    let (listener, socket) = bind(&temporary, ENGINE_SOCKET, uid)
        .await
        .expect("sticky ancestor");
    drop((listener, socket));
    std::fs::remove_dir_all(temporary).expect("cleanup");
}

#[tokio::test]
async fn starters_racing_on_one_thread_both_see_a_live_socket() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let directory = fixture.join("c");
    let uid = uid_of(fixture.path());
    let _live = bind(&directory, ENGINE_SOCKET, uid).await.expect("bind");
    // A current-thread runtime: a starter waiting for the lock must not block the holder.
    let (first, second) = tokio::join!(
        bind(&directory, ENGINE_SOCKET, uid),
        bind(&directory, ENGINE_SOCKET, uid)
    );
    for result in [first, second] {
        assert_eq!(result.expect_err("live").kind(), io::ErrorKind::AddrInUse);
    }
}

#[tokio::test]
async fn concurrent_starters_never_replace_each_others_live_socket() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let directory = fixture.join("c");
    let uid = uid_of(fixture.path());
    // A stale socket: its listener is gone but the path remains.
    let (listener, socket) = bind(&directory, ENGINE_SOCKET, uid).await.expect("bind");
    drop(listener);
    std::mem::forget(socket);
    let (first, second) = tokio::join!(
        bind(&directory, ENGINE_SOCKET, uid),
        bind(&directory, ENGINE_SOCKET, uid)
    );
    let started = [first.is_ok(), second.is_ok()];
    assert_eq!(started.iter().filter(|started| **started).count(), 1);
    let failed = [first, second]
        .into_iter()
        .find_map(Result::err)
        .expect("loser");
    assert_eq!(failed.kind(), io::ErrorKind::AddrInUse);
}

#[tokio::test]
async fn handlers_see_the_peer_of_their_connection() {
    let fixture = mixrelay_test_support::FixtureDir::ipc("s").expect("fixture");
    let uid = uid_of(fixture.path());
    let (listener, socket) = bind(&fixture.join("c"), DRIVER_SOCKET, uid)
        .await
        .expect("bind");
    let path = socket.path().to_path_buf();
    let listener = AuthorizedListener::new(listener, socket, vec![current_uid()]);
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(
            |axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<Peer>| async move {
                format!("{} {}", peer.uid, peer.audit_token.is_some())
            },
        ),
    );
    let service = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<Peer>())
            .await
            .expect("serve");
    });
    let mut stream = UnixStream::connect(&path).await.expect("connect");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("response");
    assert!(
        response.ends_with(&format!("{} {}", current_uid(), cfg!(target_os = "macos"))),
        "{response}"
    );
    service.abort();
}
