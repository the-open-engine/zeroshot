use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

fn source(root: &Path, names: Vec<PathBuf>) -> ConversationSource {
    ConversationSource::new(
        ConversationMetadata {
            harness: "codex".to_owned(),
            id: "thread".to_owned(),
            usage: super::layout_tests::usage(),
        },
        root.to_owned(),
        names,
    )
}
pub(super) fn saved(source: ConversationSource) -> ConversationCapture {
    capture(ConversationSources::from([(
        NodeInstanceId::new(1).assert_value(),
        (NodeName::new("worker").assert_value(), Ok(source)),
    )]))
}
#[test]
fn incomplete_missing_unsafe_and_oversized_conversations_are_omitted_atomically() {
    let root = tempfile::tempdir().assert_value();
    fs::write(root.path().join("rollout-thread.jsonl"), "first turn").assert_value();
    for name in [
        "missing",
        "../outside",
        "/absolute",
        "C:\\session",
        "nested/../../outside",
    ] {
        let capture = saved(source(
            root.path(),
            vec![PathBuf::from("rollout-thread.jsonl"), PathBuf::from(name)],
        ));
        assert_eq!(capture.notices.len(), 1);
        assert!(!capture.files.assert_value().directory().join("1").exists());
    }
    let oversized = root.path().join("huge");
    fs::File::create(&oversized)
        .assert_value()
        .set_len(MAX_CONVERSATION_BYTES + 1)
        .assert_value();
    let capture = saved(source(root.path(), vec![PathBuf::from("huge")]));
    assert_eq!(capture.notices[0].1, "conversation exceeds the 1 GiB cap");
    assert!(!capture.files.assert_value().directory().join("1").exists());
}
#[test]
fn restore_rolls_back_only_named_conversation_and_rejects_configuration() {
    let root = tempfile::tempdir().assert_value();
    let provider = root.path().join("old");
    fs::create_dir_all(&provider).assert_value();
    fs::write(provider.join("rollout-thread.jsonl"), "checkpoint bytes").assert_value();
    let capture = saved(source(
        &provider,
        vec![PathBuf::from("rollout-thread.jsonl")],
    ));
    let files = capture.files.assert_value();
    let node = NodeName::new("worker").assert_value();
    let restored = files
        .conversation(NodeInstanceId::new(1).assert_value(), &node)
        .assert_value();
    let destination = root.path().join("new");
    fs::create_dir_all(&destination).assert_value();
    fs::write(destination.join("rollout-thread.jsonl"), "later turn").assert_value();
    fs::write(
        destination.join("later-thread.jsonl"),
        "later file for same thread",
    )
    .assert_value();
    fs::write(
        destination.join("rollout-other.jsonl"),
        "another conversation",
    )
    .assert_value();
    fs::write(destination.join("auth.json"), "credential").assert_value();
    // Codex's layout owns the sessions subtree, not the whole provider home.
    let layout = crate::native_v2_codex::conversation::layout(root.path().to_owned());
    let layout = ConversationLayout {
        storage: destination.clone(),
        ..layout
    };
    layout.restore(&restored, None).assert_value();
    assert_eq!(
        fs::read_to_string(destination.join("rollout-thread.jsonl")).assert_value(),
        "checkpoint bytes"
    );
    assert!(!destination.join("later-thread.jsonl").exists());
    assert_eq!(
        fs::read_to_string(destination.join("rollout-other.jsonl")).assert_value(),
        "another conversation"
    );
    assert_eq!(
        fs::read_to_string(destination.join("auth.json")).assert_value(),
        "credential"
    );
    let mut malicious = restored.clone();
    fs::write(
        malicious.storage.join("auth.json"),
        "replacement credential",
    )
    .assert_value();
    malicious.files.push(PathBuf::from("auth.json"));
    assert!(layout.restore(&malicious, None).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("auth.json")).assert_value(),
        "credential"
    );
    fs::remove_file(restored.storage.join("rollout-thread.jsonl")).assert_value();
    assert!(layout.restore(&restored, None).is_err());
    assert_eq!(
        fs::read_to_string(destination.join("rollout-thread.jsonl")).assert_value(),
        "checkpoint bytes"
    );
}

