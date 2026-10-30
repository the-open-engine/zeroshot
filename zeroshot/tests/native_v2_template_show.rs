use std::process::{Command, Output};

use openengine_cluster_protocol::GraphSpec;
use openengine_cluster_testkit::assertions::AssertValue;

const CASES: &[(&str, &[&str])] = &[
    ("single-worker", &[]),
    ("single-worker", &["--delivery", "none"]),
    ("software-change", &[]),
    ("software-change", &["--delivery", "none"]),
    ("software-change", &["--push"]),
    ("software-change", &["--pr"]),
    ("software-change", &["--ship"]),
    ("software-change", &["--delivery", "push"]),
    ("software-change", &["--delivery", "pull_request"]),
    ("software-change", &["--delivery", "merge"]),
    ("auto-research", &[]),
    ("auto-research", &["--delivery", "none"]),
    ("auto-research", &["--push"]),
    ("auto-research", &["--delivery", "push"]),
    ("issue-review", &[]),
    ("issue-review", &["--delivery", "none"]),
    ("issue-review", &["--delivery", "comment"]),
    ("pr-review", &[]),
    ("pr-review", &["--delivery", "comment"]),
];

fn command(template: &str, options: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
    command.args(["template", "show", template]).args(options);
    command
}

fn successful(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn piped_and_explicit_json_are_identical_and_round_trip_for_every_delivery_variant() {
    for (template, options) in CASES {
        let piped = successful(command(template, options).output().assert_value());
        let explicit = successful(
            command(template, options)
                .arg("--json")
                .output()
                .assert_value(),
        );
        assert_eq!(piped, explicit, "{template} {options:?}");
        let graph: GraphSpec = serde_json::from_slice(&piped).assert_value();
        let mut round_trip = serde_json::to_vec(&graph).assert_value();
        round_trip.push(b'\n');
        assert_eq!(piped, round_trip, "{template} {options:?}");
    }
}

#[test]
fn redirected_stdout_still_exports_a_complete_graph() {
    let file = tempfile::NamedTempFile::new().assert_value();
    let result = command("software-change", &["--ship"])
        .stdout(file.reopen().assert_value())
        .output()
        .assert_value();
    assert!(successful(result).is_empty());
    let exported = std::fs::read(file.path()).assert_value();
    let piped = successful(
        command("software-change", &["--ship"])
            .output()
            .assert_value(),
    );
    assert_eq!(exported, piped);
    serde_json::from_slice::<GraphSpec>(&exported).assert_value();
}

#[cfg(unix)]
#[test]
fn terminal_defaults_to_tree_and_explicit_json_overrides_it_for_every_delivery_variant() {
    for (template, options) in CASES {
        let tree = terminal_stdout(command(template, options));
        assert!(
            tree.starts_with(&format!("{template} (delivery: ")),
            "{tree}"
        );
        assert!(tree.contains("└─ "), "{tree}");
        assert!(
            tree.contains("* executable node: RuntimePlan binding"),
            "{tree}"
        );
        match *template {
            "software-change" => {
                assert!(tree.contains("worker *  step  1 attempt"));
                assert!(tree.contains("parallel_reviews  par  wait for all"));
                assert!(tree.contains("acceptance *  verifier  2 attempts"));
                assert!(tree.contains("accepted | rejected"));
                assert!(tree.contains("worker.error in [crash | malformed | refusal | timeout]"));
                assert!(tree.contains("otherwise → "));
                assert!(tree.contains("change_loop  loop  max 10 iterations"));
                if *options != ["--delivery", "none"] {
                    assert!(tree.contains("deliver *  verifier"));
                    assert!(tree.contains("delivery_retry  map"));
                } else {
                    assert!(!tree.contains("deliver *"));
                }
            }
            "pr-review" => {
                assert!(tree.contains("reproducer *  step"));
                assert!(tree.contains("map  over state."));
                assert!(tree.contains("deliver *"));
            }
            _ => {}
        }
        let mut explicit = command(template, options);
        explicit.arg("--json");
        let terminal_json = terminal_stdout(explicit);
        let piped = successful(command(template, options).output().assert_value());
        assert_eq!(terminal_json.as_bytes(), piped, "{template} {options:?}");
    }
}

#[cfg(unix)]
fn terminal_stdout(mut command: Command) -> String {
    use std::fs::File;
    use std::io::{self, Read};
    use std::os::fd::{AsRawFd, FromRawFd};

    let mut master_fd = -1;
    let mut slave_fd = -1;
    // SAFETY: openpty receives valid output pointers and null optional configuration pointers.
    let result = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(result, 0, "openpty: {}", io::Error::last_os_error());
    // SAFETY: successful openpty returns two fresh file descriptors, each owned once here.
    let (mut master, slave) =
        unsafe { (File::from_raw_fd(master_fd), File::from_raw_fd(slave_fd)) };
    for file in [&master, &slave] {
        // SAFETY: these descriptors are live; CLOEXEC prevents the child inheriting the reader.
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("read terminal: {error}"),
            }
        }
        output
    });
    command.stdout(slave);
    let output = command.output().assert_value();
    drop(command);
    successful(output);
    String::from_utf8(reader.join().assert_value())
        .assert_value()
        .replace("\r\n", "\n")
}
