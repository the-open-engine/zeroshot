#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn write_private_fixture(
    root: &openengine_cluster_testkit::TemporaryDirectory,
    name: &str,
    contents: &[u8],
) -> std::path::PathBuf {
    let path = root.path(name);
    std::fs::write(&path, contents).expect("write bootstrap fixture");
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("make bootstrap fixture private");
    path
}

#[test]
fn malformed_bootstrap_material_is_rejected_at_each_security_boundary() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "private-bootstrap-invalid-material",
    );
    assert!(TargetBootstrapKey::load_and_unlink(&root.path("missing")).is_err());
    assert!(read_private_key(root.as_path()).is_err());

    let non_utf8 = [0xff; KEY_BYTES * 2];
    let uppercase = [b'A'; KEY_BYTES * 2];
    for (name, contents) in [
        ("short", b"00".as_slice()),
        ("non-utf8", non_utf8.as_slice()),
        ("uppercase", uppercase.as_slice()),
    ] {
        let path = write_private_fixture(&root, name, contents);
        assert!(read_private_key(&path).is_err(), "accepted {name}");
    }

    let request = TargetPrivateBootstrapRequest {
        nonce: encode_lower(&[0; NONCE_BYTES]),
        ciphertext: "00".to_owned(),
    };
    assert!(TargetBootstrapKey([0; KEY_BYTES]).open(&request).is_err());
    let request = TargetPrivateBootstrapRequest {
        nonce: "00".to_owned(),
        ciphertext: encode_lower(&[0; TOKEN_BYTES + TAG_BYTES]),
    };
    assert!(TargetBootstrapKey([0; KEY_BYTES]).open(&request).is_err());
    assert!(PrivateTargetToken::parse(&[b'A'; TOKEN_BYTES]).is_err());
    assert!(decode_lower_vec("0").is_err());
    assert!(decode_lower_vec("GG").is_err());
    assert!(decode_nibble(b'g').is_err());
}

#[tokio::test]
async fn bootstrap_is_authenticated_one_time_and_constant_shape() {
    let access = PrivateTargetAccess::new(TargetBootstrapKey([7; KEY_BYTES]));
    let key = UnboundKey::new(&AES_256_GCM, &[7; KEY_BYTES]);
    assert!(key.is_ok());
    let Ok(key) = key else {
        return;
    };
    let key = LessSafeKey::new(key);
    let mut ciphertext = TOKEN.as_bytes().to_vec();
    let nonce = [11; NONCE_BYTES];
    let sealed = key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(AAD),
        &mut ciphertext,
    );
    assert!(sealed.is_ok());
    if sealed.is_err() {
        return;
    }
    let request = TargetPrivateBootstrapRequest {
        nonce: encode_lower(&nonce),
        ciphertext: encode_lower(&ciphertext),
    };
    assert!(access.bootstrap(&request).await.is_ok());
    assert!(access.authenticate(TOKEN).await.is_ok());
    assert!(access.authenticate(&"b".repeat(TOKEN_BYTES)).await.is_err());
    assert!(access.bootstrap(&request).await.is_err());
}

#[test]
#[cfg(unix)]
fn bootstrap_file_is_private_and_unlinked() {
    let root = std::env::temp_dir().join(format!("zeroshot-private-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    assert!(std::fs::create_dir(&root).is_ok());
    let path = root.join("key");
    assert!(std::fs::write(&path, "07".repeat(KEY_BYTES)).is_ok());
    assert!(std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).is_ok());
    assert!(TargetBootstrapKey::load_and_unlink(&path).is_ok());
    assert!(!path.exists());
    let _ = std::fs::remove_dir(&root);
}
