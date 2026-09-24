use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::linux::LinuxTargetCredentialStore;
use super::private_file::PrivateFileTargetCredentialStore;
use super::test_support::{MemoryCredentialStore, UnavailableCredentialStore};
use super::{
    CredentialStorePreparation, KeyringTargetCredentialStore, TargetCredentialStore,
    credential_service, open_refresh_lock, refresh_lock_is_held,
};

const TARGET_ID: &str = "11111111-1111-4111-8111-111111111111";

async fn prepare_and_store(store: &dyn TargetCredentialStore) {
    assert!(matches!(
        store.prepare_for_login(TARGET_ID).await.assert_value(),
        CredentialStorePreparation::PrivateFile(_)
    ));
    store.set(TARGET_ID, "refresh-token").await.assert_value();
}

async fn assert_private_credential_rejected(
    store: &PrivateFileTargetCredentialStore,
    path: &Path,
    bytes: &[u8],
) {
    std::fs::write(path, bytes).assert_value();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).assert_value();
    assert_eq!(
        store.get(TARGET_ID).await.assert_error().to_string(),
        "private target credential store read failed"
    );
}

#[tokio::test]
async fn private_file_credentials_persist_with_private_modes() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-private-credential");
    let directory = root.path("credentials");
    let first = PrivateFileTargetCredentialStore::new(directory.clone());
    prepare_and_store(&first).await;

    let second = PrivateFileTargetCredentialStore::new(directory.clone());
    assert_eq!(
        second.get(TARGET_ID).await.assert_value().as_deref(),
        Some("refresh-token")
    );
    assert_eq!(
        std::fs::metadata(&directory)
            .assert_value()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(directory.join(format!("{TARGET_ID}.json")))
            .assert_value()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn private_file_credentials_reject_broad_permissions() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-private-credential-mode",
    );
    let directory = root.path("credentials");
    let store = PrivateFileTargetCredentialStore::new(directory.clone());
    store.set(TARGET_ID, "refresh-token").await.assert_value();
    let path = directory.join(format!("{TARGET_ID}.json"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).assert_value();

    assert_eq!(
        store.get(TARGET_ID).await.assert_error().to_string(),
        "private target credential store read failed"
    );
}

#[tokio::test]
async fn private_file_credentials_reject_symlinks_and_special_files() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-private-credential-special-file",
    );
    let directory = root.path("credentials");
    let store = PrivateFileTargetCredentialStore::new(directory.clone());
    store.prepare_for_login(TARGET_ID).await.assert_value();
    let credential = directory.join(format!("{TARGET_ID}.json"));
    let outside = root.path("outside.json");
    std::fs::write(&outside, b"not-a-credential").assert_value();
    std::os::unix::fs::symlink(&outside, &credential).assert_value();
    assert_eq!(
        store.get(TARGET_ID).await.assert_error().to_string(),
        "private target credential store read failed"
    );
    std::fs::remove_file(&credential).assert_value();

    let fifo_path = CString::new(credential.as_os_str().as_bytes()).assert_value();
    // SAFETY: fifo_path is NUL-terminated and names a path in the temporary test directory.
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), store.get(TARGET_ID))
            .await
            .assert_value()
            .assert_error()
            .to_string(),
        "private target credential store read failed"
    );
}

#[tokio::test]
async fn maximum_length_escaped_refresh_token_round_trips() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-private-credential-max-token",
    );
    let store = PrivateFileTargetCredentialStore::new(root.path("credentials"));
    let token = "\\".repeat(16 * 1024);
    store.set(TARGET_ID, &token).await.assert_value();
    assert_eq!(store.get(TARGET_ID).await.assert_value(), Some(token));
}

#[tokio::test]
async fn linux_fallback_survives_a_new_store_instance() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-linux-credential");
    let directory = root.path("credentials");
    let first = LinuxTargetCredentialStore::with_dependencies(
        directory.clone(),
        Arc::new(UnavailableCredentialStore),
        None,
        false,
    )
    .assert_value();
    prepare_and_store(&first).await;

    let second = LinuxTargetCredentialStore::with_dependencies(
        directory,
        Arc::new(MemoryCredentialStore::default()),
        None,
        true,
    )
    .assert_value();
    assert_eq!(
        second.get(TARGET_ID).await.assert_value().as_deref(),
        Some("refresh-token")
    );
}

