//! Exercise gh's real cursor selection, which shell doubles cannot reproduce.
use super::*;
use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};

fn pagination_page(second: bool, truncated_queue: bool, aliased: bool) -> Value {
    let mut page = super::recovery_tests::recovery_policy(true);
    let entries = &mut page["data"]["repository"]["mergeQueue"]["entries"];
    entries["totalCount"] = json!(if truncated_queue { 2 } else { 1 });
    entries
        .as_object_mut()
        .assert_value()
        .remove("queuePageInfo");
    entries[if aliased { "queuePageInfo" } else { "pageInfo" }] = json!({
        "hasNextPage":truncated_queue,"endCursor":"queue-next"
    });
    page["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = json!({"contexts":{
        "nodes":[check_run(if second {"second-required"} else {"first-required"}, "COMPLETED", Some("SUCCESS"), true)],
        "pageInfo":{"hasNextPage":!second,"endCursor":if second {"checks-end"} else {"checks-next"}}
    }});
    page
}

fn request_body(stream: &mut TcpStream) -> Value {
    stream.set_nonblocking(false).assert_value();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .assert_value();
    let mut reader = std::io::BufReader::new(stream);
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).assert_value() > 0);
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>().assert_value();
        }
    }
    assert!(length > 0 && length < 64 * 1024);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).assert_value();
    serde_json::from_slice(&body).assert_value()
}

#[tokio::test]
async fn github_policy_query_keeps_native_gh_pagination_on_required_checks() {
    let directory = tempfile::tempdir().assert_value();
    let program = crate::execution::platform::find_executable(
        "gh",
        &std::env::vars().collect(),
        &std::env::current_dir().assert_value(),
    )
    .expect("install GitHub CLI for the native pagination regression");
    let arguments = policy::query_arguments(&review()).assert_value();
    let aliased = arguments
        .iter()
        .any(|argument| argument.contains("queuePageInfo: pageInfo"));
    for truncated_queue in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").assert_value();
        let address = listener.local_addr().assert_value();
        listener.set_nonblocking(true).assert_value();
        let server = std::thread::spawn(move || {
            let mut cursors = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while cursors.len() < 2 && std::time::Instant::now() < deadline {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                let request = request_body(&mut stream);
                let body =
                    pagination_page(!cursors.is_empty(), truncated_queue, aliased).to_string();
                cursors.push(request["variables"]["endCursor"].clone());
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).assert_value();
            }
            cursors
        });
        // A fake-token loopback proxy stays isolated from ambient forge credentials and egress.
        let mut command = tokio::process::Command::new(&program);
        command
            .arg("api")
            .args(&arguments)
            .env_clear()
            .env("GH_HOST", "github.localhost")
            .env("GH_TOKEN", "offline-test-token")
            .env("GH_ENTERPRISE_TOKEN", "offline-test-token")
            .env("GH_CONFIG_DIR", directory.path())
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("HTTP_PROXY", format!("http://{address}"))
            .env("NO_PROXY", "")
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .assert_value()
            .assert_value();
        let cursors = server.join().assert_value();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(cursors, vec![Value::Null, json!("checks-next")]);
        let pages: Value = serde_json::from_slice(&output.stdout).assert_value();
        assert_eq!(pages.as_array().assert_value().len(), 2);
        assert!(matches!(
            classify_policy(pages, &review()).assert_value().state,
            SourceReviewState::Open {
                checks: SourceChecks::Passed
            }
        ));
    }
}