#[test]
fn restore_preserves_native_directories_and_creates_private_storage() {
    use crate::execution::platform;

    let root = tempfile::tempdir().assert_value();
    let provider = root.path().join("old");
    let name = PathBuf::from("existing/new/rollout-thread.jsonl");
    fs::create_dir_all(provider.join("existing/new")).assert_value();
    fs::write(provider.join(&name), b"checkpoint bytes").assert_value();
    let source = source(&provider, vec![name.clone()]);
    let layout = crate::native_v2_codex::conversation::layout(root.path().join("native"));
    let existing = layout.storage.join("existing");
    // Ordinary native directories may have Windows' default token owner, including Administrators.
    fs::create_dir_all(&existing).assert_value();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o750)).assert_value();
    }
    layout
        .restore(&source, None)
        .expect("restore into native provider storage");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&existing).assert_value().permissions().mode() & 0o777,
            0o750
        );
    }
    let new_directory = platform::open_directory(&existing.join("new")).assert_value();
    platform::validate_private_file(&new_directory)
        .expect("new storage directory has private permissions and current-user ownership");
    let transcript = layout.storage.join(&name);
    platform::private_file(&transcript, platform::FileAccess::Read)
        .expect("restored transcript has private permissions and current-user ownership");
    assert_eq!(fs::read(transcript).assert_value(), b"checkpoint bytes");
    assert_eq!(fs::read_dir(&layout.storage).assert_value().count(), 1);
}

#[test]
fn concurrent_restores_can_create_the_same_provider_directories() {
    let root = tempfile::tempdir().assert_value();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let barrier = &barrier;
            let root = root.path();
            scope.spawn(move || {
                let mut results = Vec::new();
                for index in 0..64 {
                    let directory = root.join(format!("{index}/provider/sessions"));
                    barrier.wait();
                    results.push(provider_directory(&directory));
                    barrier.wait();
                }
                // Finish every rendezvous before reporting errors so neither peer is stranded.
                for result in results {
                    result.expect("simultaneous restores must share native storage directories");
                }
            });
        }
    });
}

#[cfg(unix)]
#[test]
fn symbolic_links_cannot_escape_conversation_storage() {
    let root = tempfile::tempdir().assert_value();
    let outside = root.path().join("secret");
    fs::write(&outside, "credential").assert_value();
    let storage = root.path().join("sessions");
    fs::create_dir(&storage).assert_value();
    std::os::unix::fs::symlink(&outside, storage.join("rollout-thread.jsonl")).assert_value();
    let capture = saved(source(
        &storage,
        vec![PathBuf::from("rollout-thread.jsonl")],
    ));
    assert_eq!(capture.notices.len(), 1);
    assert!(!capture.files.assert_value().directory().join("1").exists());
}

#[test]
fn steer_checkpoint_manifest_carries_receipt_references_and_reads_legacy_snapshots() {
    let root = tempfile::tempdir().assert_value();
    fs::write(root.path().join("rollout-thread.jsonl"), "first turn").assert_value();
    let mut source = source(root.path(), vec![PathBuf::from("rollout-thread.jsonl")]);
    let receipts = BTreeSet::from([VectorId::new("receipt-before-snapshot").assert_value()]);
    source.steering = Some(SteeringReceipts {
        conversation: "predecessor/node-instance/1".into(),
        receipts: receipts.clone(),
    });
    let files = saved(source).files.assert_value();
    let instance = NodeInstanceId::new(1).assert_value();
    let node = NodeName::new("worker").assert_value();
    let restored = files.conversation(instance, &node).assert_value();
    assert_eq!(restored.steering.as_ref().assert_value().receipts, receipts);
    let carried = saved(restored).files.assert_value();
    assert_eq!(
        carried
            .conversation(instance, &node)
            .assert_value()
            .steering
            .assert_value()
            .receipts,
        receipts
    );
    let path = files.directory().join("1/conversation.json");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).assert_value()).assert_value();
    legacy.as_object_mut().assert_value().remove("steering");
    fs::write(path, serde_json::to_vec(&legacy).assert_value()).assert_value();
    assert!(
        files
            .conversation(instance, &node)
            .assert_value()
            .steering
            .is_none()
    );
}
