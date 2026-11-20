use super::*;

const HOOK_SECRET: &str = "local-environment-private-token";

impl LocalFixture {
    fn with_hook_worker(self, script: &str) -> Self {
        let mut graph = local_graph::graph();
        graph["root"]["children"][0]
            .as_object_mut()
            .assert_value()
            .remove("instructions");
        write_json(&self.graph, &graph);
        let mut runtime = local_runtime();
        runtime["workspaceCheckpoints"] = json!(false);
        runtime["nodes"]["worker"] = json!({
            "kind": "script",
            "command": ["sh", "-eu", "-c", format!("cat >/dev/null\n{script}")],
            "connections": {}
        });
        write_json(&self.runtime, &runtime);
        self
    }

    async fn submit_with_hooks(&self, definition: &Value) -> String {
        self.submit_with_hook_mode(definition, "finish").await
    }

    async fn submit_with_hook_mode(&self, definition: &Value, mode: &str) -> String {
        let environment = self.root.path("environment.json");
        write_json(&environment, definition);
        let output = self
            .output(
                &[
                    "run",
                    "--title",
                    "Local environment lifecycle",
                    "--graph",
                    self.graph.to_str().assert_value(),
                    "--input",
                    self.input.to_str().assert_value(),
                    "--runtime-config",
                    self.runtime.to_str().assert_value(),
                    "--environment",
                    environment.to_str().assert_value(),
                    "-d",
                ],
                mode,
            )
            .await;
        assert_success(&output, "submit local environment run");
        receipt_run_id(&output)
    }

    fn hook_file(&self, name: &str) -> String {
        fs::read_to_string(self.repository.join(name)).assert_value_with(name)
    }

    async fn wait_for_hook_file(&self, name: &str) {
        let deadline = Instant::now() + CLI_TIMEOUT;
        while !self.repository.join(name).is_file() {
            assert!(Instant::now() < deadline, "{name} did not appear");
            sleep(Duration::from_millis(25)).await;
        }
    }

