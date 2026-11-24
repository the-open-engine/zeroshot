use super::*;
use openengine_cluster_protocol::{
    FILE_TREE_MAGIC, MAX_FILE_ENTRIES, MAX_FILE_METADATA_BYTES, TARGET_FILES_PATH,
    TargetAuthentication, TargetDiscoveryDocument, TargetFilesDiscovery,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

async fn direct_fixture() -> (Fixture, Child, std::net::SocketAddr) {
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let child = fixture.serve(address).await;
    register(&fixture, address).await;
    (fixture, child, address)
}

async fn register(fixture: &Fixture, address: std::net::SocketAddr) {
    let output = fixture
        .command()
        .args([
            "target",
            "add",
            "disk",
            "--url",
            &format!("http://{address}"),
            "--direct",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn directory_header(name: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"kind":"directory", "path":name})).unwrap()
}
fn frame(output: &mut Vec<u8>, header: &[u8]) {
    output.extend((header.len() as u32).to_be_bytes());
    output.extend(header);
}

#[tokio::test]
async fn actual_cli_lists_a_complete_tree_at_the_metadata_limit() {
    let (fixture, mut child, address) = direct_fixture().await;
    let root = directory_header("");
    let header_length = directory_header(&"x".repeat(100)).len();
    let count = (MAX_FILE_ENTRIES - 1).min((MAX_FILE_METADATA_BYTES - root.len()) / header_length);
    let mut body = FILE_TREE_MAGIC.to_vec();
    frame(&mut body, &root);
    for i in 0..count {
        frame(
            &mut body,
            &directory_header(&format!("{i:08}{}", "x".repeat(92))),
        );
    }
    body.extend(0_u32.to_be_bytes());
    let request = serde_json::to_vec(&FileRequest::Replace {
        address: FileAddress {
            scope: FileScope::User,
            path: FilePath::new("large-tree").unwrap(),
        },
        operation: IdempotencyKey::new("metadata-limit").unwrap(),
    })
    .unwrap();
    let mut upload = Vec::new();
    frame(&mut upload, &request);
    upload.extend(body);
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}{TARGET_FILES_PATH}"))
        .header("Content-Type", TARGET_FILES_MEDIA_TYPE)
        .body(upload)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let list = fixture
        .cli(&["file", "list", "large-tree", "--target", "disk"])
        .await;
    assert_eq!(list["entries"].as_array().unwrap().len(), count + 1);
    assert!(serde_json::to_vec(&list).unwrap().len() > MAX_FILE_METADATA_BYTES + 4096);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn actual_cli_downloads_valid_noncanonical_headers_without_changing_file_bytes() {
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let payload = b"binary bytes\0\xff\n";
    let mut tree = FILE_TREE_MAGIC.to_vec();
    let header = format!(
        "{{\n  \"path\": \"\",\n  \"kind\": \"file\",\n  \"byteLength\": {}\n}}",
        payload.len()
    );
    frame(&mut tree, header.as_bytes());
    tree.extend(payload);
    tree.extend(Sha256::digest(payload));
    tree.extend(0_u32.to_be_bytes());
    let server = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
                assert!(request.len() <= 32768);
            }
            let (content_type, response) = if request.starts_with(b"GET ") {
                let mut discovery = TargetDiscoveryDocument::direct(TargetAuthentication::None)
                    .with_run_sizes(zeroshot_engine::native_v2_hosting::host_run_sizes());
                discovery.extensions.files = Some(TargetFilesDiscovery {
                    kind: openengine_cluster_protocol::TARGET_FILES_KIND.to_owned(),
                });
                ("application/json", serde_json::to_vec(&discovery).unwrap())
            } else {
                assert!(request.starts_with(format!("POST {TARGET_FILES_PATH} ").as_bytes()));
                let head = String::from_utf8(request).unwrap();
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                (TARGET_FILES_MEDIA_TYPE, tree.clone())
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).as_bytes()).await.unwrap();
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    register(&fixture, address).await;
    let destination = fixture.root.path().join("output");
    fixture
        .cli(&[
            "file",
            "download",
            "blob",
            destination.to_str().unwrap(),
            "--target",
            "disk",
        ])
        .await;
    assert_eq!(std::fs::read(destination).unwrap(), payload);
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .unwrap()
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn actual_cli_rejects_non_utf8_destination_before_publication() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let destination = fixture
        .root
        .path()
        .join(std::ffi::OsString::from_vec(b"output-\xff".to_vec()));
    let output = fixture
        .command()
        .args(["file", "download", "blob"])
        .arg(&destination)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(101));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    assert!(!destination.exists());
    assert!(!fixture.root.path().join("state").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn actual_direct_target_reclamation_refuses_external_objects_symlink() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (fixture, mut child, _) = direct_fixture().await;
    let external = fixture.root.path().join("outside");
    std::fs::create_dir(&external).unwrap();
    std::fs::set_permissions(&external, std::fs::Permissions::from_mode(0o700)).unwrap();
    let sentinel = external.join("stage-external-sentinel");
    std::fs::write(&sentinel, b"untouched").unwrap();
    std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o600)).unwrap();
    let objects = fixture.root.path().join("volume/files/objects");
    std::fs::rename(&objects, fixture.root.path().join("original-objects")).unwrap();
    symlink(&external, &objects).unwrap();
    let output = fixture
        .command()
        .args(["file", "remove", "never-existed", "--target", "disk"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"untouched");
    child.kill().await.unwrap();
}