#[tokio::test]
async fn switching_to_system_removes_the_private_file_after_storage() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-system-credential-cleanup",
    );
    let directory = root.path("credentials");
    let file_store = LinuxTargetCredentialStore::with_dependencies(
        directory.clone(),
        Arc::new(MemoryCredentialStore::default()),
        Some("file"),
        false,
    )
    .assert_value();
    prepare_and_store(&file_store).await;
    let credential = directory.join(format!("{TARGET_ID}.json"));
    assert!(credential.exists());

    let unavailable_system = LinuxTargetCredentialStore::with_dependencies(
        directory.clone(),
        Arc::new(UnavailableCredentialStore),
        Some("system"),
        false,
    )
    .assert_value();
    assert_eq!(
        unavailable_system
            .set(TARGET_ID, "unpersisted-refresh-token")
            .await
            .assert_error()
            .to_string(),
        "test credential store unavailable"
    );
    assert!(credential.exists());

    let system = Arc::new(MemoryCredentialStore::default());
    let system_store = LinuxTargetCredentialStore::with_dependencies(
        directory,
        system.clone(),
        Some("system"),
        false,
    )
    .assert_value();
    assert_eq!(
        system_store
            .prepare_for_login(TARGET_ID)
            .await
            .assert_value(),
        CredentialStorePreparation::Managed
    );
    system_store
        .set(TARGET_ID, "system-refresh-token")
        .await
        .assert_value();

    assert_eq!(
        system.get(TARGET_ID).await.assert_value().as_deref(),
        Some("system-refresh-token")
    );
    assert!(!credential.exists());
}

#[tokio::test]
async fn an_explicit_system_store_never_downgrades() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-system-credential");
    let store = LinuxTargetCredentialStore::with_dependencies(
        root.path("credentials"),
        Arc::new(UnavailableCredentialStore),
        Some("system"),
        false,
    )
    .assert_value();

    assert_eq!(
        store
            .prepare_for_login(TARGET_ID)
            .await
            .assert_error()
            .to_string(),
        "test credential store unavailable"
    );
}

async fn assert_system_selection_is_sticky(root: &openengine_cluster_testkit::TemporaryDirectory) {
    let system_directory = root.path("system-credentials");
    let system = Arc::new(MemoryCredentialStore::default());
    system
        .set(TARGET_ID, "system-refresh-token")
        .await
        .assert_value();
    let automatic_system = LinuxTargetCredentialStore::with_dependencies(
        system_directory.clone(),
        system,
        Some("auto"),
        false,
    )
    .assert_value();
    assert_eq!(
        automatic_system
            .get(TARGET_ID)
            .await
            .assert_value()
            .as_deref(),
        Some("system-refresh-token")
    );

    let restarted = LinuxTargetCredentialStore::with_dependencies(
        system_directory,
        Arc::new(UnavailableCredentialStore),
        None,
        false,
    )
    .assert_value();
    assert_eq!(
        restarted.get(TARGET_ID).await.assert_error().to_string(),
        "test credential store unavailable"
    );
}

