use super::*;
use std::io::Write;

fn fixture() -> (tempfile::TempDir, TrustedGitHubTokenFile) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("checkout");
    std::fs::create_dir(&workspace).unwrap();
    let file = TrustedGitHubTokenFile::new(root.path().join("token"), &workspace).unwrap();
    (root, file)
}

fn issue(path: &Path, bytes: &[u8]) {
    let mut file = private_file(path, FileAccess::CreateNew).unwrap();
    file.write_all(bytes).unwrap();
}

#[test]
fn token_file_reopens_an_atomically_replaced_private_issuance() {
    let (root, file) = fixture();
    issue(file.path(), b"first-token\n");
    assert_eq!(file.read().unwrap(), "first-token");
    let replacement = root.path().join("replacement");
    issue(&replacement, b"second-token\r\n");
    std::fs::rename(replacement, file.path()).unwrap();
    assert_eq!(file.read().unwrap(), "second-token");
    assert!(!format!("{file:?}").contains(&root.path().to_string_lossy().to_string()));
}

#[test]
fn token_file_rejects_empty_multiline_oversized_or_non_token_bytes() {
    for bytes in [
        b"".as_slice(),
        b" ",
        b"token\nextra",
        b"token ",
        b"\xff",
        b"token\0",
    ] {
        let (_root, file) = fixture();
        issue(file.path(), bytes);
        assert_eq!(file.read(), Err(EnvironmentRefreshError::InvalidResponse));
    }
    let (_root, file) = fixture();
    issue(file.path(), &vec![b'x'; super::super::MAX_TOKEN_BYTES + 1]);
    assert_eq!(file.read(), Err(EnvironmentRefreshError::InvalidResponse));
}

#[test]
fn token_file_missing_is_unavailable_and_checkout_paths_are_rejected() {
    let (root, file) = fixture();
    assert_eq!(file.read(), Err(EnvironmentRefreshError::Unavailable));
    let workspace = root.path().join("checkout");
    assert!(TrustedGitHubTokenFile::new(workspace.join("token"), &workspace).is_err());
    assert!(TrustedGitHubTokenFile::new("relative".into(), &workspace).is_err());
}

#[cfg(unix)]
#[test]
fn token_file_rejects_symlinks_and_public_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (root, file) = fixture();
    let issuance = root.path().join("actual");
    issue(&issuance, b"private-token");
    symlink(&issuance, file.path()).unwrap();
    assert!(file.read().is_err());
    std::fs::remove_file(file.path()).unwrap();
    issue(file.path(), b"private-token");
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(file.read(), Err(EnvironmentRefreshError::Refused));
    let checkout_alias = root.path().join("checkout-alias");
    symlink(root.path().join("checkout"), &checkout_alias).unwrap();
    assert!(
        TrustedGitHubTokenFile::new(checkout_alias.join("token"), &root.path().join("checkout"))
            .is_err()
    );
}
