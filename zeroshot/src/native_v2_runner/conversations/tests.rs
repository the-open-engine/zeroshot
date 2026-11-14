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
fn saved(source: ConversationSource) -> ConversationCapture {
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
