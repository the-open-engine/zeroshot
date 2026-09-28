"""Unit tests that need neither Docker nor an API key: ``python -m unittest discover -s tests``.

Requires Python 3.12 (the runner image); the ProgramBench-dependent tests also need programbench.
"""

from __future__ import annotations

import copy
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from bench import accounting, audit, config, graphs, images, report  # noqa: E402
from bench.attempt import run_files, snapshot_label  # noqa: E402

EXPERIMENT = config.load("experiments/luna-xhigh-svgbob.json")


def _tar(path: Path, files: dict[str, bytes]) -> Path:
    with tarfile.open(path, "w:gz") as tar:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return path


def _rollout(prompt_text: str, rounds: list[tuple[int, int]], continuation: dict[int, tuple[int, int]] | None = None) -> bytes:
    """A Codex transcript with one graph execution per round, each starting with the node prompt;
    cumulative usage grows by (input, output) per turn. ``continuation`` adds a second Codex turn
    (prompted "Continue", as after a provider error) to the given round."""
    lines = []
    total_in = total_out = 0

    def turn(message: str, tokens_in: int, tokens_out: int) -> None:
        nonlocal total_in, total_out
        total_in += tokens_in
        total_out += tokens_out
        lines.append({"type": "event_msg", "payload": {"type": "task_started"}})
        lines.append({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": message}]}})
        lines.append({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": total_in, "cached_input_tokens": total_in // 2, "cache_write_input_tokens": 0, "output_tokens": total_out}}}})
        lines.append({"type": "event_msg", "payload": {"type": "task_complete"}})

    for index, (tokens_in, tokens_out) in enumerate(rounds):
        turn("Execute this graph node\nAuthored instructions:\n" + prompt_text, tokens_in, tokens_out)
        if continuation and index in continuation:
            turn("Continue", *continuation[index])
    return "\n".join(json.dumps(line) for line in lines).encode()


class GraphTests(unittest.TestCase):
    def test_builder_node_is_identical_across_arms(self):
        loop = run_files(EXPERIMENT, "loop")["graph.json"]
        single = run_files(EXPERIMENT, "single")["graph.json"]
        self.assertEqual(loop["root"]["children"][0]["body"]["children"][0], single["root"]["children"][0])

    def test_prompts_are_frozen_generic_text(self):
        loop = run_files(EXPERIMENT, "loop")["graph.json"]
        build, check = loop["root"]["children"][0]["body"]["children"]
        self.assertEqual(build["instructions"], config.prompt("builder"))
        self.assertEqual(check["instructions"], config.prompt("checker"))
        for text in (build["instructions"], check["instructions"]):
            for word in ("ProgramBench", "svgbob", "SVG", "reverse", "executable"):
                self.assertNotIn(word, text)

    def test_loop_stops_on_accept_and_feeds_back_diagnostics(self):
        loop = run_files(EXPERIMENT, "loop")["graph.json"]["root"]["children"][0]
        self.assertEqual(loop["until"]["value"], {"name": "check", "source": "signal", "field": "verdict"})
        self.assertEqual(loop["maxIterations"], EXPERIMENT.max_iterations)
        self.assertEqual(loop["body"]["children"][1]["writeBindings"][0]["target"], ["feedback"])
        self.assertEqual(loop["promotedStatePaths"], [["feedback"]])

    def test_runtime_sessions(self):
        plan = graphs.runtime_plan("loop", "m", "xhigh")
        self.assertEqual(plan["nodes"]["build"]["sessionScope"], "node_instance")
        self.assertEqual(plan["nodes"]["check"]["sessionScope"], "execution")
        self.assertEqual(set(graphs.runtime_plan("single", "m", "xhigh")["nodes"]), {"build"})

    def test_task_statement_is_upstream_minus_scaffold(self):
        task = config.prompt("task")
        self.assertIn("Make sure that you have a `./compile.sh` file", task)
        for scaffold in ("COMPLETE_TASK_AND_SUBMIT", "bash tool", "system_information", "helpful assistant"):
            self.assertNotIn(scaffold, task)
        root = Path(__file__).resolve().parent.parent
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run([sys.executable, str(root / "scripts/build_task_prompt.py"), str(root / "prompts/upstream/mini-swe-agent-programbench.yaml"), f"{tmp}/task.md"], check=True)
            self.assertEqual(Path(tmp, "task.md").read_text(), (root / "prompts/task.md").read_text())


class ConfigTests(unittest.TestCase):
    def _load(self, raw):
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump(raw, f)
        try:
            return config.load(f.name)
        finally:
            os.unlink(f.name)

    def test_order_must_match_repeats(self):
        raw = copy.deepcopy(EXPERIMENT.raw)
        raw["order"] = raw["order"][:-1]
        with self.assertRaises(ValueError):
            self._load(raw)

    def test_task_image_must_be_pinned(self):
        raw = copy.deepcopy(EXPERIMENT.raw)
        raw["task"]["image"] = raw["task"]["image"].split("@")[0]
        with self.assertRaises(ValueError):
            self._load(raw)

    def test_pilot_is_five_loop_three_single_interleaved(self):
        order = EXPERIMENT.raw["order"]
        self.assertEqual((order.count("loop"), order.count("single")), (5, 3))
        self.assertEqual(set(order[:2]), {"loop", "single"})
        self.assertFalse(any(a == b == "single" for a, b in zip(order, order[1:], strict=False)))

    def test_reference_can_move_out_of_the_workspace_only(self):
        v2 = config.load("experiments/luna-xhigh-svgbob-v2.json")
        self.assertEqual(v2.reference_path, "/reference/executable")
        self.assertEqual(len(v2.doc_fixes), 4)
        for bad in ("/workspace/ref/executable", "reference/executable"):
            with self.assertRaises(ValueError):
                self._load({**EXPERIMENT.raw, "task": {**EXPERIMENT.raw["task"], "reference_path": bad}})
        with self.assertRaises(ValueError):
            self._load({**EXPERIMENT.raw, "task": {**EXPERIMENT.raw["task"], "doc_fixes": [{"file": "../etc/passwd", "old": "a", "new": "b"}]}})

    def test_task_statement_points_at_the_moved_reference(self):
        self.assertEqual(config.task_statement(EXPERIMENT), config.prompt("task"))  # unchanged upstream wording
        moved = config.task_statement(config.load("experiments/luna-xhigh-svgbob-v2.json"))
        self.assertNotIn("reference `./executable`", moved)
        self.assertNotIn("decompile `./executable`", moved)
        self.assertEqual(moved.count("`/reference/executable`"), 9)
        self.assertEqual(moved.count("`./executable`"), 1)  # the build target
        self.assertIn("produces an executable `./executable` in the workspace root", moved)
        self.assertIn("(`cp /reference/executable ./executable`)", moved)

    def test_round_schedule_must_start_at_build_1(self):
        v3 = config.load("experiments/luna-xhigh-svgbob-v3.json")
        self.assertEqual(v3.max_iterations, 50)
        for bad in ([2, 3], [1, 3, 2], [1, 1, 2], []):
            with self.assertRaises(ValueError, msg=bad):
                self._load({**v3.raw, "eval": {**v3.raw["eval"], "rounds": bad}})

    def test_v4_is_five_sol_loops_without_a_single_arm(self):
        v4 = config.load("experiments/sol-xhigh-svgbob-v4.json")
        self.assertEqual((v4.model, v4.effort, v4.max_iterations), ("gpt-5.6-sol", "xhigh", 10))
        self.assertEqual((v4.raw["order"], set(v4.raw["arms"])), (["loop"] * 5, {"loop"}))
        self.assertEqual(v4.raw["eval"]["rounds"], list(range(1, 11)))
        self.assertEqual(v4.raw["task"], config.load("experiments/luna-xhigh-svgbob-v3.json").raw["task"])

    def test_panel_tasks_rerun_luna_v3_on_new_tasks(self):
        v3 = config.load("experiments/luna-xhigh-svgbob-v3.json")
        for name, iid in (("ditaa", "stathissideris__ditaa.f2286c4"), ("chroma", "alecthomas__chroma.8d04def")):
            exp = config.load(f"experiments/luna-xhigh-{name}-v1.json")
            self.assertEqual((exp.instance_id, exp.model, exp.effort, exp.max_iterations), (iid, "gpt-5.6-luna", "xhigh", 50))
            self.assertEqual((exp.raw["order"], set(exp.raw["arms"])), (["loop"] * 5, {"loop"}))
            for key in ("limits", "pricing"):
                self.assertEqual(exp.raw[key], v3.raw[key], key)
            # Scoring runs as in v3 except for the overall time limit: ditaa's tests run one at a time
            # (about 37 minutes a workspace), so 45 or so workspaces would not fit v3's 4 hours.
            self.assertEqual({**exp.raw["eval"], "timeout_seconds": None}, {**v3.raw["eval"], "timeout_seconds": None})
            self.assertEqual(exp.raw["eval"]["timeout_seconds"], 12 * 3600)
            rule = {k: v for k, v in exp.raw["decision_rule"].items() if k != "comparison"}
            self.assertEqual(rule, {k: v for k, v in v3.raw["decision_rule"].items() if k != "comparison"})
            # The general adjustment only: the reference moves, the documentation stays as shipped.
            self.assertEqual((exp.reference_path, exp.doc_fixes), ("/reference/executable", []))
            self.assertEqual(config.task_statement(exp), config.task_statement(v3))
            smoke = config.load(f"experiments/smoke-{name}.json")
            self.assertEqual(smoke.raw["task"], exp.raw["task"])
            self.assertEqual(exp.fidelity_reference["archive_url"].split("/")[-2], iid)

    def test_fidelity_reference_defaults_to_the_pinned_svgbob_submission(self):
        self.assertEqual(config.load("experiments/sol-xhigh-svgbob-v4.json").fidelity_reference, config.pins()["fidelity_reference"])
        ditaa = config.load("experiments/luna-xhigh-ditaa-v1.json")
        wrong_task = {**ditaa.raw["task"]["fidelity_reference"], "archive_url": config.pins()["fidelity_reference"]["archive_url"]}
        for bad in (wrong_task, {**wrong_task, "archive_sha256": "abc"}, {"archive_url": ditaa.fidelity_reference["archive_url"]}):
            with self.assertRaises(ValueError):
                self._load({**ditaa.raw, "task": {**ditaa.raw["task"], "fidelity_reference": bad}})

    def test_digest_covers_code_but_not_tests_or_results(self):
        names = {str(p.relative_to(config.ROOT)) for p in config.code_files()}
        self.assertTrue({"bench/attempt.py", "bench/evaluate.py", "requirements.lock", "agent/Dockerfile"} <= names)
        self.assertFalse(any(n.startswith(("tests/", "results/")) or "/." in f"/{n}" for n in names))


class CodexConfigTests(unittest.TestCase):
    def test_rendered_config_keeps_credentials_web_and_memory_out(self):
        import tomllib

        rendered = images.codex_config({"PATH": "/usr/bin", "CARGO_HOME": "/usr/local/cargo", "HOME": "/root"})
        parsed = tomllib.loads(rendered)
        self.assertEqual(parsed["web_search"], "disabled")
        self.assertFalse(parsed["features"]["shell_snapshot"])
        self.assertFalse(parsed["memories"]["generate_memories"])
        self.assertFalse(parsed["memories"]["use_memories"])
        policy = parsed["shell_environment_policy"]
        self.assertFalse(policy["ignore_default_excludes"])
        self.assertTrue({"*KEY*", "*TOKEN*", "*PROXY*"} <= set(policy["exclude"]))
        self.assertEqual(policy["set"]["CARGO_HOME"], "/usr/local/cargo")
        self.assertEqual(policy["set"]["TMPDIR"], "/tmp")
        self.assertNotIn("HOME", policy["set"])
        self.assertEqual(parsed["projects"]["/workspace"]["trust_level"], "untrusted")


class AccountingTests(unittest.TestCase):
    PRICING = EXPERIMENT.pricing

    def test_cost_splits_cache_reads_and_writes(self):
        tokens = {"inputTokens": 1_000_000, "cacheReadInputTokens": 900_000, "cacheCreationInputTokens": 50_000, "outputTokens": 10_000}
        expected = (50_000 * 0.2 + 900_000 * 0.02 + 50_000 * 0.25 + 10_000 * 1.2) / 1e6
        self.assertAlmostEqual(accounting.cost(tokens, self.PRICING), expected)

    def test_resumed_builder_is_counted_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            _tar(Path(tmp, "trajectories.tar.gz"), {
                ".codex/sessions/2026/09/24/rollout-a.jsonl": _rollout(config.prompt("builder"), [(1000, 100), (300, 20)]),
                ".codex/sessions/2026/09/24/rollout-b.jsonl": _rollout(config.prompt("checker"), [(400, 40)]),
            })
            usage = accounting.usage(Path(tmp))
        self.assertEqual(usage["nodes"]["build"]["inputTokens"], 1300)
        self.assertEqual(usage["nodes"]["check"]["inputTokens"], 400)
        self.assertEqual(usage["nodes"]["total"]["outputTokens"], 160)
        self.assertEqual(usage["first_build_round"]["inputTokens"], 1000)
        self.assertEqual(sorted((s["node"], s["rounds"]) for s in usage["sessions"]), [("build", 2), ("check", 1)])

    def test_a_continuation_turn_belongs_to_its_round(self):
        with tempfile.TemporaryDirectory() as tmp:
            _tar(Path(tmp, "trajectories.tar.gz"), {
                ".codex/sessions/rollout-a.jsonl": _rollout(config.prompt("builder"), [(1000, 100), (300, 20)], continuation={0: (4000, 50)}),
            })
            usage = accounting.usage(Path(tmp))
        self.assertEqual(usage["first_build_round"]["inputTokens"], 5000)
        self.assertEqual(usage["nodes"]["build"]["inputTokens"], 5300)
        self.assertEqual(usage["sessions"][0]["rounds"], 2)

    def test_first_build_round_comes_from_the_earliest_builder_thread(self):
        # After a build error Zeroshot starts a new builder thread; only the earliest holds build 1.
        with tempfile.TemporaryDirectory() as tmp:
            _tar(Path(tmp, "trajectories.tar.gz"), {
                ".codex/sessions/2026/09/24/rollout-2026-09-24T02-00-00-b.jsonl": _rollout(config.prompt("builder"), [(700, 70)]),
                ".codex/sessions/2026/09/24/rollout-2026-09-24T01-00-00-a.jsonl": _rollout(config.prompt("builder"), [(1000, 100)]),
            })
            usage = accounting.usage(Path(tmp))
        self.assertEqual(usage["first_build_round"]["inputTokens"], 1000)
        self.assertEqual(usage["nodes"]["build"]["inputTokens"], 1700)

    def test_ledger_view_double_counts_resumed_sessions(self):
        events = [
            {"kind": "node_started", "reference": {"node": "build", "execution": 1}},
            {"kind": "token_usage_observed", "execution": 1, "usage": {"inputTokens": 1000, "outputTokens": 100}},
            {"kind": "node_started", "reference": {"node": "build", "execution": 3}},
            {"kind": "token_usage_observed", "execution": 3, "usage": {"inputTokens": 1300, "outputTokens": 120}},
        ]
        self.assertEqual(accounting.usage_from_events(events)["build"]["inputTokens"], 2300)  # truth: 1300


class ArchiveTests(unittest.TestCase):
    def test_tar_reports_its_own_status_and_archives_must_be_gzip(self):
        from bench import attempt

        original = attempt.docker_to_file
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp, "a.tar.gz")

            def fake(stderr: str, content: bytes):
                def docker_to_file(args, path, timeout=None, ok_codes=(0,)):
                    path.write_bytes(content)
                    return stderr
                return docker_to_file

            try:
                attempt.docker_to_file = fake("tar: ./x: file changed as we read it\nzsbench-tar-status=1\n", b"\x1f\x8b rest")
                self.assertEqual(attempt.archive("c", "agent", ["tar"], dest), (1, "tar: ./x: file changed as we read it\n"))
                attempt.docker_to_file = fake("Error response from daemon: container is not running\n", b"")
                with self.assertRaisesRegex(RuntimeError, "tar did not run"):
                    attempt.archive("c", "agent", ["tar"], dest)
                self.assertFalse(dest.exists())
                attempt.docker_to_file = fake("zsbench-tar-status=0\n", b"not gzip")
                with self.assertRaisesRegex(RuntimeError, "not gzip"):
                    attempt.archive("c", "agent", ["tar"], dest)
                self.assertFalse(dest.exists())
            finally:
                attempt.docker_to_file = original


class RerunLimitTests(unittest.TestCase):
    def test_an_attempt_that_errors_twice_is_not_run_a_third_time(self):
        from bench.attempt import Attempt

        with tempfile.TemporaryDirectory() as tmp:
            results = Path(tmp)
            spec = EXPERIMENT.attempts()[0]
            attempt = Attempt(EXPERIMENT, spec, results, "image", "proxy", {})
            attempt.dir.mkdir(parents=True)
            (attempt.dir / "attempt.json").write_text(json.dumps({"state": "error"}))
            self.assertFalse(attempt._rerun_limit_reached())  # first error: re-run once
            earlier = attempt.dir.with_name(f"{attempt.dir.name}.discarded-1")
            earlier.mkdir()
            (earlier / "attempt.json").write_text(json.dumps({"state": "stopped"}))
            self.assertFalse(attempt._rerun_limit_reached())  # an operator stop does not count
            (earlier / "attempt.json").write_text(json.dumps({"state": "error"}))
            self.assertTrue(attempt._rerun_limit_reached())


class SnapshotLabelTests(unittest.TestCase):
    def test_labels_follow_rounds_and_retries(self):
        self.assertEqual(snapshot_label("build", 1, set()), "build-1")
        self.assertEqual(snapshot_label("check", 1, {"build-1"}), "check-1")
        self.assertEqual(snapshot_label("check", 1, {"build-1", "check-1"}), "check-1.2")
        self.assertEqual(snapshot_label("build", 2, {"build-1", "check-1", "check-1.2"}), "build-2")


class AuditTests(unittest.TestCase):
    def test_command_rules(self):
        rules = audit.COMMAND_RULES
        self.assertTrue(rules["reference_binary_analysis"].search("objdump -d ./executable"))
        self.assertTrue(rules["binary_instrumentation"].search("LD_PRELOAD=./dump.so ./executable"))
        self.assertTrue(rules["binary_instrumentation"].search('os.environ["LD_AUDIT"] = "./audit.so"'))
        self.assertTrue(rules["binary_instrumentation"].search("LD_LIBRARY_PATH=./fakelibc ./executable"))
        for harmless in ("./executable < in.txt > out.svg", "unset LD_PRELOAD", "env -u LD_PRELOAD ./executable", "LD_LIBRARY_PATH=/opt/lib cc -o executable main.c"):
            self.assertFalse(rules["binary_instrumentation"].search(harmless), harmless)
        self.assertTrue(rules["reference_binary_moved_or_copied"].search("mv /workspace/executable /tmp/ref"))
        self.assertFalse(rules["reference_binary_moved_or_copied"].search("./executable -s 'x' > out.svg"))
        self.assertTrue(rules["network_fetch"].search("cargo install svgbob_cli"))
        self.assertTrue(rules["model_api_calls"].search("curl https://api.openai.com/v1/responses"))
        self.assertTrue(rules["proxy_usage"].search("HTTPS_PROXY=http://zsbench-x-proxy:8888 curl x"))
        self.assertTrue(rules["proxy_usage"].search("curl -x 172.18.0.2:8888 https://example.com"))
        self.assertFalse(rules["proxy_usage"].search("curl -s http://localhost:8888/render"))
        self.assertTrue(rules["process_environment_read"].search("cat /proc/123/environ"))
        self.assertTrue(rules["process_environment_read"].search("open(os.path.join('/proc', p, 'environ'))"))
        self.assertFalse(rules["process_environment_read"].search("python3 -c 'import os; print(os.environ)'"))
        for reading in ("ps eww", "  ps auxe", "sudo ps e", "true && ps axe", "x=$(ps e)", "timeout 5 ps e", "watch -n1 ps e", 'os.system("ps eww")', "cat /proc/self/mem"):
            self.assertTrue(rules["process_environment_read"].search(reading), reading)
        # A checker's comparison script (v3, 03-loop): `ps` is a list of subprocess results.
        comparison = "    ps=[]\n    for exe in ['/reference/executable','./executable']:\n        ps.append(run(exe))\n    a,b=ps\n    same=(a.stdout==b.stdout)\n"
        for listing in ("ps -ef", "ps aux | grep svgbob", "ps -efww", "ps -eo pid,cmd", "ps -u agent", comparison, "for ps in groups:\n    each(ps)", "# ps entries"):
            self.assertFalse(rules["process_environment_read"].search(listing), listing)
        self.assertTrue(rules["harness_internals"].search("ls /opt/codex/bin"))
        self.assertTrue(rules["harness_internals"].search("zeroshot list"))
        self.assertTrue(rules["sudo"].search("sudo apt-get install foo"))
        self.assertFalse(rules["sudo"].search("echo pseudo"))

    def test_commands_are_the_scripts_codex_ran(self):
        def completed(script, output=""):
            return {"type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "CommandExecution", "command": ["/bin/bash", "-lc", script], "aggregated_output": output}}}

        def js(code):
            return {"type": "response_item", "payload": {"type": "custom_tool_call", "input": code}}

        records = [
            {"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Authored instructions:\n" + config.prompt("builder")}]}},
            {"type": "event_msg", "payload": {"type": "task_started"}},
            # Code-mode JavaScript: identifiers such as r2 or strings are not commands.
            js('const r2 = await tools.exec_command({cmd:"./executable --help"}); const strings = [1];'),
            completed("./executable --help", "usage"),
            js('await tools.exec_command({cmd:"strings ./executable | head"})'),
            completed("strings ./executable | head", "strings: ./executable: Permission denied"),
            # Never reported as completed: still audited from the call itself.
            js("await tools.exec_command({cmd:'cat /proc/1/environ'})"),
            {"type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "FileChange", "changes": {"/tmp/dump.c": {"type": "add", "content": "long r = ptrace(PTRACE_PEEKTEXT, pid, 0, 0);"}}}}},
        ]
        with tempfile.TemporaryDirectory() as tmp:
            archive = _tar(Path(tmp, "t.tar.gz"), {".codex/sessions/rollout-a.jsonl": "\n".join(json.dumps(r) for r in records).encode()})
            result = audit.command_audit(archive)
        self.assertEqual(result["commands"], 3)
        self.assertEqual(result["rule_counts"]["reference_binary_analysis"], 1)
        self.assertEqual(result["rule_counts"]["reference_binary_analysis_denied"], 1)
        self.assertEqual(result["rule_counts"]["process_environment_read"], 1)
        self.assertEqual(result["rule_counts"]["binary_instrumentation"], 1)
        self.assertEqual(result["rule_counts_by_round"]["build.round1"]["process_environment_read"], 1)

    def test_loaded_agents_md_is_detected(self):
        # The two records Codex 0.155.0 writes when it loads a workspace AGENTS.md.
        world = {"type": "world_state", "payload": {"full": True, "state": {"agents_md": {"directory": "/workspace", "text": "x"}}}}
        message = {"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions for /workspace\n\n<INSTRUCTIONS>x"}]}}
        user_level = {"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "# AGENTS.md instructions\n\n<INSTRUCTIONS>x"}]}}
        self.assertTrue(audit.loaded_agents_md(world))
        self.assertTrue(audit.loaded_agents_md(message))
        self.assertTrue(audit.loaded_agents_md(user_level))
        self.assertFalse(audit.loaded_agents_md({"type": "world_state", "payload": {"full": True, "state": {"agents_md": {}}}}))

    def test_turns_continue_across_builder_threads(self):
        # A failed build makes Zeroshot start a new builder thread; its first turn is round 2.
        def session(script):
            return "\n".join(json.dumps(r) for r in [
                {"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Authored instructions:\n" + config.prompt("builder")}]}},
                {"type": "event_msg", "payload": {"type": "task_started"}},
                {"type": "event_msg", "payload": {"type": "item_completed", "item": {"type": "CommandExecution", "command": ["/bin/bash", "-lc", script], "aggregated_output": ""}}},
            ]).encode()

        with tempfile.TemporaryDirectory() as tmp:
            archive = _tar(Path(tmp, "t.tar.gz"), {
                ".codex/sessions/2026/09/24/rollout-2026-09-24T02-00-00-b.jsonl": session("zeroshot list"),
                ".codex/sessions/2026/09/24/rollout-2026-09-24T01-00-00-a.jsonl": session("ls"),
            })
            result = audit.command_audit(archive)
        self.assertEqual(result["rule_counts_by_round"], {"build.round2": {"harness_internals": 1}})

    def test_git_objects_scan_tolerates_absolute_links(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp, "ws")
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            (repo / "a.txt").write_text("marker-123")
            subprocess.run(["git", "-C", str(repo), "add", "a.txt"], check=True)
            subprocess.run(["git", "-C", str(repo), "-c", "user.email=a@b", "-c", "user.name=n", "commit", "-qm", "x"], check=True)
            (repo / ".venv" / "bin").mkdir(parents=True)
            os.symlink("/usr/bin/python3", repo / ".venv" / "bin" / "python3")
            archive = Path(tmp, "ws.tar.gz")
            subprocess.run(["tar", "-czf", str(archive), "-C", str(repo), "."], check=True)
            self.assertIn(b"marker-123", audit._git_objects(archive))

    def test_build_artifacts_are_not_source_edits(self):
        for path in ("__pycache__/svgbob.cpython-310.pyc", "target/release/foo", "src/x.o"):
            self.assertTrue(audit.BUILD_ARTIFACT.search(path), path)
        for path in ("svgbob.py", "src/main.rs", "compile.sh", "targets.txt"):
            self.assertFalse(audit.BUILD_ARTIFACT.search(path), path)

    def test_proxy_log_parsing(self):
        log = "\n".join([
            'CONNECT   Sep 24 01:59:05.430 [1]: Request (file descriptor 4): CONNECT example.com:443 HTTP/1.1',
            'NOTICE    Sep 24 01:59:05.430 [1]: Proxying refused on filtered domain "example.com"',
            'CONNECT   Sep 24 01:59:05.606 [1]: Request (file descriptor 4): CONNECT api.openai.com:443 HTTP/1.1',
            'CONNECT   Sep 24 01:59:05.610 [1]: Established connection to host "api.openai.com" using file descriptor 5.',
        ])
        parsed = audit.proxy_audit(log)
        self.assertEqual(parsed["established"], {"api.openai.com": 1})
        self.assertEqual(parsed["refused"], {"example.com": 1})

    def test_secret_scan_finds_key_in_archives_nested_archives_and_git_objects(self):
        fake = "sk-test-" + "x" * 40
        os.environ["OPENAI_API_KEY"] = fake
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp, "results")
                root.mkdir()
                _tar(root / "a.tar.gz", {"inner.txt": f"leaked {fake}".encode()})
                inner = _tar(Path(tmp, "inner.tar.gz"), {"deep.txt": fake.encode()})
                _tar(root / "b.tar.gz", {"nested.tar.gz": inner.read_bytes()})
                repo = Path(tmp, "repo")
                subprocess.run(["git", "init", "-q", str(repo)], check=True)
                (repo / "f.txt").write_text(fake)
                subprocess.run(["git", "-C", str(repo), "add", "f.txt"], check=True)
                subprocess.run(["git", "-C", str(repo), "-c", "user.email=a@b", "-c", "user.name=a", "commit", "-qm", "x"], check=True)
                (repo / "f.txt").unlink()
                with tarfile.open(root / "c.tar.gz", "w:gz") as tar:
                    tar.add(repo, arcname=".")
                (root / "clean.txt").write_text("nothing here")
                hits = audit.secret_scan(root)["literal_key_hits"]
        finally:
            del os.environ["OPENAI_API_KEY"]
        self.assertIn("a.tar.gz!inner.txt", hits)
        self.assertIn("b.tar.gz!nested.tar.gz!deep.txt", hits)
        self.assertIn("c.tar.gz!(git objects)", hits)
        self.assertFalse(any(h.startswith("clean") for h in hits))


