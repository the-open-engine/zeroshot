use super::*;

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("claude").await;
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip(layout, "checkpoint-id.jsonl");
}

#[test]
fn checkpoint_restore_assigns_nested_storage_ancestors_to_provider() {
    crate::native_v2_runner::conversations::layout_tests::nested_storage_ownership(
        |home| layout(home.join(".claude"), Path::new("/relocated/workspace")),
        "checkpoint-id.jsonl",
    );
}

#[cfg(unix)]
#[test]
fn checkpoint_capture_rechecks_nested_storage_ancestors() {
    crate::native_v2_runner::conversations::layout_tests::nested_storage_capture(
        |home| layout(home.join(".claude"), Path::new("/relocated/workspace")),
        "checkpoint-id.jsonl",
    );
}

#[test]
fn project_directory_matches_claude_code_for_long_and_non_bmp_paths() {
    // The two names Claude Code 2.1.317 (native build) created for these working directories.
    let segment = "workspace-with-a-long-name-";
    assert_eq!(
        project_directory(&format!(
            "/var/tmp/claude-project-name-check/{}tail",
            segment.repeat(7)
        )),
        format!(
            "-var-tmp-claude-project-name-check-{}wor-mdmnqf",
            segment.repeat(6)
        )
    );
    assert_eq!(
        project_directory("/var/tmp/claude-project-name-check/repo-😀x"),
        "-var-tmp-claude-project-name-check-repo---x"
    );
    assert_eq!(project_directory("/w/repo.a_b"), "-w-repo-a-b");
    let exact = format!("/{}", "b".repeat(199));
    assert_eq!(project_directory(&exact), format!("-{}", "b".repeat(199)));
    let over = format!("/{}", "b".repeat(200));
    assert_eq!(
        project_directory(&over),
        format!("-{}-86ms41", "b".repeat(199))
    );
    let long = format!("/workspace/{}", "a".repeat(230));
    assert_eq!(
        project_directory(&long),
        format!("-workspace-{}-cooguh", "a".repeat(189))
    );
    let mixed = format!("/w/répo-😀x/{}", "c".repeat(200));
    let name = project_directory(&mixed);
    assert!(name.starts_with("-w-r-po---x-"), "{name}");
    assert!(name.ends_with("-8w1zva"), "{name}");
    assert_eq!(name.len(), 207);
}
