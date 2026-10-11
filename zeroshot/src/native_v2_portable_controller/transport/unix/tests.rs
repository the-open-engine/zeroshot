use std::fs;
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;
use tokio::time::timeout;

use super::*;
use crate::native_v2_portable_controller::PortableControllerPaths;

#[test]
fn socket_path_preserves_fitting_paths_and_shortens_at_the_native_byte_limit() {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let capacity = address.sun_path.len();
    let storage = PathBuf::from(format!(
        "/{}",
        "x".repeat(capacity - 2 - "/controller.sock".len())
    ));
    let paths = PortableControllerPaths::new(&storage);
    assert_eq!(paths.socket(), storage.join("controller.sock"));
    assert_eq!(
        paths.socket().as_os_str().as_encoded_bytes().len(),
        capacity - 1
    );

    let long_storage = PathBuf::from(format!("{}x", storage.display()));
    let long = PortableControllerPaths::new(&long_storage);
    let socket = long.socket();
    assert_ne!(socket, long_storage.join("controller.sock"));
    assert!(socket.as_os_str().as_encoded_bytes().len() < 104);
    assert_eq!(socket, PortableControllerPaths::new(&long_storage).socket());
    let other = PortableControllerPaths::new(long_storage.join("other-run")).socket();
    assert_ne!(socket, other);
    assert_ne!(
        socket.parent(),
        other.parent(),
        "each run has its own temporary directory"
    );
    assert_ne!(
        socket.parent().assert_value(),
        Path::new(&format!("/tmp/zeroshot-{}", unsafe { libc::geteuid() }))
    );
    let decorated = PathBuf::from(format!(
        "/./{}//./",
        long_storage.strip_prefix("/").assert_value().display()
    ));
    assert_eq!(socket, PortableControllerPaths::new(decorated).socket());
    let padded_short = PathBuf::from(format!(
        "/{}{}//",
        "./".repeat(capacity),
        storage.strip_prefix("/").assert_value().display()
    ));
    assert_eq!(
        PortableControllerPaths::new(padded_short).socket(),
        storage.join("controller.sock")
    );
    assert_eq!(long.storage(), long_storage);
    assert_eq!(long.ledger(), long_storage.join("runs.sqlite3"));

    let unicode_storage = PathBuf::from(format!("/{}", "é".repeat(capacity / 2)));
    assert!(unicode_storage.to_str().assert_value().chars().count() < capacity);
    assert_ne!(
        PortableControllerPaths::new(&unicode_storage).socket(),
        unicode_storage.join("controller.sock")
    );
    let null_storage = Path::new("/state\0invalid");
    assert_eq!(
        PortableControllerPaths::new(null_storage).socket(),
        null_storage.join("controller.sock")
    );
}

#[tokio::test]
async fn long_storage_socket_is_private_reconnectable_and_removed_when_listener_drops() {
    let root = tempfile::tempdir().assert_value();
    let paths = PortableControllerPaths::new(root.path().join("long-state-directory".repeat(8)));
    let socket = paths.socket();
    let listener = Listener::bind(&socket).assert_value();
    let metadata = fs::metadata(socket.parent().assert_value()).assert_value();
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::metadata(&socket).assert_value().permissions().mode() & 0o777,
        0o600
    );

    for _ in 0..2 {
        let (client, accepted) = timeout(Duration::from_secs(1), async {
            tokio::join!(connect(&socket), listener.accept())
        })
        .await
        .assert_value();
        client.assert_value();
        accepted.assert_value();
    }
    drop(listener);
    assert!(!socket.exists());
    assert!(!socket.parent().assert_value().exists());

    crate::execution::platform::create_private_directory(socket.parent().assert_value())
        .assert_value();
    let stale = std::os::unix::net::UnixListener::bind(&socket).assert_value();
    drop(stale);
    assert!(socket.exists());
    drop(Listener::bind(&socket).assert_value());
    assert!(!socket.exists());
}

#[tokio::test]
async fn socket_operations_reject_public_or_symlinked_directories_without_changing_them() {
    let root = tempfile::tempdir_in("/tmp").assert_value();
    let public = root.path().join("public");
    fs::create_dir(&public).assert_value();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).assert_value();
    let socket = public.join("controller.sock");
    let existing = std::os::unix::net::UnixListener::bind(&socket).assert_value();
    assert!(Listener::bind(&socket).is_err());
    assert!(connect(&socket).await.is_err());
    assert!(remove_endpoint(&socket).is_err());
    assert!(socket.exists());
    assert_eq!(
        fs::metadata(&public).assert_value().permissions().mode() & 0o777,
        0o755
    );
    drop(existing);

    let link = root.path().join("link");
    std::os::unix::fs::symlink(root.path(), &link).assert_value();
    let linked_socket = link.join("controller.sock");
    assert!(Listener::bind(&linked_socket).is_err());
    assert!(connect(&linked_socket).await.is_err());
    assert!(remove_endpoint(&linked_socket).is_err());
    assert!(!root.path().join("controller.sock").exists());
}

#[tokio::test]
async fn socket_bind_and_cleanup_preserve_non_socket_entries() {
    let root = tempfile::tempdir_in("/tmp").assert_value();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).assert_value();
    let path = root.path().join("controller.sock");
    fs::write(&path, "keep this file").assert_value();
    assert!(Listener::bind(&path).is_err());
    assert!(remove_endpoint(&path).is_err());
    assert_eq!(fs::read_to_string(&path).assert_value(), "keep this file");

    fs::remove_file(&path).assert_value();
    let listener = Listener::bind(&path).assert_value();
    fs::remove_file(&path).assert_value();
    fs::write(&path, "replacement").assert_value();
    drop(listener);
    assert_eq!(fs::read_to_string(path).assert_value(), "replacement");
}

#[tokio::test]
async fn retired_listener_cleanup_preserves_a_replacement_socket() {
    let root = tempfile::tempdir_in("/tmp").assert_value();
    crate::execution::platform::private_directory(root.path()).assert_value();
    let socket = root.path().join("controller.sock");
    let retired = Listener::bind(&socket).assert_value();
    let replacement = Listener::bind(&socket).assert_value();
    drop(retired);
    assert!(
        socket.exists(),
        "retired listener must preserve the replacement"
    );

    let (client, accepted) = timeout(Duration::from_secs(1), async {
        tokio::join!(connect(&socket), replacement.accept())
    })
    .await
    .assert_value();
    client.assert_value();
    accepted.assert_value();
    drop(replacement);
    assert!(!socket.exists());
}
