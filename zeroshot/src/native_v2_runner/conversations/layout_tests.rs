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
    let root = tempfile::tempdir().assert_value();
    let one = root.path().join("first/workspace");
    let two = root.path().join("second/workspace");
    fs::create_dir_all(&one).assert_value();
    fs::create_dir_all(&two).assert_value();
    let before = make(root.path().join("first/provider"), &one);
    let after = make(root.path().join("second/provider"), &two);
    let file = before.storage.join(name);
    fs::create_dir_all(file.parent().unwrap()).assert_value();
    fs::write(&file, b"only the named conversation").assert_value();
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
        .assert_value();
    assert_eq!(
        fs::read(after.storage.join(name)).assert_value(),
        b"only the named conversation"
    );
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
        .restore_with_ownership(&source, Some(&home), |path| {
            assigned.push(path.to_owned());
            Ok(())
        })
        .assert_value();
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
            .restore_with_ownership(&source, Some(&root.path().join("unrelated")), |_| {
                panic!("ownership must not escape the trusted private home")
            })
            .is_err()
    );
}