async fn assert_automatic_backend_defaults(root: &openengine_cluster_testkit::TemporaryDirectory) {
    let desktop_directory = root.path("desktop-credentials");
    let desktop_system = Arc::new(MemoryCredentialStore::default());
    let desktop = LinuxTargetCredentialStore::with_dependencies(
        desktop_directory,
        desktop_system.clone(),
        None,
        true,
    )
    .assert_value();
    assert_eq!(
        desktop.prepare_for_login(TARGET_ID).await.assert_value(),
        CredentialStorePreparation::Managed
    );
    desktop
        .set(TARGET_ID, "desktop-refresh-token")
        .await
        .assert_value();
    assert_eq!(
        desktop_system
            .get(TARGET_ID)
            .await
            .assert_value()
            .as_deref(),
        Some("desktop-refresh-token")
    );

    let headless_directory = root.path("headless-credentials");
    let headless = LinuxTargetCredentialStore::with_dependencies(
        headless_directory,
        Arc::new(MemoryCredentialStore::default()),
        None,
        false,
    )
    .assert_value();
    assert!(matches!(
        headless.prepare_for_login(TARGET_ID).await.assert_value(),
        CredentialStorePreparation::PrivateFile(_)
    ));

    let discovered_directory = root.path("discovered-file-credentials");
    let private = PrivateFileTargetCredentialStore::new(discovered_directory.clone());
    private
        .set(TARGET_ID, "discovered-refresh-token")
        .await
        .assert_value();
    let automatic_file = LinuxTargetCredentialStore::with_dependencies(
        discovered_directory.clone(),
        Arc::new(MemoryCredentialStore::default()),
        None,
        false,
    )
    .assert_value();
    assert_eq!(
        automatic_file
            .get(TARGET_ID)
            .await
            .assert_value()
            .as_deref(),
        Some("discovered-refresh-token")
    );
    let restarted_file = LinuxTargetCredentialStore::with_dependencies(
        discovered_directory,
        Arc::new(UnavailableCredentialStore),
        None,
        true,
    )
    .assert_value();
    assert_eq!(
        restarted_file
            .get(TARGET_ID)
            .await
            .assert_value()
            .as_deref(),
        Some("discovered-refresh-token")
    );
}

async fn assert_malformed_backend_selection_is_rejected(
    root: &openengine_cluster_testkit::TemporaryDirectory,
) {
    let malformed_directory = root.path("malformed-selection");
    let malformed_private = PrivateFileTargetCredentialStore::new(malformed_directory.clone());
    malformed_private
        .prepare_for_login(TARGET_ID)
        .await
        .assert_value();
    let malformed_selection = malformed_directory.join(format!("{TARGET_ID}.store"));
    std::fs::write(&malformed_selection, "ambient\n").assert_value();
    std::fs::set_permissions(&malformed_selection, std::fs::Permissions::from_mode(0o600))
        .assert_value();
    let malformed = LinuxTargetCredentialStore::with_dependencies(
        malformed_directory,
        Arc::new(MemoryCredentialStore::default()),
        None,
        false,
    )
    .assert_value();
    assert_eq!(
        malformed.get(TARGET_ID).await.assert_error().to_string(),
        "target credential store selection is malformed"
    );

    for invalid in ["invalid", "SYSTEM", " file"] {
        assert_eq!(
            LinuxTargetCredentialStore::with_dependencies(
                root.path(&format!("invalid-{invalid:?}")),
                Arc::new(MemoryCredentialStore::default()),
                Some(invalid),
                false,
            )
            .assert_error()
            .to_string(),
            "ZEROSHOT_CREDENTIAL_STORE must be auto, system, or file"
        );
    }
}

#[tokio::test]
async fn automatic_backend_selection_is_persisted_and_never_silently_reinterpreted() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-automatic-credential-selection",
    );
    assert_system_selection_is_sticky(&root).await;
    assert_automatic_backend_defaults(&root).await;
    assert_malformed_backend_selection_is_rejected(&root).await;
}

#[test]
fn credential_identity_and_refresh_lock_fail_closed_at_the_filesystem_boundary() {
    assert_eq!(
        credential_service(TARGET_ID).assert_value(),
        format!("zeroshot-target-{TARGET_ID}")
    );
    for target_id in [
        "",
        "11111111-1111-4111-8111-11111111111",
        "11111111-1111-4111-8111-1111111111111",
        "11111111-1111-4111-8111-11111111111A",
        "../../../../../../tmp/credential",
    ] {
        assert_eq!(
            credential_service(target_id).assert_error().to_string(),
            "stored target credential identity is invalid"
        );
    }

    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-refresh-lock");
    let directory = root.path("locks");
    let path = directory.join("target.lock");
    assert!(!refresh_lock_is_held(&directory, &path).assert_value());
    let guard = open_refresh_lock(&directory, &path).assert_value();
    assert!(refresh_lock_is_held(&directory, &path).assert_value());
    drop(guard);
    assert!(!refresh_lock_is_held(&directory, &path).assert_value());
}

#[tokio::test]
async fn malformed_identity_is_rejected_before_keyring_access() {
    let store = KeyringTargetCredentialStore;
    for error in [
        store.prepare_for_login("invalid").await.unwrap_err(),
        store.get("invalid").await.unwrap_err(),
        store.set("invalid", "refresh-token").await.unwrap_err(),
    ] {
        assert_eq!(
            error.to_string(),
            "stored target credential identity is invalid"
        );
    }
}

