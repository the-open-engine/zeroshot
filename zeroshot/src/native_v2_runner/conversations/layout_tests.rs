//! File-layout coverage runs on Windows and macOS CI as well as Linux, without native CLIs.
use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

pub(crate) fn usage() -> Option<TokenUsageDelta> {
    Some(TokenUsageDelta {
        input_tokens: openengine_cluster_protocol::TokenCount::new(0).assert_value(),
        output_tokens: openengine_cluster_protocol::TokenCount::new(0).assert_value(),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    })
}

pub(crate) fn roundtrip(make: impl Fn(PathBuf, &Path) -> ConversationLayout, name: &str) {
    let content = b"only the named conversation";
    roundtrip_with(make, name, content, |restored, _| {
        assert_eq!(restored, content);
    });
}

/// `restored` receives the restored file's bytes and the successor workspace.
pub(crate) fn roundtrip_with(
    make: impl Fn(PathBuf, &Path) -> ConversationLayout,
    name: &str,
    content: &[u8],
    restored: impl Fn(&[u8], &Path),
) {
    let root = tempfile::tempdir().assert_value();
    let one = root.path().join("first/workspace");
    let two = root.path().join("second/workspace");
    fs::create_dir_all(&one).assert_value();
    fs::create_dir_all(&two).assert_value();
    let before = make(root.path().join("first/provider"), &one);
    let after = make(root.path().join("second/provider"), &two);
    let file = before.storage.join(name);
    fs::create_dir_all(file.parent().unwrap()).assert_value();
    fs::write(&file, content).assert_value();
    fs::write(before.storage.join("auth.json"), b"credential marker").assert_value();
    fs::write(
        before.storage.join("settings.json"),
        b"configuration marker",
    )
    .assert_value();
    let source = before
        .capture(ConversationMetadata {
            harness: before.harness.to_owned(),
            id: "checkpoint-id".to_owned(),
            usage: usage(),
        })
        .assert_value();
    assert_eq!(source.files, vec![PathBuf::from(name)]);
    let node = NodeName::new("worker").assert_value();
    let id = NodeInstanceId::new(1).assert_value();
    let saved = capture(ConversationSources::from([(
        id,
        (node.clone(), Ok(source)),
    )]));
    assert!(saved.notices.is_empty());
    let files = saved.files.assert_value();
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(files.directory().join("1/conversation.json")).assert_value(),
    )
    .assert_value();
    assert_eq!(manifest["files"], serde_json::json!([name]));
    fs::remove_dir_all(root.path().join("first")).assert_value();
    assert!(!after.storage.exists());
    after
        .restore(&files.conversation(id, &node).assert_value(), None)
        .expect("restore into an empty provider home");
    crate::execution::platform::private_file(
        &after.storage.join(name),
        crate::execution::platform::FileAccess::Read,
    )
    .expect("restored transcript must remain private to the current user");
    restored(&fs::read(after.storage.join(name)).assert_value(), &two);
    assert!(!after.storage.join("auth.json").exists());
    assert!(!after.storage.join("settings.json").exists());
}

