//! Real shipped CLI against local state and the persistent direct-target HTTP boundary.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use openengine_cluster_protocol::{
    FileAddress, FilePath, FileRequest, FileScope, IdempotencyKey, TARGET_FILES_MEDIA_TYPE,
};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap(),
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .env_remove("ZEROSHOT_API_KEY")
            .env_remove("ZEROSHOT_ORGANIZATION")
            .env("ZEROSHOT_STATE_DIR", self.root.path().join("state"))
            .env("ZEROSHOT_CONFIG_DIR", self.root.path().join("config"))
            .env("NO_PROXY", "127.0.0.1,localhost,::1")
            .kill_on_drop(true);
        command
    }
    async fn cli(&self, args: &[&str]) -> Value {
        let output =
            tokio::time::timeout(Duration::from_secs(30), self.command().args(args).output())
                .await
                .unwrap()
                .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn source(&self) -> PathBuf {
        let path = self.root.path().join("source");
        for directory in ["a", "b", "empty"] {
            std::fs::create_dir_all(path.join(directory)).unwrap();
        }
        std::fs::write(path.join("a/same.bin"), binary_data()).unwrap();
        std::fs::write(path.join("b/same.bin"), [128, 42, 0]).unwrap();
        path
    }
    async fn serve(&self, listen: std::net::SocketAddr) -> Child {
        let mut command = self.command();
        let child = command
            .args([
                "target",
                "serve",
                "--listen",
                &listen.to_string(),
                "--public-origin",
                &format!("http://{listen}"),
                "--storage",
            ])
            .arg(self.root.path().join("volume"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if client
                    .get(format!("http://{listen}/.well-known/zeroshot-native-v2"))
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        child
    }
}

fn binary_data() -> Vec<u8> {
    // Exceed the existing target JSON-body ceiling to exercise the separate binary route.
    [0, 255, 17, 0]
        .into_iter()
        .cycle()
        .take(4 * 1024 * 1024 + 17)
        .collect()
}

fn assert_tree(path: &Path) {
    assert_eq!(
        std::fs::read(path.join("a/same.bin")).unwrap(),
        binary_data()
    );
    assert_eq!(
        std::fs::read(path.join("b/same.bin")).unwrap(),
        [128, 42, 0]
    );
    assert!(path.join("empty").is_dir());
}

#[tokio::test]
async fn actual_cli_round_trips_local_binary_tree_empty_members_and_scopes() {
    let fixture = Fixture::new();
    let source = fixture.source();
    fixture
        .cli(&[
            "file",
            "upload",
            source.to_str().unwrap(),
            "data/tree",
            "--operation",
            "upload-one",
        ])
        .await;
    fixture
        .cli(&[
            "file",
            "upload",
            source.join("a/same.bin").to_str().unwrap(),
            "binary",
            "--scope",
            "org",
        ])
        .await;
    let paths = fixture.cli(&["file", "list"]).await;
    assert_eq!(paths["paths"].as_array().unwrap().len(), 1);
    let entries = fixture.cli(&["file", "list", "data/tree"]).await;
    assert_eq!(entries["entries"].as_array().unwrap().len(), 6);
    let output = fixture.root.path().join("download");
    fixture
        .cli(&["file", "download", "data/tree", output.to_str().unwrap()])
        .await;
    assert_tree(&output);
    let empty = fixture.root.path().join("empty-download");
    fixture
        .cli(&[
            "file",
            "download",
            "data/tree",
            empty.to_str().unwrap(),
            "--member",
            "empty",
        ])
        .await;
    assert!(empty.is_dir());
    assert_eq!(std::fs::read_dir(empty).unwrap().count(), 0);
    let member = fixture.root.path().join("member");
    fixture
        .cli(&[
            "file",
            "download",
            "data/tree",
            member.to_str().unwrap(),
            "--member",
            "b/same.bin",
        ])
        .await;
    assert_eq!(std::fs::read(&member).unwrap(), [128, 42, 0]);
    let org = fixture.root.path().join("org");
    fixture
        .cli(&[
            "file",
            "download",
            "binary",
            org.to_str().unwrap(),
            "--scope",
            "org",
        ])
        .await;
    assert_eq!(std::fs::read(org).unwrap(), binary_data());
    fixture.cli(&["file", "remove", "data/tree"]).await;
    assert!(
        fixture.cli(&["file", "list"]).await["paths"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture.cli(&["file", "list", "--scope", "org"]).await["paths"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let refused = fixture
        .command()
        .args([
            "file",
            "upload",
            source.to_str().unwrap(),
            "result",
            "--run",
            "one",
        ])
        .output()
        .await
        .unwrap();
    assert!(!refused.status.success());
}

#[tokio::test]
async fn actual_cli_direct_target_preserves_volume_after_process_restart_and_rejects_truncated_upload()
 {
    let fixture = Fixture::new();
    let source = fixture.source();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = listener.local_addr().unwrap();
    drop(listener);
    let mut child = fixture.serve(listen).await;
    let added = fixture
        .command()
        .args([
            "target",
            "add",
            "disk",
            "--url",
            &format!("http://{listen}"),
            "--direct",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    fixture
        .cli(&[
            "file",
            "upload",
            source.to_str().unwrap(),
            "data/tree",
            "--target",
            "disk",
            "--operation",
            "upload-one",
        ])
        .await;
    fixture
        .cli(&[
            "file",
            "upload",
            source.to_str().unwrap(),
            "data/tree",
            "--target",
            "disk",
            "--operation",
            "upload-one",
        ])
        .await;
    // A known Content-Length followed by an incomplete binary tree cannot replace prior bytes.
    let request = FileRequest::Replace {
        address: FileAddress {
            scope: FileScope::User,
            path: FilePath::new("data/tree").unwrap(),
        },
        operation: IdempotencyKey::new("truncated").unwrap(),
    };
    let metadata = serde_json::to_vec(&request).unwrap();
    let mut socket = tokio::net::TcpStream::connect(listen).await.unwrap();
    socket.write_all(format!("POST /native-v2/files HTTP/1.1\r\nHost: {listen}\r\nContent-Type: {TARGET_FILES_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n", metadata.len()+4+10000).as_bytes()).await.unwrap();
    socket
        .write_all(&(metadata.len() as u32).to_be_bytes())
        .await
        .unwrap();
    socket.write_all(&metadata).await.unwrap();
    socket.write_all(b"OEFILES1partial").await.unwrap();
    socket.shutdown().await.unwrap();
    drop(socket);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let mut child = fixture.serve(listen).await;
    let entries = fixture
        .cli(&["file", "list", "data/tree", "--target", "disk"])
        .await;
    assert_eq!(entries["entries"].as_array().unwrap().len(), 6);
    let output = fixture.root.path().join("download");
    fixture
        .cli(&[
            "file",
            "download",
            "data/tree",
            output.to_str().unwrap(),
            "--target",
            "disk",
        ])
        .await;
    assert_tree(&output);
    assert!(
        fixture.cli(&["file", "list"]).await["paths"]
            .as_array()
            .unwrap()
            .is_empty(),
        "named transfer must not populate local library"
    );
    fixture
        .cli(&["file", "remove", "data/tree", "--target", "disk"])
        .await;
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[tokio::test]
async fn old_target_keeps_discovery_and_refuses_file_commands_before_transfer() {
    use tokio::io::AsyncReadExt;
    use openengine_cluster_protocol::{TargetDiscoveryDocument, TargetAuthentication};
    let fixture = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let byte = socket.read_u8().await.unwrap();
                request.push(byte);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
                assert!(request.len() <= 32768);
            }
            assert!(request.starts_with(b"GET /.well-known/zeroshot-native-v2 "));
            let document = TargetDiscoveryDocument::direct(TargetAuthentication::None)
                .with_run_sizes(zeroshot_engine::native_v2_hosting::host_run_sizes());
            let response = serde_json::to_vec(&document).unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).as_bytes()).await.unwrap();
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
        }
    });
    let added = fixture
        .command()
        .args([
            "target",
            "add",
            "older",
            "--url",
            &format!("http://{listen}"),
            "--direct",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let refused = fixture
        .command()
        .args(["file", "list", "--target", "older"])
        .output()
        .await
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("does not advertise openengine.target-files/v1")
    );
    server.await.unwrap();
}

#[path = "native_v2_files/regressions.rs"]
mod regressions;