#[test]
fn refresh_lock_errors_identify_directory_and_file_boundaries() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("zeroshot-refresh-lock-errors");
    let blocked_directory = root.path("blocked");
    std::fs::write(&blocked_directory, b"not a directory").assert_value();
    let blocked_path = blocked_directory.join("target.lock");
    for error in [
        open_refresh_lock(&blocked_directory, &blocked_path).assert_error(),
        refresh_lock_is_held(&blocked_directory, &blocked_path).assert_error(),
    ] {
        assert_eq!(
            error.to_string(),
            "target refresh lock directory is unavailable"
        );
    }

    let directory = root.path("locks");
    std::fs::create_dir(&directory).assert_value();
    let directory_path = directory.join("target.lock");
    std::fs::create_dir(&directory_path).assert_value();
    for error in [
        open_refresh_lock(&directory, &directory_path).assert_error(),
        refresh_lock_is_held(&directory, &directory_path).assert_error(),
    ] {
        assert_eq!(error.to_string(), "target refresh lock is unavailable");
    }
}

#[tokio::test]
async fn private_file_store_rejects_malformed_tokens_metadata_and_backend_selection() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-private-credential-refusals",
    );
    let directory = root.path("credentials");
    let store = PrivateFileTargetCredentialStore::new(directory.clone());
    store.prepare_for_login(TARGET_ID).await.assert_value();

    assert_eq!(store.get(TARGET_ID).await.assert_value(), None);
    assert_eq!(store.read_backend(TARGET_ID).await.assert_value(), None);
    assert_eq!(
        store
            .write_backend(TARGET_ID, "ambient\n")
            .await
            .assert_error()
            .to_string(),
        "target credential store selection is invalid"
    );
    for backend in ["system\n", "file\n"] {
        store.write_backend(TARGET_ID, backend).await.assert_value();
        assert_eq!(
            store
                .read_backend(TARGET_ID)
                .await
                .assert_value()
                .as_deref(),
            Some(backend)
        );
    }

    for token in [
        String::new(),
        "line\nbreak".to_owned(),
        "nul\0byte".to_owned(),
        "x".repeat(16 * 1024 + 1),
    ] {
        assert_eq!(
            store
                .set(TARGET_ID, &token)
                .await
                .assert_error()
                .to_string(),
            "refresh token is malformed"
        );
    }
    assert_eq!(
        store
            .get("unsafe/target-id")
            .await
            .assert_error()
            .to_string(),
        "stored target credential identity is invalid"
    );

    let credential = directory.join(format!("{TARGET_ID}.json"));
    for bytes in [
        br#"{"version":2,"refreshToken":"token"}"#.as_slice(),
        br#"{"version":1,"refreshToken":""}"#.as_slice(),
        br#"{"version":1,"refreshToken":"token","extra":true}"#.as_slice(),
        b"not-json".as_slice(),
        b"\xff\xfe".as_slice(),
    ] {
        assert_private_credential_rejected(&store, &credential, bytes).await;
    }

    let oversized = vec![b'x'; 16 * 1024 * 2 + 257];
    assert_private_credential_rejected(&store, &credential, &oversized).await;
}

#[tokio::test]
async fn private_file_cleanup_removes_only_private_regular_credentials() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "zeroshot-private-credential-cleanup",
    );
    let directory = root.path("credentials");
    let store = PrivateFileTargetCredentialStore::new(directory.clone());
    store.set(TARGET_ID, "refresh-token").await.assert_value();
    let credential = directory.join(format!("{TARGET_ID}.json"));

    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o644)).assert_value();
    assert_eq!(
        store
            .remove_credential(TARGET_ID)
            .await
            .assert_error()
            .to_string(),
        "private target credential store cleanup failed"
    );
    assert!(credential.exists());

    std::fs::set_permissions(&credential, std::fs::Permissions::from_mode(0o600)).assert_value();
    store.remove_credential(TARGET_ID).await.assert_value();
    assert!(!credential.exists());
    store.remove_credential(TARGET_ID).await.assert_value();
}