pub(crate) fn nested_storage_ownership(make: impl Fn(&Path) -> ConversationLayout, filename: &str) {
    let root = tempfile::tempdir().assert_value();
    let before = make(&root.path().join("old"));
    fs::create_dir_all(&before.storage).assert_value();
    fs::write(before.storage.join(filename), b"first turn").assert_value();
    let source = before
        .capture(ConversationMetadata {
            harness: before.harness.to_owned(),
            id: "checkpoint-id".to_owned(),
            usage: usage(),
        })
        .assert_value();
    let home = root.path().join("new");
    crate::execution::platform::private_directory(&home).assert_value();
    let after = make(&home);
    assert!(!after.storage.exists());
    let mut assigned = Vec::new();
    after
        .restore_with_ownership(
            ConversationRestore {
                source: &source,
                private_home: Some(&home),
                operation: &ConversationIo::default(),
            },
            |path| {
                assigned.push(path.to_owned());
                Ok(())
            },
        )
        .expect("restore into nested private provider storage");
    let target = after.storage.join(filename);
    assert_eq!(fs::read(&target).assert_value(), b"first turn");
    let expected = target
        .ancestors()
        .take_while(|path| path.starts_with(&home))
        .map(Path::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        assigned, expected,
        "every private storage ancestor must be traversable by the provider"
    );
    assert_eq!(assigned.last(), Some(&home));
    #[cfg(unix)]
    for path in &assigned {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).assert_value().permissions().mode() & 0o777,
            if path == &target { 0o600 } else { 0o700 },
            "{}",
            path.display()
        );
    }
    assert!(
        after
            .restore_with_ownership(
                ConversationRestore {
                    source: &source,
                    private_home: Some(&root.path().join("unrelated")),
                    operation: &ConversationIo::default()
                },
                |_| { panic!("ownership must not escape the trusted private home") }
            )
            .is_err()
    );
}

/// A restore that fails after it created storage leaves that storage to the provider, which then
/// writes its new conversation there.
pub(crate) fn failed_restore_storage_ownership(
    make: impl Fn(&Path) -> ConversationLayout,
    filename: &str,
    content: &[u8],
) {
    let root = tempfile::tempdir().assert_value();
    let before = make(&root.path().join("old"));
    fs::create_dir_all(&before.storage).assert_value();
    fs::write(before.storage.join(filename), content).assert_value();
    let source = before
        .capture(ConversationMetadata {
            harness: before.harness.to_owned(),
            id: "checkpoint-id".to_owned(),
            usage: usage(),
        })
        .assert_value();
    let home = root.path().join("new");
    crate::execution::platform::private_directory(&home).assert_value();
    let after = make(&home);
    let mut assigned = Vec::new();
    assert!(
        after
            .restore_with_ownership(
                ConversationRestore {
                    source: &source,
                    private_home: Some(&home),
                    operation: &ConversationIo::default(),
                },
                |path| {
                    assigned.push(path.to_owned());
                    Ok(())
                },
            )
            .is_err()
    );
    let expected = after
        .storage
        .ancestors()
        .take_while(|path| path.starts_with(&home))
        .map(Path::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(assigned, expected);
    assert!(after.storage.is_dir());
    assert!(!after.storage.join(filename).exists());
}

#[cfg(unix)]
pub(crate) fn nested_storage_capture(make: impl Fn(&Path) -> ConversationLayout, filename: &str) {
    let root = tempfile::tempdir().assert_value();
    let home = root.path().join("home");
    let layout = make(&home);
    fs::create_dir_all(&layout.storage).assert_value();
    fs::write(layout.storage.join(filename), b"provider first turn").assert_value();
    let metadata = ConversationMetadata {
        harness: layout.harness.to_owned(),
        id: "checkpoint-id".to_owned(),
        usage: usage(),
    };
    let source = layout
        .capture_in_home(metadata.clone(), Some(home.clone()))
        .assert_value();
    let relative = layout.storage.strip_prefix(&home).assert_value();
    let ancestor = home.join(relative.components().next().assert_value());
    let outside = root.path().join("supervisor-only");
    fs::rename(&ancestor, &outside).assert_value();
    fs::write(
        outside
            .join(layout.storage.strip_prefix(&ancestor).assert_value())
            .join(filename),
        b"supervisor-only secret",
    )
    .assert_value();
    std::os::unix::fs::symlink(&outside, &ancestor).assert_value();
    // Reject both an already-linked layout and a link installed after filename selection.
    assert!(layout.capture_in_home(metadata, Some(home)).is_err());
    let captured = super::tests::saved(source);
    assert_eq!(captured.notices.len(), 1);
    assert!(captured.notices[0].1.contains("symbolic link"));
    assert_eq!(
        fs::read_dir(captured.files.assert_value().directory())
            .assert_value()
            .count(),
        0
    );
}
