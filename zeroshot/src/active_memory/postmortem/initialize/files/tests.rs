#[cfg(windows)]
use super::*;

#[cfg(windows)]
#[test]
fn windows_initialization_creates_private_files_and_rejects_a_public_session_directory() {
    use crate::execution::platform::{self, FileAccess};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("session");
    create_directory(&directory).unwrap();
    platform::validate_private_file(&platform::open_directory(&directory).unwrap()).unwrap();
    let document = directory.join("state.json");
    write(&document, &serde_json::json!({"private":"fixture"})).unwrap();
    drop(platform::private_file(&document, FileAccess::Read).unwrap());
    assert_eq!(
        read::<serde_json::Value>(&document).unwrap()["private"],
        "fixture"
    );
    assert!(
        std::process::Command::new("icacls.exe")
            .arg(&directory)
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(private_directory(&directory).is_err());
}
