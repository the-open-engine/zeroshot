use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

// A header and a tool result as Pi 1.4.0 writes them, then bytes that are not JSON: every line
// after the header must come back exactly as it was.
const HEADER: &str = r#"{"type":"session","version":3,"id":"zs-id","timestamp":"2026-11-16T13:42:24.042Z","cwd":"/predecessor/workspace"}"#;
const TURNS: &str = "{\"type\":\"message\",\"id\":\"16b719e1\",\"message\":{\"role\":\"toolResult\",\"content\":[{\"type\":\"text\",\"text\":\"NUMBER 363445642772\\n\"}]}}\nnot json \u{1f600}\n";

fn proceed() -> io::Result<()> {
    Ok(())
}

fn assert_relocated(restored: &[u8], workspace: &Path) {
    let restored = std::str::from_utf8(restored).assert_value();
    let (header, rest) = restored.split_once('\n').assert_value();
    let header: Value = serde_json::from_str(header).assert_value();
    assert_eq!(header["cwd"], process_cwd(workspace));
    assert_eq!(header["type"], "session");
    assert_eq!(header["version"], 3);
    assert_eq!(header["id"], "zs-id");
    assert_eq!(header["timestamp"], "2026-11-16T13:42:24.042Z");
    assert_eq!(rest, TURNS);
}

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("pi").await;
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip_with(
        |home, workspace| layout(&home, workspace),
        "2026-11-14_checkpoint-id.jsonl",
        format!("{HEADER}\n{TURNS}").as_bytes(),
        assert_relocated,
    );
}

#[test]
fn a_restored_session_names_the_successor_workspace_and_keeps_every_turn() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("successor");
    fs::create_dir(&workspace).assert_value();
    let file = root.path().join("2026-11-16T13-42-24-042Z_zs-id.jsonl");
    fs::write(&file, format!("{HEADER}\n{TURNS}")).assert_value();

    relocate(&file, &workspace, &mut proceed).assert_value();

    assert_relocated(&fs::read(&file).assert_value(), &workspace);
    assert_eq!(fs::read_dir(root.path()).assert_value().count(), 2);
    // A session that already names this workspace is not written again.
    let relocated = fs::read(&file).assert_value();
    relocate(&file, &workspace, &mut || {
        Err(io::ErrorKind::Interrupted.into())
    })
    .assert_value();
    assert_eq!(fs::read(&file).assert_value(), relocated);
}

#[test]
fn a_first_line_that_is_no_session_header_fails_and_leaves_the_file_as_it_is() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).assert_value();
    let oversized = format!(
        "{{\"cwd\":\"/predecessor\",\"padding\":\"{}\"}}\nturn\n",
        "x".repeat(usize::try_from(MAX_HEADER_BYTES).assert_value())
    );
    for content in [
        oversized.as_str(),
        "first-turn bytes without a header\n",
        "[\"cwd\"]\n",
        "{\"type\":\"session\"}\nturn\n",
        "{\"cwd\":7}\nturn\n",
        "{\"cwd\":\"/predecessor\"}\nturn\n",
        "{\"type\":\"session\",\"cwd\":\"/predecessor\"}\nturn\n",
        "{\"type\":\"session\",\"id\":\"\",\"cwd\":\"/predecessor\"}\nturn\n",
        "{\"type\":\"message\",\"id\":\"zs-id\",\"cwd\":\"/predecessor\"}\nturn\n",
        "",
    ] {
        let file = root.path().join("session.jsonl");
        fs::write(&file, content).assert_value();
        assert_eq!(
            relocate(&file, &workspace, &mut proceed)
                .expect_err("no header")
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read_to_string(&file).assert_value(), content);
    }
}

#[test]
fn a_cancelled_relocation_fails_and_leaves_the_file_as_it_is() {
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).assert_value();
    let file = root.path().join("session.jsonl");
    let content = format!("{HEADER}\n{}", "turn\n".repeat(64 * 1024));
    fs::write(&file, &content).assert_value();
    let mut reads = 0;
    let mut cancelled_after_two_reads = || {
        reads += 1;
        if reads > 2 {
            Err(io::ErrorKind::Interrupted.into())
        } else {
            Ok(())
        }
    };
    assert_eq!(
        relocate(&file, &workspace, &mut cancelled_after_two_reads)
            .expect_err("cancelled")
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(fs::read_to_string(&file).assert_value(), content);
}

#[test]
fn a_checkpoint_whose_session_has_no_header_is_not_restored() {
    use crate::native_v2_runner::conversations::ConversationMetadata;
    let root = tempfile::tempdir().assert_value();
    let workspace = root.path().join("second/workspace");
    fs::create_dir_all(&workspace).assert_value();
    let before = layout(&root.path().join("first/provider"), &workspace);
    let after = layout(&root.path().join("second/provider"), &workspace);
    let name = "2026-11-14_checkpoint-id.jsonl";
    fs::create_dir_all(&before.storage).assert_value();
    fs::write(before.storage.join(name), "turns without a header\n").assert_value();
    let source = before
        .capture(ConversationMetadata {
            harness: "pi".to_owned(),
            id: "checkpoint-id".to_owned(),
            usage: None,
        })
        .assert_value();
    // The run then starts a new conversation and says so, instead of an empty Pi session
    // passing for the restored one.
    assert_eq!(
        after.restore(&source, None),
        Err("provider conversation could not be moved to this workspace")
    );
    assert!(!after.storage.join(name).exists());
}

#[test]
fn a_failed_restore_leaves_the_storage_it_created_to_the_provider() {
    let workspace = tempfile::tempdir().assert_value();
    crate::native_v2_runner::conversations::layout_tests::failed_restore_storage_ownership(
        |home| layout(home, workspace.path()),
        "2026-11-14_checkpoint-id.jsonl",
        b"turns without a header\n",
    );
}
