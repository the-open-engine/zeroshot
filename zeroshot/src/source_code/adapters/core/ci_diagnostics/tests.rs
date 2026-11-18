use super::*;
use crate::source_code_provider::SourceDeliveryCredential;
use openengine_cluster_testkit::assertions::AssertValue;

pub(in crate::source_code_provider::adapters::core) fn rustc_diagnostics() -> [String; 2] {
    let root = tempfile::tempdir().assert_value();
    std::fs::write(
        root.path().join("colored-syntax.rs"),
        "fn main() {\n    let value = ;\n}\n",
    )
    .assert_value();
    let output = std::process::Command::new("rustc")
        .current_dir(root.path())
        .args(["--color", "always", "colored-syntax.rs"])
        .output()
        .assert_value();
    assert!(!output.status.success());
    let colored = String::from_utf8(output.stderr).assert_value();
    assert!(colored.contains("error\x1b["));
    let mut plain = colored.clone();
    while let Some(start) = plain.find("\x1b[") {
        let end = start + plain[start..].find('m').assert_value() + 1;
        plain.replace_range(start..end, "");
    }
    assert!(!plain.contains('\x1b'));
    assert!(plain.contains("expected expression"));
    assert!(plain.contains("colored-syntax.rs:2:17"));
    [plain, colored]
}

#[test]
fn ansi_markers_use_raw_offsets_and_keep_specific_failure_priority() {
    for marker in [
        "error\x1b[0m\x1b[1m: expected expression",
        "error\x1b[0m[E0433]: unresolved item",
        "fatal\x1b[0m: failed",
        "##[\x1b[31merror]failed",
        "panicked\x1b[0m at source.rs:2",
        "panic\x1b[0m: failed",
        "assertion\x1b[0m failed",
        "Assertion\x1b[0mError: failed",
        "Traceback\x1b[0m (most recent call last)",
    ] {
        let prefix = "résumé 界🙂 \x1b[1;31m";
        let raw = format!("{prefix}{marker}\n");
        assert_eq!(first_failure(&raw), Some(prefix.len()), "{marker}");
    }
    let raw = "error\x1b[0m: expected rejection\n界 \x1b[1massertion\x1b[0m failed\n";
    assert_eq!(first_failure(raw), raw.find("assertion"));
}

#[test]
fn rustc_errors_survive_capture_and_shared_feedback_budgets() {
    for diagnostic in rustc_diagnostics() {
        for (noise, jobs) in [(8_000, 1), (8_000, 2), (24_000, 1), (24_000, 8)] {
            let raw = format!("{diagnostic}{}final exit code 1\n", "noise\n".repeat(noise));
            assert!(raw.is_ascii());
            assert_eq!(raw.len() > MAX_JOB_LOG_TAIL_BYTES, noise == 24_000);
            let log = job_log_tail(
                &raw,
                raw.len(),
                SourceDeliveryCredential::token("test-token"),
                &[],
            );
            assert!(log.text.len() <= MAX_JOB_LOG_TAIL_BYTES);
            assert!(log.text.contains("expected expression"));
            assert!(log.text.contains("colored-syntax.rs:2:17"));
            if noise == 8_000 {
                assert_eq!(log.text, raw);
                assert!(raw.len() * 2 > MAX_FAILURE_FEEDBACK_CHARS);
            }
            let logs = (91..91 + jobs)
                .map(|job| (job, log.clone()))
                .collect::<Vec<_>>();
            let mut feedback = "Required CI checks failed".to_owned();
            include_job_logs(&mut feedback, "CI", &logs);
            assert!(feedback.chars().count() <= MAX_FAILURE_FEEDBACK_CHARS);
            assert_eq!(
                feedback.matches("expected expression, found").count(),
                jobs as usize
            );
            assert_eq!(
                feedback.matches("colored-syntax.rs:2:17").count(),
                jobs as usize
            );
            assert_eq!(feedback.matches("final exit code 1").count(), jobs as usize);
            assert!(!feedback.contains('\u{fffd}'));
            assert!(
                feedback
                    .chars()
                    .all(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            );
        }
    }
}