    async fn force_stop_hook_run(&self, run: &str) {
        self.json(&["force-stop", run], "finish").await;
        self.wait_terminal(run, "finish", "force_stopped").await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_hooks_keep_native_service_alive_through_graph_and_teardown() {
    let mut fixture = LocalFixture::new().with_hook_worker(
        r#"
test "$(cat hook-run)" = "$ZEROSHOT_RUN_ID"
test "$LOCAL_ENV_MARKER" = public
test -z "${HOOK_TEST_TOKEN+x}"
test -z "${UNDECLARED_SECRET+x}"
touch service-write-request
tries=0
while ! test -f service-write-confirmed && test "$tries" -lt 100; do
    tries=$((tries + 1))
    sleep 0.05
done
test -f service-write-confirmed
kill -0 "$(cat service.pid)"
printf 'worker\n' >> hook-order
printf 'null\n'
"#,
    );
    fixture
        .harness_environment
        .insert("HOOK_TEST_TOKEN".to_owned(), HOOK_SECRET.to_owned());
    fixture
        .harness_environment
        .insert("HOOK_LOG_TOKEN".to_owned(), "~".to_owned());
    let run = fixture
        .submit_with_hooks(&json!({
            "variables": {"LOCAL_ENV_MARKER": "public"},
            "connections": {"package-registry": ["HOOK_TEST_TOKEN", "HOOK_LOG_TOKEN"]},
            "setup": format!(r#"
test "$LOCAL_ENV_MARKER" = public
test "$HOOK_TEST_TOKEN" = {HOOK_SECRET}
test -z "${{UNDECLARED_SECRET+x}}"
test -z "${{OPENAI_API_KEY+x}}"
printf 'setup\n' > hook-order
printf '%s\n' "$ZEROSHOT_RUN_ID" > hook-run
printf 'setup output %s\n' "$HOOK_TEST_TOKEN"
printf 'setup stderr %s\n' "$HOOK_TEST_TOKEN" >&2
"#),
            "startup": r#"
test "$(cat hook-order)" = setup
test "$(cat hook-run)" = "$ZEROSHOT_RUN_ID"
test -n "$HOOK_TEST_TOKEN"
(
    printf 'expanding secret line: '
    printf '%7500s' '' | tr ' ' "$HOOK_LOG_TOKEN"
    printf '\n'
    while ! test -f service-write-request; do sleep 0.05; done
    printf 'background output after expanded line\n'
    touch service-write-confirmed
    exec sleep 300
) &
printf '%s\n' "$!" > service.pid
printf 'startup\n' >> hook-order
"#,
            "teardown": r#"
test "$(cat hook-run)" = "$ZEROSHOT_RUN_ID"
test -n "$HOOK_TEST_TOKEN"
kill -0 "$(cat service.pid)"
printf 'teardown\n' >> hook-order
"#,
        }))
        .await;
    fixture.wait_succeeded(&run, "finish").await;
    assert_eq!(
        fixture.hook_file("hook-order"),
        "setup\nstartup\nworker\nteardown\n"
    );
    assert_eq!(fixture.hook_file("hook-run").trim(), run);
    let pid = fixture
        .hook_file("service.pid")
        .trim()
        .parse()
        .assert_value();
    wait_for_exit(pid).await;
    let logs = fixture.output(&["logs", &run], "finish").await;
    assert_success(&logs, "replay hook output");
    let text = String::from_utf8_lossy(&logs.stdout);
    assert!(text.contains("setup output"), "{text}");
    assert!(text.contains("setup stderr"), "{text}");
    assert!(!text.contains(HOOK_SECRET), "{text}");
    assert!(
        text.contains("background output after expanded line"),
        "{text}"
    );
    assert!(
        !text.contains("~~~~~~~~"),
        "raw short secret leaked to hook output"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_teardown_runs_after_setup_startup_and_graph_failures() {
    for (reason, setup_exit, startup_exit, expected_order) in [
        ("environment_setup_failed", 23, 0, "setup\nteardown\n"),
        (
            "environment_startup_failed",
            0,
            24,
            "setup\nstartup\nteardown\n",
        ),
        ("worker_failed", 0, 0, "setup\nstartup\nworker\nteardown\n"),
    ] {
        let fixture =
            LocalFixture::new().with_hook_worker("printf 'worker\\n' >> hook-order\nexit 25");
        let teardown_exit = if reason == "worker_failed" { 26 } else { 0 };
        let run = fixture
            .submit_with_hooks(&json!({
                "setup": format!("printf 'setup\\n' >> hook-order\nexit {setup_exit}"),
                "startup": format!("printf 'startup\\n' >> hook-order\nexit {startup_exit}"),
                "teardown": format!("printf 'teardown\\n' >> hook-order\nexit {teardown_exit}")
            }))
            .await;
        fixture.wait_terminal(&run, "finish", reason).await;
        assert_eq!(fixture.hook_file("hook-order"), expected_order, "{reason}");
        if teardown_exit != 0 {
            fixture
                .assert_replay(
                    "logs",
                    &run,
                    "Environment teardown failed with exit code 26",
                )
                .await;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_force_stop_ends_inflight_startup_before_teardown() {
    let fixture = LocalFixture::new().with_hook_worker("touch worker-ran\nprintf 'null\\n'");
    let run = fixture
        .submit_with_hooks(&json!({
            "startup": "printf '%s\\n' \"$$\" > startup.pid\nexec sleep 300",
            "teardown": r#"
if kill -0 "$(cat startup.pid)" 2>/dev/null; then printf 'startup-live\n' >> hook-order; fi
printf 'teardown\n' >> hook-order
"#,
        }))
        .await;
    fixture.wait_for_hook_file("startup.pid").await;
    fixture.force_stop_hook_run(&run).await;
    let pid = fixture
        .hook_file("startup.pid")
        .trim()
        .parse()
        .assert_value();
    wait_for_exit(pid).await;
    assert_eq!(fixture.hook_file("hook-order"), "teardown\n");
    assert!(!fixture.repository.join("worker-ran").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn local_resume_reuses_saved_hooks_and_assigns_a_fresh_run_id() {
    let fixture =
        LocalFixture::new().with_hook_worker("printf 'worker\\n' >> hook-order\nprintf 'null\\n'");
    let original = fixture
        .submit_with_hooks(&json!({
            "setup": "printf 'setup %s\\n' \"$ZEROSHOT_RUN_ID\" >> hook-order",
            "startup": "if ! test -f startup-failed-once; then touch startup-failed-once; exit 27; fi",
            "teardown": "printf 'teardown %s\\n' \"$ZEROSHOT_RUN_ID\" >> hook-order"
        }))
        .await;
    fixture
        .wait_terminal(&original, "finish", "environment_startup_failed")
        .await;
    wait_for_exit(fixture.ready_pid(&original)).await;
    fs::remove_file(fixture.root.path("environment.json")).assert_value();

    let output = fixture.output(&["resume", &original], "finish").await;
    assert_success(&output, "resume local environment run");
    let successor = receipt_run_id(&output);
    assert_ne!(successor, original);
    fixture.wait_succeeded(&successor, "finish").await;
    assert_eq!(
        fixture.hook_file("hook-order"),
        format!(
            "setup {original}\nteardown {original}\nsetup {successor}\nworker\nteardown {successor}\n"
        )
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_suspension_tears_down_and_successor_force_stop_waits_for_slow_teardown() {
    let fixture = LocalFixture::new().with_second_worker();
    let original = fixture
        .submit_with_hook_mode(
            &json!({
                "startup": r#"
sleep 300 &
printf '%s\n' "$!" > service.pid
printf 'startup %s\n' "$ZEROSHOT_RUN_ID" >> hook-order
"#,
                "teardown": r#"
kill -0 "$(cat service.pid)"
if test -f slow-teardown; then sleep 12; fi
printf 'teardown %s\n' "$ZEROSHOT_RUN_ID" >> hook-order
"#,
            }),
            "hold",
        )
        .await;
    fixture.wait_running(&original).await;
    let original_service = fixture
        .hook_file("service.pid")
        .trim()
        .parse()
        .assert_value();
    let original_controller = fixture.ready_pid(&original);
    let requested = fixture.json(&["suspend", &original], "hold").await;
    assert_eq!(requested["status"]["suspendRequested"], true);
    let release = fixture.repository.join("release-turn");
    fs::write(&release, b"release").assert_value();
    let suspended = fixture.wait_suspended(&original).await;
    assert_eq!(suspended["status"]["reason"], "command");
    assert_eq!(suspended["status"]["node"], "second");
    assert_eq!(
        fixture.hook_file("hook-order"),
        format!("startup {original}\nteardown {original}\n")
    );
    wait_for_exit(original_service).await;
    wait_for_exit(original_controller).await;
    fs::remove_file(release).assert_value();
    fs::write(fixture.repository.join("slow-teardown"), b"").assert_value();

    let resumed = fixture.json(&["resume", &original], "hold").await;
    let successor = resumed["runId"].as_str().assert_value();
    assert_eq!(resumed["resumedFrom"], original);
    let resumed_running = fixture.wait_running(successor).await;
    assert_eq!(
        resumed_running["status"]["activeExecutions"][0]["node"],
        "second"
    );
    let successor_service = fixture
        .hook_file("service.pid")
        .trim()
        .parse()
        .assert_value();
    let stop = Instant::now();
    fixture.force_stop_hook_run(successor).await;
    assert!(stop.elapsed() >= Duration::from_secs(12));
    assert_eq!(
        fixture.hook_file("hook-order"),
        format!(
            "startup {original}\nteardown {original}\nstartup {successor}\nteardown {successor}\n"
        )
    );
    wait_for_exit(successor_service).await;
}

#[cfg(target_os = "linux")]
mod compose {
    use super::*;

    // This test uses the host daemon and a cached public image. It never publishes host ports.
    const MANIFEST: &str = "\
services:
  redis:
    image: redis:8-alpine
    healthcheck:
      test: [CMD, redis-cli, ping]
      interval: 1s
      timeout: 1s
      retries: 10
    volumes:
      - data:/data
volumes:
  data:
";
    const SELECT_PROJECT: &str = "export COMPOSE_PROJECT_NAME=zs-$ZEROSHOT_RUN_ID";
    const DOCKER_HOST: &str = "unix:///var/run/docker.sock";
    const UP: &[&str] = &[
        "up",
        "-d",
        "--wait",
        "--wait-timeout",
        "30",
        "--pull",
        "never",
    ];
    const DOWN: &[&str] = &["down", "--volumes", "--remove-orphans"];

    fn docker() -> StdCommand {
        let mut command = StdCommand::new("docker");
        command
            .env_remove("DOCKER_CONTEXT")
            .env("DOCKER_HOST", DOCKER_HOST);
        command
    }

    struct ComposeProject {
        directory: PathBuf,
        project_file: PathBuf,
    }

    impl ComposeProject {
        fn new(directory: PathBuf) -> Self {
            fs::write(directory.join("compose.yml"), MANIFEST).assert_value();
            let project_file = directory.join("compose-project");
            Self {
                directory,
                project_file,
            }
        }

        fn name(&self) -> Option<String> {
            let name = fs::read_to_string(&self.project_file).ok()?;
            let name = name.trim();
            let id = name.strip_prefix("zs-")?;
            uuid::Uuid::parse_str(id).ok().map(|_| name.to_owned())
        }

        fn command(&self, arguments: &[&str]) -> std::io::Result<Output> {
            let name = self
                .name()
                .ok_or_else(|| std::io::Error::other("Compose project has no ID"))?;
            docker()
                .current_dir(&self.directory)
                .args(["compose", "-p", &name])
                .args(arguments)
                .output()
        }

        fn assert_absent(&self, run: &str) {
            assert_eq!(self.name(), Some(format!("zs-{run}")));
            let filter = format!("label=com.docker.compose.project=zs-{run}");
            for arguments in [
                vec!["ps", "-aq"],
                vec!["network", "ls", "-q"],
                vec!["volume", "ls", "-q"],
            ] {
                let output = docker()
                    .args(arguments)
                    .args(["--filter", &filter])
                    .output()
                    .assert_value();
                assert_success(&output, "inspect test project resources");
                assert!(
                    output.stdout.is_empty(),
                    "test project resources leaked: {output:?}"
                );
            }
        }
    }

    impl Drop for ComposeProject {
        fn drop(&mut self) {
            // Emergency cleanup targets only this test's generated project.
            let _ = self.command(DOWN);
        }
    }

    #[derive(Clone, Copy)]
    enum End {
        Success,
        StartupFailure,
        GraphFailure,
        ForceStop,
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a local Docker Compose daemon with redis:8-alpine cached"]
    async fn local_docker_compose_services_are_available_and_torn_down() {
        let control_root = TemporaryDirectory::for_test("zv2-compose-control");
        let control_directory = control_root.path("project");
        fs::create_dir(&control_directory).assert_value();
        let control = ComposeProject::new(control_directory);
        fs::write(
            &control.project_file,
            format!("zs-{}\n", uuid::Uuid::now_v7()),
        )
        .assert_value();
        let up = control.command(UP).assert_value();
        assert_success(&up, "start an unrelated Compose project");

        for end in [
            End::Success,
            End::StartupFailure,
            End::GraphFailure,
            End::ForceStop,
        ] {
            let graph_end = match end {
                End::Success | End::StartupFailure => "printf 'null\\n'",
                End::GraphFailure => "exit 28",
                End::ForceStop => "exec sleep 300",
            };
            let fixture = LocalFixture::new().with_hook_worker(&format!(
                r#"
{SELECT_PROJECT}
test "$(docker compose exec -T redis redis-cli ping)" = PONG
touch compose-seen-in-graph
{graph_end}
"#
            ));
            let project = ComposeProject::new(fixture.repository.clone());
            let startup_end = if matches!(end, End::StartupFailure) {
                "exit 29"
            } else {
                ":"
            };
            let run = fixture
                .submit_with_hooks(&json!({
                    "variables": {"DOCKER_HOST": DOCKER_HOST},
                    "startup": format!("{SELECT_PROJECT}\nprintf '%s\\n' \"$COMPOSE_PROJECT_NAME\" > compose-project\ndocker compose {}\n{startup_end}", UP.join(" ")),
                    "teardown": format!("{SELECT_PROJECT}\ndocker compose {}", DOWN.join(" "))
                }))
                .await;
            match end {
                End::Success => {
                    fixture.wait_succeeded(&run, "finish").await;
                }
                End::StartupFailure => {
                    fixture
                        .wait_terminal(&run, "finish", "environment_startup_failed")
                        .await;
                }
                End::GraphFailure => {
                    fixture.wait_terminal(&run, "finish", "worker_failed").await;
                }
                End::ForceStop => {
                    fixture.wait_for_hook_file("compose-seen-in-graph").await;
                    fixture.force_stop_hook_run(&run).await;
                }
            }
            assert_eq!(
                fixture.repository.join("compose-seen-in-graph").exists(),
                !matches!(end, End::StartupFailure)
            );
            project.assert_absent(&run);
            let ping = control
                .command(&["exec", "-T", "redis", "redis-cli", "ping"])
                .assert_value();
            assert_success(&ping, "unrelated Compose service stays available");
            assert_eq!(ping.stdout, b"PONG\n");
        }
    }
}