class DecisionTests(unittest.TestCase):
    def _loop(self, label, first, final, **extra):
        record = {
            "label": label, "arm": "loop", "state": "complete", "build_outcomes": ["verified"], "snapshots": {},
            "rounds": {
                "build-1": {"score": first / 472, "passed": first, "scored_tests": 472, "rerun_plugin_pinned": True},
                "final": {"score": final / 472, "passed": final, "scored_tests": 472, "rerun_plugin_pinned": True},
            },
            "commands": {"rule_counts": {}, "rule_counts_by_round": {}}, "reference_copies_in_final": [], "codex_config_unchanged": True, "harness_unchanged": True,
            "gain_tests": final - first,
        }
        record.update(extra)
        record["ineligible_reasons"] = report._eligibility(record, 472)
        return record

    def test_supported_needs_every_run_to_clear_noise_and_a_large_median(self):
        runs = [self._loop(f"0{i}", 200, 200 + g) for i, g in enumerate([30, 25, 40, 28, 5])]
        self.assertEqual(report._decision(runs, 472, EXPERIMENT)["verdict"], "supported")
        runs[-1] = self._loop("09", 200, 202)  # +2 tests < 1 pp
        self.assertEqual(report._decision(runs, 472, EXPERIMENT)["verdict"], "inconclusive")

    def test_not_supported_and_incomplete(self):
        flat = [self._loop(f"0{i}", 200, 203) for i in range(5)]
        self.assertEqual(report._decision(flat, 472, EXPERIMENT)["verdict"], "not supported")
        flat[0] = self._loop("00", 200, 260, build_outcomes=["crash"])
        self.assertTrue(report._decision(flat, 472, EXPERIMENT)["verdict"].startswith("inconclusive (only 4 of 5"))
        # A build 1 that used its full time budget is a fair baseline: a single-arm build hits the same limit.
        self.assertEqual(self._loop("05", 200, 260, build_outcomes=["timeout"])["ineligible_reasons"], [])

    def test_discarded_attempts_are_reported_not_scored(self):
        with tempfile.TemporaryDirectory() as tmp:
            discarded = Path(tmp, "attempts", "03-loop.discarded-1700000000")
            discarded.mkdir(parents=True)
            (discarded / "attempt.json").write_text(json.dumps({"label": "03-loop", "state": "error", "error": "RuntimeError: boom", "wall_seconds": 12}))
            entries = report._discarded(Path(tmp), EXPERIMENT.pricing)
        self.assertEqual(entries, [{"directory": "03-loop.discarded-1700000000", "label": "03-loop", "state": "error", "error": "RuntimeError: boom", "force_stopped": None, "wall_seconds": 12, "cost_usd": None}])

    def test_compile_failures_count_and_infrastructure_errors_disqualify(self):
        # A workspace that does not compile scores 0 on every test, as on the leaderboard.
        broken = self._loop("01", 0, 240)
        broken["rounds"]["build-1"].update(error_code="compile_failed", passed=0, score=0.0)
        self.assertEqual(report._eligibility(broken, 472), [])
        flaky = self._loop("02", 200, 240)
        flaky["rounds"]["final"]["test_branch_errors"] = {"main": [{"error_code": "run_tests_failed"}]}
        self.assertTrue(any(r.startswith("final evaluation failed") for r in report._eligibility(flaky, 472)))
        unpinned = self._loop("04", 200, 240)
        unpinned["rounds"]["final"]["rerun_plugin_pinned"] = False
        self.assertIn("final evaluation failed: pinned pytest-rerunfailures was not active", report._eligibility(unpinned, 472))
        docker = self._loop("03", 200, 240)
        docker["rounds"]["build-1"]["error_code"] = "wipe_workspace_failed"
        self.assertTrue(any(r.startswith("build-1 evaluation failed") for r in report._eligibility(docker, 472)))

    def test_a_missing_codex_home_probe_counts_as_not_checked(self):
        meta = {"codex_home_surfaces": ["./config.toml 00"], "snapshots": {"build-1": {"codex_home_surfaces": ["./config.toml 00"]}, "check-1": {"probe_errors": {"codex_home_surfaces": "x"}}}, "codex_home_surfaces_end": ["./config.toml 00"]}
        self.assertEqual(report.codex_home_changes(meta), ["not checked after check-1"])

    def test_files_left_in_codex_home_make_a_run_ineligible(self):
        meta = {"codex_home_surfaces": [], "snapshots": {"build-1": {"codex_home_surfaces": ["./AGENTS.md 0123456789abcdef"]}}, "codex_home_surfaces_end": []}
        self.assertEqual(report.codex_home_changes(meta), ["./AGENTS.md 0123456789abcdef"])
        run = self._loop("01", 200, 260, home_changes=report.home_changes(meta))
        self.assertTrue(any(r.startswith("a node left files for later agent sessions") for r in run["ineligible_reasons"]))

    def test_building_the_reference_makes_a_run_ineligible(self):
        run = self._loop("01", 200, 260, reference_sha256="ab" * 32)
        run["rounds"]["final"]["executable_hash"] = "ab" * 32
        self.assertIn("final built executable is the reference", report._eligibility(run, 472))

    def test_learning_curve_lists_scored_rounds(self):
        run = self._loop("01", 200, 230)
        run["rounds"]["build-10"] = {"passed": 225}
        lines = report.learning_curve([run, {**run, "label": "02", "rounds": {"build-1": {"passed": 190}, "final": {"passed": 199}}}])
        self.assertIn("| Run | R1 | R10 | Final |", lines)
        self.assertIn("| 01 | 200 | 225 | 230 |", lines)
        self.assertIn("| 02 | 190 | — | 199 |", lines)

    def test_baseline_line_needs_a_single_arm(self):
        loops = [{"arm": "loop", "score_first_build": 0.45, "score_final": 0.55}] * 5
        self.assertEqual(report.baseline_lines(report._baseline_check(loops)), [])
        both = [*loops, {"arm": "single", "score_first_build": 0.42, "score_final": 0.42}]
        self.assertIn("Baseline check: single-arm finals mean 42.0% vs loop first builds mean 45.0%.", report.baseline_lines(report._baseline_check(both)))

    def test_disqualifying_audit_makes_a_run_ineligible(self):
        run = self._loop("01", 200, 260, commands={"rule_counts": {"process_environment_read": 1}, "rule_counts_by_round": {}})
        self.assertIn("audit: process_environment_read", run["ineligible_reasons"])