async fn replace_tree(
    address: std::net::SocketAddr,
    operation: &str,
    tree: Vec<u8>,
) -> reqwest::Response {
    let request = serde_json::to_vec(&FileRequest::Replace {
        address: FileAddress {
            scope: FileScope::User,
            path: FilePath::new("canonical-limit").unwrap(),
        },
        operation: IdempotencyKey::new(operation).unwrap(),
    })
    .unwrap();
    let mut upload = Vec::new();
    frame(&mut upload, &request);
    upload.extend(tree);
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}{TARGET_FILES_PATH}"))
        .header("Content-Type", TARGET_FILES_MEDIA_TYPE)
        .body(upload)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn normalized_metadata_overflow_rejects_replacement_without_poisoning_the_prior_selection() {
    let (fixture, mut child, address) = direct_fixture().await;
    // Valid exponent encodings remain usable when both representations fit their budgets.
    let prior = [255_u8; 1000];
    let mut seed = FILE_TREE_MAGIC.to_vec();
    frame(&mut seed, br#"{"kind":"file","path":"","byteLength":1e3}"#);
    seed.extend(prior);
    seed.extend(Sha256::digest(prior));
    seed.extend(0_u32.to_be_bytes());
    assert_eq!(
        replace_tree(address, "seed-valid", seed).await.status(),
        reqwest::StatusCode::OK
    );

    let mut tree = FILE_TREE_MAGIC.to_vec();
    let root = directory_header("");
    frame(&mut tree, &root);
    let mut metadata = root.len();
    let mut parent = String::new();
    for component in ['p', 'q', 'r', 's', 't', 'u', 'v'] {
        if !parent.is_empty() {
            parent.push('/');
        }
        parent.push_str(&component.to_string().repeat(128));
        let header = directory_header(&parent);
        metadata += header.len();
        frame(&mut tree, &header);
    }
    let header = |i| {
        format!(
            "{{\"kind\":\"file\",\"path\":\"{parent}/{i:08}{}\",\"byteLength\":1e3}}",
            "x".repeat(92)
        )
        .into_bytes()
    };
    let length = header(0).len();
    let count = (MAX_FILE_METADATA_BYTES - metadata) / length;
    assert!(count + 8 <= MAX_FILE_ENTRIES);
    assert!(metadata + count * length <= MAX_FILE_METADATA_BYTES);
    assert!(metadata + count * (length + 1) > MAX_FILE_METADATA_BYTES);
    let data = [b'x'; 1000];
    let digest = Sha256::digest(data);
    for i in 0..count {
        frame(&mut tree, &header(i));
        tree.extend(data);
        tree.extend(digest);
    }
    tree.extend(0_u32.to_be_bytes());
    let response = replace_tree(address, "canonical-expansion", tree).await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    // Reopen after rejection, then use the real CLI for every operation that the oversized
    // stored representation previously broke. The failed operation must not consume its key.
    child.kill().await.unwrap();
    child = fixture.serve(address).await;
    let entries = fixture
        .cli(&["file", "list", "canonical-limit", "--target", "disk"])
        .await;
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    assert_eq!(entries["entries"][0]["byteLength"], 1000);
    let output = fixture.root.path().join("download");
    fixture
        .cli(&[
            "file",
            "download",
            "canonical-limit",
            output.to_str().unwrap(),
            "--target",
            "disk",
        ])
        .await;
    assert_eq!(std::fs::read(output).unwrap(), prior);
    let source = fixture.root.path().join("replacement.bin");
    std::fs::write(&source, [0, 254, 43, 0]).unwrap();
    fixture
        .cli(&[
            "file",
            "upload",
            source.to_str().unwrap(),
            "canonical-limit",
            "--target",
            "disk",
            "--operation",
            "canonical-expansion",
        ])
        .await;
    fixture
        .cli(&["file", "remove", "canonical-limit", "--target", "disk"])
        .await;
    assert!(
        fixture.cli(&["file", "list", "--target", "disk"]).await["paths"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    child.kill().await.unwrap();
}