class EvalTests(unittest.TestCase):
    def test_targets_skip_discarded_attempts(self):
        from bench import evaluate

        with tempfile.TemporaryDirectory() as tmp:
            for name in ("01-loop", "01-loop.discarded-123"):
                d = Path(tmp, "attempts", name, "snapshots")
                d.mkdir(parents=True)
                _tar(d.parent / "submission.tar.gz", {"a": b"1"})
                _tar(d / "build-1.tar.gz", {"a": b"1"})
            labels = [label for label, _ in evaluate.targets(Path(tmp))]
        self.assertEqual(labels, ["01-loop__final", "01-loop__build-1"])

    def test_archive_identity_survives_copies_and_unreadable_evals_count_as_not_evaluated(self):
        from bench import evaluate

        with tempfile.TemporaryDirectory() as tmp:
            results = Path(tmp, "results")
            archive = _tar(Path(tmp, "a.tar.gz"), {"x": b"1"})
            copy = Path(tmp, "copy.tar.gz")
            copy.write_bytes(archive.read_bytes())
            self.assertEqual(evaluate.archive_id(archive), evaluate.archive_id(copy))
            attempt = results / "attempts" / "01-loop"
            attempt.mkdir(parents=True)
            _tar(attempt / "submission.tar.gz", {"x": b"1"})
            eval_json = results / "evals" / "01-loop__final" / EXPERIMENT.instance_id / f"{EXPERIMENT.instance_id}.eval.json"
            eval_json.parent.mkdir(parents=True)
            eval_json.write_text('{"test_results": [')  # killed mid-write
            original = evaluate.score_eval
            evaluate.score_eval = lambda path, iid, ignores: json.loads(path.read_text())
            try:
                scores = evaluate._scores(EXPERIMENT, results, {})
            finally:
                evaluate.score_eval = original
        self.assertEqual(scores["01-loop__final"]["error_code"], "not_evaluated")

    def test_a_round_schedule_scores_only_scheduled_builds_in_numeric_order(self):
        from bench import evaluate

        with tempfile.TemporaryDirectory() as tmp:
            snaps = Path(tmp, "attempts", "01-loop", "snapshots")
            snaps.mkdir(parents=True)
            for name in ("build-1", "check-1", "build-2", "build-10", "check-10", "build-11"):
                _tar(snaps / f"{name}.tar.gz", {"a": name.encode()})
            _tar(snaps.parent / "submission.tar.gz", {"a": b"final"})
            everything = [label for label, _ in evaluate.targets(Path(tmp))]
            scheduled = [label for label, _ in evaluate.targets(Path(tmp), [1, 2, 10])]
        self.assertEqual(everything, ["01-loop__final", "01-loop__build-1", "01-loop__check-1", "01-loop__build-2", "01-loop__build-10", "01-loop__check-10", "01-loop__build-11"])
        self.assertEqual(scheduled, ["01-loop__final", "01-loop__build-1", "01-loop__build-2", "01-loop__build-10"])

    def test_identical_workspaces_are_evaluated_once(self):
        from bench import evaluate

        def workspace(path: Path, files: dict[str, bytes], mtime: int, mode: int = 0o644) -> Path:
            with tarfile.open(path, "w:gz") as tar:
                for name, data in files.items():
                    info = tarfile.TarInfo(f"./{name}")
                    info.size, info.mtime, info.mode = len(data), mtime, mode
                    tar.addfile(info, io.BytesIO(data))
            return path

        with tempfile.TemporaryDirectory() as tmp:
            results = Path(tmp, "results")
            attempt = results / "attempts" / "01-loop"
            (attempt / "snapshots").mkdir(parents=True)
            workspace(attempt / "snapshots" / "build-1.tar.gz", {"a.py": b"v1"}, mtime=1)
            workspace(attempt / "snapshots" / "check-1.tar.gz", {"a.py": b"v1"}, mtime=2)  # same code, later snapshot
            workspace(attempt / "snapshots" / "build-2.tar.gz", {"a.py": b"v2"}, mtime=3)
            workspace(attempt / "submission.tar.gz", {"a.py": b"v2"}, mtime=4)
            self.assertNotEqual(evaluate.content_key(attempt / "snapshots" / "build-1.tar.gz"), evaluate.content_key(workspace(Path(tmp, "x.tar.gz"), {"a.py": b"v1"}, mtime=1, mode=0o755)))
            calls = []

            def fake_eval(exp, results, run_dirs, force):
                calls.append(sorted(d.name for d in run_dirs))
                for d in run_dirs:
                    (d / exp.instance_id / f"{exp.instance_id}.eval.json").write_text(json.dumps({"from": d.name}))
                return 0

            saved = evaluate._programbench_eval, evaluate.score_eval, evaluate.leaderboard_ignores
            evaluate._programbench_eval = fake_eval
            evaluate.score_eval = lambda path, iid, ignores: {"score": 0.5, "passed": 1, "scored_tests": 2, "rerun_plugin_pinned": True, "from": json.loads(path.read_text())["from"], "tests": {}}
            evaluate.leaderboard_ignores = lambda cache: {}
            try:
                scores = evaluate.evaluate(EXPERIMENT, results, Path(tmp))
            finally:
                evaluate._programbench_eval, evaluate.score_eval, evaluate.leaderboard_ignores = saved
        self.assertEqual(calls, [["01-loop__build-1", "01-loop__final"]])  # two distinct workspaces, four archives
        self.assertEqual(scores["01-loop__check-1"]["from"], "01-loop__build-1")
        self.assertEqual(scores["01-loop__check-1"]["evaluated_as"], "01-loop__build-1")
        self.assertEqual(scores["01-loop__build-2"]["from"], "01-loop__final")
        self.assertNotIn("evaluated_as", scores["01-loop__final"])

    def test_rerun_plugin_is_pinned(self):
        try:
            from bench import pbeval
        except ImportError:
            self.skipTest("programbench not installed")
        seen = []
        original = pbeval._run_step
        pbeval._run_step = lambda self, command, **kw: seen.append(command)
        try:
            pbeval._pinned_run_step(None, "pip3 install -q --disable-pip-version-check pytest-rerunfailures", env=None)
        finally:
            pbeval._run_step = original
        self.assertEqual(seen, ["pip3 install -q --disable-pip-version-check pytest-rerunfailures==16.4"])


def _load_gateway():
    import importlib.util
    spec = importlib.util.spec_from_file_location("zsbench_gateway", Path(__file__).resolve().parent.parent / "gateway" / "gateway.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _claude_record(kind: str, **fields) -> dict:
    return {"type": kind, **fields}


def _claude_prompt(text: str, timestamp: str) -> dict:
    return _claude_record("user", timestamp=timestamp, message={"role": "user", "content": "Execute this graph node\nAuthored instructions:\n" + text})


def _claude_response(message_id: str, timestamp: str, blocks: list, usage: dict) -> list[dict]:
    """Claude Code writes one record per content block, each repeating the response's usage."""
    return [_claude_record("assistant", timestamp=timestamp, requestId="req_" + message_id, message={"id": message_id, "model": "claude-opus-5", "content": [block], "usage": usage}) for block in blocks]


def _claude_archive(path: Path, files: dict[str, list[dict]]) -> Path:
    return _tar(path, {name: "\n".join(json.dumps(r) for r in records).encode() for name, records in files.items()})


class GatewayTests(unittest.TestCase):
    def test_only_custom_tools_pass(self):
        gateway = _load_gateway()
        self.assertEqual(gateway.blocked_request_features({"tools": [{"name": "Bash", "input_schema": {}}, {"type": "custom", "name": "Edit"}]}), [])
        self.assertEqual(gateway.blocked_request_features({"tools": [{"type": "web_search_20250305", "name": "web_search"}]}), ["web_search_20250305"])
        self.assertEqual(gateway.blocked_request_features({"tools": [], "mcp_servers": [{"url": "x"}]}), ["mcp_servers"])

    def test_key_replaces_client_credentials_and_responses_stay_readable(self):
        gateway = _load_gateway()
        headers = gateway.forward_headers([("X-Api-Key", "placeholder"), ("Authorization", "Bearer x"), ("Accept-Encoding", "gzip"), ("Connection", "keep-alive"), ("anthropic-version", "2023-06-01")], "sk-real")
        self.assertEqual(headers["x-api-key"], "sk-real")
        self.assertEqual(headers["accept-encoding"], "identity")
        self.assertEqual(headers["host"], "api.anthropic.com")
        self.assertNotIn("Authorization", headers)
        self.assertNotIn("Connection", headers)
        self.assertNotIn("X-Api-Key", headers)
        self.assertEqual(headers["anthropic-version"], "2023-06-01")

    def test_streamed_usage_and_cost(self):
        gateway = _load_gateway()
        parser = gateway.SSEUsage()
        start = {"type": "message_start", "message": {"model": "claude-opus-5", "usage": {"input_tokens": 10, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 5000, "output_tokens": 1, "cache_creation": {"ephemeral_5m_input_tokens": 600, "ephemeral_1h_input_tokens": 400}}}}
        delta = {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 200}}
        stream = f"event: message_start\ndata: {json.dumps(start)}\n\nevent: message_delta\ndata: {json.dumps(delta)}\n\n".encode()
        for i in range(0, len(stream), 7):  # usage survives arbitrary chunk boundaries
            parser.feed(stream[i:i + 7])
        self.assertEqual((parser.usage["output_tokens"], parser.usage["cache_read_input_tokens"], parser.stop_reason, parser.model), (200, 5000, "end_turn", "claude-opus-5"))
        prices = {"input": 5, "cached_input": 0.5, "cache_write": 6.25, "cache_write_1h": 10, "output": 25}
        expected = (10 * 5 + 5000 * 0.5 + 600 * 6.25 + 400 * 10 + 200 * 25) / 1e6
        self.assertAlmostEqual(gateway.cost_usd(parser.usage, prices), expected)
        usage, model = gateway.usage_from_body(json.dumps({"model": "claude-opus-5", "usage": {"input_tokens": 3, "output_tokens": 4}}).encode())
        self.assertEqual((usage, model), ({"input_tokens": 3, "output_tokens": 4}, "claude-opus-5"))


class ClaudeHarnessTests(unittest.TestCase):
    def test_v5_runs_claude_code_behind_the_gateway(self):
        v5 = config.load("experiments/opus5-xhigh-svgbob-v5.json")
        self.assertEqual((v5.harness, v5.provider, v5.secret_env, v5.model, v5.effort), ("claude", "anthropic", "ANTHROPIC_API_KEY", "claude-opus-5", "xhigh"))
        self.assertEqual(v5.raw["task"], config.load("experiments/sol-xhigh-svgbob-v4.json").raw["task"])
        plan = run_files(v5, "loop")["runtime.json"]
        self.assertEqual((plan["harness"], plan["provider"]), ("claude", "anthropic"))
        self.assertEqual(run_files(EXPERIMENT, "loop")["runtime.json"]["harness"], "codex")
        raw = copy.deepcopy(EXPERIMENT.raw)
        raw["limits"]["usd_cap_per_attempt"] = 100  # the cap needs the Claude gateway
        with self.assertRaises(ValueError):
            ConfigTests._load(None, raw)

    def test_placeholder_key_is_not_reported_as_a_key(self):
        self.assertEqual(audit.PLACEHOLDER_KEY, images.CLAUDE_PLACEHOLDER_KEY.encode())
        shaped: dict = {}
        audit._scan_blob("x", images.CLAUDE_PLACEHOLDER_KEY.encode(), [], [], shaped, [], 0)
        self.assertEqual(shaped, {})

    def test_launcher_isolates_every_launch(self):
        launcher = images.claude_launcher({"PATH": "/usr/local/go/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin", "HOME": "/root", "CARGO_HOME": "/usr/local/cargo", "ODD": "a b'c"})
        self.assertIn("export PATH=/usr/local/bin:/usr/bin:/bin:/usr/local/go/bin:/usr/local/cargo/bin\n", launcher)
        self.assertIn("export TMPDIR=/tmp\n", launcher)
        self.assertIn("export ODD='a b'\"'\"'c'\n", launcher)
        self.assertNotIn("export HOME=", launcher)  # Zeroshot sets HOME itself
        for switch in ("CLAUDE_CODE_DISABLE_CLAUDE_MDS=1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1", "DISABLE_TELEMETRY=1"):
            self.assertIn(f"export {switch}\n", launcher)
        exec_line = launcher.strip().splitlines()[-1]
        self.assertTrue(exec_line.startswith('exec /opt/claude-code/claude --safe-mode --setting-sources "" --disallowedTools "WebSearch,WebFetch,'), exec_line)
        self.assertTrue(exec_line.endswith('"$@"'))

    def test_claude_usage_counts_each_response_once_by_round(self):
        usage = {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 50, "cache_creation": {"ephemeral_1h_input_tokens": 40}}
        build = [
            _claude_prompt(config.prompt("builder"), "2026-09-25T10:00:00Z"),
            *_claude_response("m1", "2026-09-25T10:00:05Z", [{"type": "text", "text": "x"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}], usage),
            _claude_prompt(config.prompt("builder"), "2026-09-25T10:10:00Z"),
            *_claude_response("m2", "2026-09-25T10:10:05Z", [{"type": "text", "text": "y"}], usage),
        ]
        subagent = _claude_response("s1", "2026-09-25T10:11:00Z", [{"type": "text", "text": "z"}], usage)
        check = [_claude_prompt(config.prompt("checker"), "2026-09-25T10:05:00Z"), *_claude_response("c1", "2026-09-25T10:05:05Z", [{"type": "text", "text": "ok"}], usage)]
        with tempfile.TemporaryDirectory() as tmp:
            archive = _claude_archive(Path(tmp, "trajectories.tar.gz"), {
                ".claude/projects/-workspace/b0000000-0000-4000-8000-00000000000b.jsonl": build,
                ".claude/projects/-workspace/b0000000-0000-4000-8000-00000000000b/subagents/agent-1.jsonl": subagent,
                ".claude/projects/-workspace/c0000000-0000-4000-8000-00000000000c.jsonl": check,
            })
            sessions = accounting.claude_sessions(archive)
        self.assertEqual([s["node"] for s in sessions], ["build", "check"])  # by first timestamp
        one = {"inputTokens": 1102, "outputTokens": 50, "cacheReadInputTokens": 1000, "cacheCreationInputTokens": 100, accounting.ONE_HOUR: 40}
        rounds = sessions[0]["rounds"]
        self.assertEqual(rounds[0], one)  # m1 once, although it spans two records
        self.assertEqual(rounds[1]["inputTokens"], 2 * 1102)  # m2 plus the subagent that ran during round 2
        pricing = {"usd_per_million_tokens": {"input": 5, "cached_input": 0.5, "cache_write": 6.25, "cache_write_1h": 10, "output": 25}}
        self.assertAlmostEqual(accounting.cost(one, pricing), (2 * 5 + 1000 * 0.5 + 60 * 6.25 + 40 * 10 + 50 * 25) / 1e6)

    def test_ledger_input_includes_cache_for_claude(self):
        events = [{"kind": "node_started", "reference": {"execution": 1, "node": "check"}}, {"kind": "token_usage_observed", "execution": 1, "usage": {"inputTokens": 2, "outputTokens": 5, "cacheReadInputTokens": 10, "cacheCreationInputTokens": 3}}]
        self.assertEqual(accounting.usage_from_events(events, anthropic=True)["check"]["inputTokens"], 15)
        self.assertEqual(accounting.usage_from_events(events)["check"]["inputTokens"], 2)

    def test_claude_audit(self):
        build = [
            _claude_prompt(config.prompt("builder"), "2026-09-25T10:00:00Z"),
            *_claude_response("m1", "2026-09-25T10:00:05Z", [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "strings /reference/executable"}}], {"output_tokens": 1}),
            _claude_record("user", timestamp="2026-09-25T10:00:06Z", message={"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "strings: /reference/executable: Permission denied", "is_error": True}]}),
            *_claude_response("m2", "2026-09-25T10:00:07Z", [{"type": "tool_use", "id": "t2", "name": "Write", "input": {"file_path": "/tmp/p.c", "content": "ptrace(PTRACE_PEEKTEXT, 0, 0, 0);"}}], {"output_tokens": 1}),
            *_claude_response("m3", "2026-09-25T10:00:08Z", [{"type": "tool_use", "id": "t3", "name": "WebSearch", "input": {"query": "svgbob"}}], {"output_tokens": 1}),
            _claude_record("attachment", timestamp="2026-09-25T10:00:01Z", attachment={"type": "instructions", "files": [{"path": "/workspace/CLAUDE.md", "content": "x"}]}),
        ]
        stray = _claude_response("n1", "2026-09-25T10:20:00Z", [{"type": "text", "text": "hi"}], {"output_tokens": 1})
        with tempfile.TemporaryDirectory() as tmp:
            archive = _claude_archive(Path(tmp, "t.tar.gz"), {".claude/projects/-workspace/a.jsonl": build, ".claude/projects/-tmp/nested.jsonl": stray})
            result = audit.command_audit(archive)
        counts = result["rule_counts"]
        self.assertEqual((counts.get("reference_binary_analysis"), counts.get("reference_binary_analysis_denied"), counts.get("binary_instrumentation")), (1, 1, 1))
        self.assertEqual(counts.get("unlaunched_model_session"), 1)  # a session Zeroshot did not start
        self.assertIn("unlaunched_model_session", audit.DISQUALIFYING)
        self.assertEqual((result["web_search_calls"], result["agents_md_loaded"], result["commands"]), (1, 1, 1))
        self.assertEqual(result["rule_counts_by_round"]["build.round1"]["reference_binary_analysis"], 1)
        self.assertFalse(audit.loaded_claude_md({"type": "attachment", "attachment": {"type": "edited_text_file", "filename": "CLAUDE.md"}}))


if __name__ == "__main__":
    unittest.main()
