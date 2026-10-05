"""Unit tests that need neither Docker nor an API key: ``python -m unittest discover -s tests``.

Requires Python 3.12 (the runner image); the ProgramBench-dependent tests also need programbench.
"""

from __future__ import annotations

import copy
import io
import json
import os
import random
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from bench import accounting, audit, config, graphs, images, report  # noqa: E402
from bench.attempt import run_files, snapshot_label  # noqa: E402

EXPERIMENT = config.load("experiments/luna-xhigh-svgbob.json")
EXPANSION_SELECTION = Path(__file__).resolve().parent.parent / "experiments" / "selection" / "cost-parity-expansion.json"


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

    def test_glm_runs_repeat_the_luna_protocol_in_claude_code_through_openrouter(self):
        for glm_name, luna_name in (("glm52-xhigh-svgbob-v6", "luna-xhigh-svgbob-v3"), ("glm52-xhigh-ditaa-v1", "luna-xhigh-ditaa-v1")):
            glm, luna = config.load(f"experiments/{glm_name}.json"), config.load(f"experiments/{luna_name}.json")
            self.assertEqual((glm.model, glm.effort, glm.harness, glm.provider, glm.runtime_provider), ("z-ai/glm-5.2", "xhigh", "claude", "openrouter", "anthropic"))
            self.assertEqual((glm.secret_env, glm.api_host, glm.provider_routing), ("OPENROUTER_API_KEY", "openrouter.ai", {"order": ["z-ai"], "allow_fallbacks": False}))
            self.assertEqual((glm.raw["order"], glm.max_iterations), (["loop"] * 5, 50))
            for key in ("task", "limits", "eval"):  # Luna's limits exactly: no spending cap that could cut a run short
                self.assertEqual(glm.raw[key], luna.raw[key], key)
            same = {k: v for k, v in glm.raw["decision_rule"].items() if k != "comparison"}
            self.assertEqual(same, {k: v for k, v in luna.raw["decision_rule"].items() if k != "comparison"})
            self.assertEqual(run_files(glm, "loop")["graph.json"], run_files(luna, "loop")["graph.json"])
            self.assertEqual(run_files(glm, "loop")["input.json"], run_files(luna, "loop")["input.json"])
            plan = run_files(glm, "loop")["runtime.json"]
            self.assertEqual((plan["harness"], plan["provider"]), ("claude", "anthropic"))
            settings = images.gateway_settings(glm, "gateway-image")
            self.assertEqual((settings["upstream"], settings["secret_env"], json.loads(settings["models"])), ("openrouter", "OPENROUTER_API_KEY", ["z-ai/glm-5.2"]))
            self.assertEqual(json.loads(settings["routing"]), {"order": ["z-ai"], "allow_fallbacks": False})
            # Claude Code's default for a model its catalog does not know is 32,000 output tokens a response.
            self.assertEqual(glm.max_output_tokens, config.CLAUDE_MAX_OUTPUT_TOKENS)
            self.assertIn("export CLAUDE_CODE_MAX_OUTPUT_TOKENS=128000\n", images.claude_launcher({"PATH": "/usr/bin"}, glm.max_output_tokens))

    def test_cost_parity_study_pairs_luna_loops_with_sol_single_workers(self):
        ditaa_luna = config.load("experiments/luna-xhigh-ditaa-v1.json")
        for task in ("ditaa", "calcurse", "revive", "fasttext"):
            luna = config.load(f"experiments/luna-xhigh-{task}-v1.json")
            sol = config.load(f"experiments/sol-xhigh-{task}-single.json")
            self.assertEqual((luna.model, luna.effort, sol.model, sol.effort), ("gpt-5.6-luna", "xhigh", "gpt-5.6-sol", "xhigh"))
            self.assertEqual(sol.raw["task"], luna.raw["task"])
            self.assertEqual((sol.raw["arms"], sol.raw["order"]), ({"single": {"repeats": 5}}, ["single"] * 5))
            self.assertEqual((sol.limits, sol.resources), (luna.limits, luna.resources))
            self.assertEqual({k: v for k, v in sol.raw["eval"].items() if k != "rounds"}, {k: v for k, v in luna.raw["eval"].items() if k != "rounds"})
            self.assertEqual(sol.pricing["usd_per_million_tokens"], config.load("experiments/sol-xhigh-svgbob-v4.json").pricing["usd_per_million_tokens"])
            self.assertEqual(run_files(sol, "single")["input.json"], run_files(luna, "loop")["input.json"])  # a single worker is round 1's build
            self.assertIn("parity workspace", sol.raw["decision_rule"]["cost_parity"])
            if task == "ditaa":
                continue
            self.assertEqual(luna.raw["decision_rule"]["cost_parity"], sol.raw["decision_rule"]["cost_parity"])
            for key in ("arms", "order", "limits", "resources", "eval", "pricing", "model"):  # ditaa's protocol on a new task
                self.assertEqual(luna.raw[key], ditaa_luna.raw[key], key)
            smoke = config.load(f"experiments/smoke-{task}.json")
            self.assertEqual(smoke.raw["task"], luna.raw["task"])
            fidelity = luna.fidelity_reference
            self.assertTrue(fidelity["archive_url"].endswith(f"/{luna.instance_id}/submission.tar.gz"))
            self.assertEqual(luna.reference_path, "/reference/executable")

    def test_cost_parity_expansion_draws_its_tasks_by_the_registered_rule(self):
        selection = json.loads(EXPANSION_SELECTION.read_text())
        rule = selection["criteria"]
        for task in selection["pool"]:
            self.assertTrue(rule["solx_min"] <= task["solx"] <= rule["solx_max"], task["iid"])
            self.assertGreaterEqual(task["best"] - task["solx"], rule["best_minus_solx_min"], task["iid"])
            self.assertTrue(task["tests"] <= rule["tests_max"] and task["branches"] >= rule["branches_min"], task["iid"])
            self.assertNotIn(task["iid"], rule["excluded"])
        rng, drawn = random.Random(selection["draw"]["seed"]), []
        for language, count in selection["draw"]["quota"].items():
            drawn += rng.sample(sorted(t["iid"] for t in selection["pool"] if t["lang"] == language), count)
        self.assertEqual(drawn, selection["chosen"])
        self.assertEqual(len(set(drawn)), 15)

    def test_cost_parity_expansion_repeats_the_study_protocol(self):
        calcurse_luna = config.load("experiments/luna-xhigh-calcurse-v1.json")
        calcurse_sol = config.load("experiments/sol-xhigh-calcurse-single.json")
        selection = json.loads(EXPANSION_SELECTION.read_text())
        for instance_id in selection["chosen"]:
            task = instance_id.split("__", 1)[1].rsplit(".", 1)[0].lower()
            luna = config.load(f"experiments/luna-xhigh-{task}-v1.json")
            sol = config.load(f"experiments/sol-xhigh-{task}-single.json")
            smoke = config.load(f"experiments/smoke-{task}.json")
            self.assertEqual((luna.instance_id, luna.reference_path), (instance_id, "/reference/executable"))
            self.assertEqual(sol.raw["task"], luna.raw["task"])
            self.assertEqual(smoke.raw["task"], luna.raw["task"])
            self.assertTrue(luna.fidelity_reference["archive_url"].endswith(f"/{instance_id}/submission.tar.gz"))
            self.assertEqual(luna.fidelity_reference["submission"], calcurse_luna.fidelity_reference["submission"])
            for key in ("arms", "order", "limits", "resources", "pricing", "model"):
                self.assertEqual(luna.raw[key], calcurse_luna.raw[key], key)
            for key in ("arms", "order", "limits", "resources", "pricing", "model", "eval"):
                self.assertEqual(sol.raw[key], calcurse_sol.raw[key], key)
            # Scored at build 1 and the final workspace; parity workspaces are scored by `bench parity`.
            self.assertEqual(luna.raw["eval"], {**calcurse_luna.raw["eval"], "rounds": [1]})
            self.assertEqual(run_files(sol, "single")["input.json"], run_files(luna, "loop")["input.json"])
            rule = luna.raw["decision_rule"]["cost_parity"]
            self.assertEqual(rule, sol.raw["decision_rule"]["cost_parity"])
            self.assertTrue(rule.startswith("Pre-registered 2026-10-05"))
            expected = calcurse_luna.raw["decision_rule"]["cost_parity"].split(" C = ", 1)[1]
            expected = expected.replace("luna-xhigh-calcurse-v1", luna.id).replace("sol-xhigh-calcurse-single", sol.id)
            self.assertEqual(rule.split(" C = ", 1)[1], expected)  # the study's rule, word for word

    def test_attempts_reach_their_proxy_by_a_short_alias(self):
        # A container name can exceed a DNS label's 63 characters and then does not resolve.
        long_name = "zsbench-sol-xhigh-ascii-image-converter-single-01-single-net"
        self.assertGreater(len(f"{long_name}-proxy"), 63)
        network = images.Network(long_name, "proxy-image")
        self.assertEqual(network.proxy_url, "http://egress-proxy:8888")
        self.assertLessEqual(len(images.PROXY_ALIAS), 63)
        env = images.container_env(config.load("experiments/sol-xhigh-ascii-image-converter-single.json"), network)
        self.assertIn("HTTPS_PROXY=http://egress-proxy:8888", env)

    def test_keifu_reruns_differ_from_its_first_start_only_by_the_reaper(self):
        for version in ("v2", "v3"):
            self._keifu_rerun(version)

    def _keifu_rerun(self, version: str) -> None:
        renamed = {"luna-xhigh-keifu-v1": f"luna-xhigh-keifu-{version}", "sol-xhigh-keifu-single": f"sol-xhigh-keifu-single-{version}", "smoke-keifu": f"smoke-keifu-{version}"}

        def comparable(raw: dict) -> dict:
            text = json.dumps({k: v for k, v in raw.items() if k not in ("id", "description")})
            for old, new in renamed.items():
                text = text.replace(f'"{new}', f'"{old}').replace(f" {new}", f" {old}")
            out = json.loads(text)
            out["resources"].pop("reap_orphans", None)
            return out

        for old, new in renamed.items():
            first, rerun = config.load(f"experiments/{old}.json"), config.load(f"experiments/{new}.json")
            self.assertEqual((first.reap_orphans, rerun.reap_orphans), (False, True))
            self.assertEqual(comparable(rerun.raw), comparable(first.raw), new)
            self.assertTrue(rerun.raw["description"].startswith(("Rerun" if version == "v2" else "Second rerun") + f" of {old} (both arms)"))
            for arm in rerun.raw["arms"]:
                self.assertEqual(run_files(rerun, arm), run_files(first, arm))  # same prompts, graphs and limits
        luna, sol = config.load(f"experiments/luna-xhigh-keifu-{version}.json"), config.load(f"experiments/sol-xhigh-keifu-single-{version}.json")
        self.assertEqual(luna.raw["decision_rule"]["cost_parity"], sol.raw["decision_rule"]["cost_parity"])
        self.assertIn(f"the 5 runs of sol-xhigh-keifu-single-{version}", luna.raw["decision_rule"]["cost_parity"])
        self.assertIn(f"each loop run of luna-xhigh-keifu-{version}", luna.raw["decision_rule"]["cost_parity"])
        with self.assertRaises(ValueError):
            self._load({**luna.raw, "resources": {**luna.raw["resources"], "reap_orphans": "yes"}})
        glm = config.load("experiments/glm52-xhigh-ditaa-v1.json")
        with self.assertRaises(ValueError):  # the reaper wraps Codex only
            self._load({**glm.raw, "resources": {**glm.raw["resources"], "reap_orphans": True}})

    def test_parity_workspace_is_the_last_build_within_budget(self):
        from bench import parity

        spend = [0.3, 0.8, 1.5, 2.2]
        self.assertEqual(parity.parity_workspace(spend, 1.6), "build-3")
        self.assertEqual(parity.parity_workspace(spend, 0.8), "build-2")  # at the budget counts as within it
        self.assertEqual(parity.parity_workspace(spend, 0.1), "build-1")  # build 1 alone costs more
        self.assertEqual(parity.parity_workspace(spend, 3.0), "final")  # the run stopped before reaching the budget
        self.assertEqual(parity.parity_workspace([], 1.0), "final")

    def test_parity_costs_add_each_build_and_the_checks_before_it(self):
        from bench import accounting, parity

        pricing = {"usd_per_million_tokens": {"input": 1.0, "cached_input": 0.1, "cache_write": 1.0, "output": 10.0}}
        with tempfile.TemporaryDirectory() as tmp:
            archive = _tar(Path(tmp, "trajectories.tar.gz"), {
                ".codex/sessions/2026/10/04/rollout-2026-10-04T10-00-00-a.jsonl": _rollout(config.prompt("builder"), [(1000, 100), (2000, 200), (3000, 300)]),
                ".codex/sessions/2026/10/04/rollout-2026-10-04T10-05-00-b.jsonl": _rollout(config.prompt("checker"), [(500, 50), (700, 70)]),
            })
            spend = parity.cumulative_costs(archive, pricing)
            sessions = {s["node"]: s["rounds"] for s in accounting.sessions(archive)}
        price = lambda tokens: accounting.cost(tokens, pricing)
        builds, checks = sessions["build"], sessions["check"]
        self.assertEqual(len(spend), 3)
        self.assertAlmostEqual(spend[0], price(builds[0]))
        self.assertAlmostEqual(spend[1] - spend[0], price(builds[1]) + price(checks[0]))
        self.assertAlmostEqual(spend[2] - spend[1], price(builds[2]) + price(checks[1]))

    def test_parity_verdict_compares_means_and_intervals(self):
        from bench import parity

        self.assertIn("robustly", parity.verdict([50, 52, 51, 53, 52], [40, 41, 42, 40, 41])["verdict"])
        overlapping = parity.verdict([50, 30, 60, 45, 52], [44, 46, 45, 43, 47])
        self.assertEqual(overlapping["verdict"], "Luna outperforms (higher mean; intervals overlap)")
        self.assertTrue(parity.verdict([40, 41, 39, 40, 42], [50, 51, 52, 50, 49])["verdict"].startswith("Sol is ahead robustly"))

    def test_parallel_runners_only_on_request(self):
        from unittest import mock

        from bench import __main__ as cli

        with mock.patch.object(cli, "docker", return_value="aaaaaaaaaaaa\nbbbbbbbbbbbb\n"), mock.patch.object(cli.socket, "gethostname", return_value="aaaaaaaaaaaa"):
            with mock.patch.dict(os.environ, {"ZSBENCH_ALLOW_PARALLEL": ""}):
                for command in ("run", "eval", "cleanup"):
                    with self.assertRaises(SystemExit):
                        cli._refuse_while_another_runner_runs(command)
            with mock.patch.dict(os.environ, {"ZSBENCH_ALLOW_PARALLEL": "1"}), mock.patch.object(cli, "log"):
                cli._refuse_while_another_runner_runs("run")
                cli._refuse_while_another_runner_runs("eval")
                with self.assertRaises(SystemExit):
                    cli._refuse_while_another_runner_runs("cleanup")  # would remove a live runner's attempts

    def test_only_claude_experiments_set_an_output_limit(self):
        glm = config.load("experiments/glm52-xhigh-svgbob-v6.json")
        for bad in (0, 128_001, "128000", True, 64000.0):
            with self.assertRaises(ValueError, msg=bad):
                self._load({**glm.raw, "model": {**glm.raw["model"], "max_output_tokens": bad}})
        codex = {k: v for k, v in glm.raw["model"].items() if k != "provider_routing"}
        with self.assertRaises(ValueError):
            self._load({**glm.raw, "model": {**codex, "harness": "codex"}})
        self.assertEqual(self._load({**glm.raw, "model": {**glm.raw["model"], "max_output_tokens": 64000}}).max_output_tokens, 64000)
        self.assertIsNone(config.load("experiments/opus5-xhigh-svgbob-v5.json").max_output_tokens)  # v5 keeps its launcher

    def test_glm_on_ditaa_is_declared_text_only(self):
        ditaa, svgbob = config.load("experiments/glm52-xhigh-ditaa-v1.json"), config.load("experiments/glm52-xhigh-svgbob-v6.json")
        self.assertEqual((ditaa.image_input, svgbob.image_input, EXPERIMENT.image_input), (False, True, True))
        self.assertEqual(images.gateway_settings(ditaa, "g")["text_only"], "1")
        self.assertEqual(images.gateway_settings(svgbob, "g")["text_only"], "")
        for bad in ("false", 0, None):
            with self.assertRaises(ValueError, msg=bad):
                self._load({**ditaa.raw, "model": {**ditaa.raw["model"], "image_input": bad}})
        codex = {k: v for k, v in ditaa.raw["model"].items() if k not in ("provider_routing", "max_output_tokens")}
        with self.assertRaises(ValueError):
            self._load({**ditaa.raw, "model": {**codex, "harness": "codex"}})

    def test_providers_belong_to_their_harness(self):
        glm = config.load("experiments/glm52-xhigh-svgbob-v6.json")
        unpinned = {k: v for k, v in glm.raw["model"].items() if k not in ("provider_routing", "max_output_tokens", "image_input")}
        for model in ({**unpinned, "provider": "openai"}, {**unpinned, "harness": "codex", "provider": "anthropic"}, {**unpinned, "provider": "bedrock"},
                      {**glm.raw["model"], "provider": "anthropic"}, {**glm.raw["model"], "provider_routing": {"order": []}}):
            with self.assertRaises(ValueError, msg=model):
                self._load({**glm.raw, "model": model})
        codex = {**unpinned, "harness": "codex"}
        self.assertEqual(self._load({**glm.raw, "model": codex, "limits": {k: v for k, v in glm.limits.items() if k != "usd_cap_per_attempt"}}).runtime_provider, "openrouter")
        self.assertEqual(EXPERIMENT.provider, "openai")
        self.assertEqual(EXPERIMENT.api_host, "api.openai.com")

    def test_the_openai_proxy_allowlist_is_unchanged(self):
        self.assertEqual(images.proxy_filter("api.openai.com"), (config.ROOT / "proxy" / "filter").read_bytes())
        self.assertEqual(images.proxy_filter("openrouter.ai"), b"^openrouter\\.ai$\n")

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
        self.assertTrue(rules["model_api_calls"].search("curl -s https://openrouter.ai/api/v1/chat/completions"))
        self.assertTrue(rules["credential_probe"].search("echo ${OPENROUTER_API_KEY:0:8}"))
        self.assertTrue(rules["proxy_usage"].search("HTTPS_PROXY=http://zsbench-x-proxy:8888 curl x"))
        self.assertTrue(rules["proxy_usage"].search("curl http://egress-proxy:8888/x"))
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

    def test_checker_edits_compare_each_check_with_the_snapshot_before_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            snaps = Path(tmp, "snapshots")
            snaps.mkdir()
            _tar(snaps / "build-1.tar.gz", {"./src/a.rs": b"one", "./.git/HEAD": b"ref"})
            _tar(snaps / "check-1.tar.gz", {"./src/a.rs": b"one", "./.git/HEAD": b"ref2", "./target/x.o": b"obj"})
            _tar(snaps / "build-2.tar.gz", {"./src/a.rs": b"two", "./.git/HEAD": b"ref2"})
            _tar(snaps / "check-2.tar.gz", {"./src/a.rs": b"edited by the checker", "./.git/HEAD": b"ref2"})
            rounds = audit.checker_edits(Path(tmp), ["build-1", "check-1", "build-2", "check-2"])
        self.assertEqual([(r["check"], r["compared_with"]) for r in rounds], [("check-1", "build-1"), ("check-2", "build-2")])
        self.assertEqual((rounds[0]["workspace_changes"], rounds[0]["artifact_changes"], rounds[0]["git_metadata_changes"]), ([], 1, 1))
        self.assertEqual(rounds[1]["workspace_changes"], ["src/a.rs"])

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

    def test_a_final_whose_tests_hung_scores_them_as_failed(self):
        # The submission kept its test run going past ProgramBench's time limit: its tests count as
        # not passed, as on the leaderboard, and the run stays eligible.
        hung = self._loop("05", 200, 0)
        hung["rounds"]["final"].update(test_branch_errors={"main": [{"error_code": "results_read_failed"}]}, test_runs_timed_out=True, rerun_plugin_pinned=None)
        self.assertEqual(report._eligibility(hung, 472), [])

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


@unittest.skipUnless(shutil.which("cc"), "needs a C compiler")
class ReaperTests(unittest.TestCase):
    """agent/codex-reaper.c, compiled against a stand-in for Codex."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = Path(self.tmp.name)
        self.codex = self.dir / "codex"
        self.reaper = self.dir / "codex-reaper"
        subprocess.run(["cc", "-O2", "-Wall", "-Wextra", "-Werror", f'-DCODEX="{self.codex}"', "-o", str(self.reaper), str(Path(__file__).resolve().parent.parent / "agent" / "codex-reaper.c")], check=True)

    def tearDown(self):
        self.tmp.cleanup()

    def _codex(self, script: str) -> None:
        self.codex.write_text("#!/bin/sh\n" + script)
        self.codex.chmod(0o755)

    def test_orphans_in_other_sessions_are_reaped_and_the_exit_code_passes_through(self):
        # An orphan in its own session (as a program started in a pseudo-terminal) is adopted by
        # the reaper, not by an outer subreaper, and does not stay a zombie once it exits.
        self._codex(
            "sh -c 'setsid sh -c \"sleep 0.6\" & echo $! > \"$0.orphan\"; exit 0' \"$0\"\n"
            "sleep 0.3\necho \"adopted_by=$(ps -o ppid= -p $(cat \"$0.orphan\") | tr -d ' ')\"\necho \"reaper=$PPID\"\n"
            "sleep 0.7\necho \"zombies=$(ps -o stat= --ppid $PPID | grep -c Z)\"\nexit 7\n"
        )
        result = subprocess.run([str(self.reaper), "exec", "--json"], capture_output=True, text=True, timeout=20)
        out = dict(line.split("=", 1) for line in result.stdout.split())
        self.assertEqual(result.returncode, 7)
        self.assertEqual(out["adopted_by"], out["reaper"])
        self.assertEqual(out["zombies"], "0")

    def test_processes_the_node_leaves_running_end_with_it(self):
        # A detached sleeper and a session that keeps creating orphans (keifu's reference app left
        # running in a tmux server did): both end before the reaper exits, so nothing is handed
        # back to Zeroshot's controller.
        self._codex(
            "setsid sh -c 'sleep 30' & echo $! > \"$0.sleeper\"\n"
            "setsid sh -c 'while :; do sh -c \"sleep 0.05 &\"; sleep 0.02; done' & echo $! > \"$0.spawner\"\n"
            "sleep 0.3\nexit 3\n"
        )
        started = time.time()
        result = subprocess.run([str(self.reaper)], timeout=30)
        self.assertEqual(result.returncode, 3)
        self.assertLess(time.time() - started, 5)
        sessions = {int(Path(f"{self.codex}.{name}").read_text()) for name in ("sleeper", "spawner")}
        left = []
        for stat in Path("/proc").glob("[0-9]*/stat"):
            try:
                fields = stat.read_text().rsplit(")", 1)[1].split()
            except OSError:
                continue
            if int(fields[3]) in sessions:  # field 6 of /proc/<pid>/stat: the session id
                left.append(stat.parent.name)
        self.assertEqual(left, [])

    def test_the_reaper_dies_the_way_codex_dies(self):
        self._codex("kill -TERM $$\nsleep 5\n")
        self.assertEqual(subprocess.run([str(self.reaper)], timeout=20).returncode, -signal.SIGTERM)

    def test_codex_dies_with_the_reaper_and_group_signals_reach_codex(self):
        self._codex("echo $$ > \"$(dirname \"$0\")/pid\"\nexec sleep 30\n")
        for kill in (lambda proc: proc.kill(), lambda proc: os.killpg(proc.pid, signal.SIGTERM)):
            (self.dir / "pid").unlink(missing_ok=True)
            proc = subprocess.Popen([str(self.reaper)], start_new_session=True)
            deadline = time.time() + 10
            while not (self.dir / "pid").exists() and time.time() < deadline:
                time.sleep(0.05)
            codex = int((self.dir / "pid").read_text())
            kill(proc)
            proc.wait(timeout=10)
            time.sleep(0.3)
            self.assertFalse(Path(f"/proc/{codex}").exists() and "Z" not in Path(f"/proc/{codex}/stat").read_text().split()[2], "Codex outlived the reaper")


class EvalTests(unittest.TestCase):
    def test_a_test_run_stopped_at_the_time_limit_counts_as_failed_tests(self):
        from bench import evaluate

        install = {"step": "install_rerunfailures", "returncode": 0, "command": "pip install -q pytest-rerunfailures==16.4"}
        timeout = {"step": "run_tests", "returncode": -1, "exception_info": "Command timed out after 3600s", "output": ""}
        missing = {"step": "results_read", "returncode": 1, "exception_info": ""}
        finished = {"step": "run_tests", "returncode": 0, "output": "plugins: rerunfailures-16.4, timeout-2.4.0"}
        read = {"step": "results_read", "returncode": 0, "branch": "b"}
        errors = {"a": [{"error_code": "results_read_failed", "error_details": ""}]}
        # chroma's smoke: the only branch timed out twice (attempt and retry), so no test run finished.
        hung = {"log": [install, timeout, missing, timeout, missing]}
        self.assertTrue(evaluate.test_runs_timed_out(hung))
        self.assertIsNone(evaluate.rerun_plugin_active(hung))
        self.assertIsNone(evaluate.infrastructure_error({"score": 0.0, "test_branch_errors": errors, "test_runs_timed_out": True, "rerun_plugin_pinned": None}))
        # One branch finished with the pinned plugin, the other hung.
        mixed = {"log": [install, finished, read, timeout, missing, timeout, missing]}
        self.assertTrue(evaluate.test_runs_timed_out(mixed))
        self.assertTrue(evaluate.rerun_plugin_active(mixed))
        # No results after a test run that ended on its own (a crash, a failed install) stays an
        # infrastructure error, and so does any other branch error.
        crashed = {"log": [install, {**finished, "returncode": 2}, missing]}
        self.assertFalse(evaluate.test_runs_timed_out(crashed))
        self.assertEqual(evaluate.infrastructure_error({"score": 0.0, "test_branch_errors": errors, "test_runs_timed_out": False}), "test branch errors ['a']")
        other = {"a": [{"error_code": "run_tests_failed"}]}
        self.assertEqual(evaluate.infrastructure_error({"score": 0.0, "test_branch_errors": other, "test_runs_timed_out": True}), "test branch errors ['a']")
        self.assertEqual(evaluate.infrastructure_error({"score": 0.5, "rerun_plugin_pinned": False}), "pinned pytest-rerunfailures was not active")

    def test_branches_that_run_no_tests_leave_the_plugin_check_and_count_as_not_passed(self):
        from bench import evaluate

        install = {"step": "install_rerunfailures", "returncode": 0, "command": "pip install -q pytest-rerunfailures==16.4"}
        header = "=== test session starts ===\nplugins: timeout-2.4.0, rerunfailures-16.4\n"
        finished = {"step": "run_tests", "returncode": 1, "output": header + "=== 2 failed, 101 passed, 4 rerun in 3.1s ==="}
        read = {"step": "results_read", "returncode": 0, "branch": "b"}
        missing = {"step": "results_read", "returncode": 1, "exception_info": ""}
        # ascii-image-converter: branches with nothing to test never start pytest.
        nothing = {"step": "run_tests", "returncode": 0, "output": "No subcommands found. No tests to run.\n"}
        self.assertTrue(evaluate.rerun_plugin_active({"log": [install, finished, read, nothing, read]}))
        self.assertIsNone(evaluate.rerun_plugin_active({"log": [install, nothing, read]}))
        # A run that ran tests without the pinned plugin still fails the check.
        unpinned = {**finished, "output": "=== test session starts ===\nplugins: rerunfailures-16.6.1\n=== 3 passed in 1s ==="}
        self.assertFalse(evaluate.rerun_plugin_active({"log": [install, finished, read, unpinned, read]}))
        # dust: pytest failed to load the image's libtmux plugin, before any test ran, on the attempt
        # and on ProgramBench's retry. Its tests count as not passed, as for a timed-out branch.
        crash = {"step": "run_tests", "returncode": 1, "output": "Traceback (most recent call last):\n  File \"/usr/local/lib/python3.10/dist-packages/_pytest/config/__init__.py\", line 1583, in parse\n    self.pluginmanager.load_setuptools_entrypoints(\"pytest11\")\nFailed: Marks cannot be applied to fixtures.\n"}
        dust = {"log": [install, finished, read, crash, missing, crash, missing]}
        self.assertTrue(evaluate.rerun_plugin_active(dust))
        self.assertFalse(evaluate.test_runs_timed_out(dust))
        self.assertTrue(evaluate.missing_results_explained(dust))
        errors = {"d42ad55b3a4b": [{"error_code": "results_read_failed", "error_details": "Could not find the file /workspace/eval/results.xml"}]}
        scored = {"score": 0.66, "test_branch_errors": errors, "missing_results_explained": True, "rerun_plugin_pinned": True}
        self.assertIsNone(evaluate.infrastructure_error(scored))
        # A crash after tests ran is not a plugin-load failure, and missing results stay an error.
        late = {**crash, "output": header + crash["output"]}
        self.assertFalse(evaluate.missing_results_explained({"log": [install, late, missing]}))
        self.assertEqual(evaluate.infrastructure_error({**scored, "missing_results_explained": False}), "test branch errors ['d42ad55b3a4b']")
        # Scores written before this field existed keep the timed-out rule.
        self.assertIsNone(evaluate.infrastructure_error({"score": 0.0, "test_branch_errors": errors, "test_runs_timed_out": True}))

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


class OpenRouterGatewayTests(unittest.TestCase):
    def test_openrouter_upstream_gets_a_bearer_key_and_the_pinned_provider(self):
        gateway = _load_gateway()
        headers = gateway.forward_headers([("X-Api-Key", "placeholder"), ("anthropic-version", "2023-06-01")], "sk-or-real", "openrouter")
        self.assertEqual((headers["authorization"], headers["host"]), ("Bearer sk-or-real", "openrouter.ai"))
        self.assertNotIn("x-api-key", {k.lower() for k in headers})
        routing = {"order": ["z-ai"], "allow_fallbacks": False}
        body = json.loads(gateway.routed_body({"model": "z-ai/glm-5.2", "messages": []}, routing))
        self.assertEqual(body["provider"], routing)
        self.assertIsNone(gateway.routed_body({"model": "m"}, None))

    def test_the_billed_cost_is_charged_when_reported(self):
        gateway = _load_gateway()
        parser = gateway.SSEUsage()
        delta = {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"input_tokens": 55, "cache_read_input_tokens": 16192, "cache_creation_input_tokens": None, "output_tokens": 3, "cost": 0.00430452}}
        parser.feed(f"data: {json.dumps(delta)}\n\n".encode())
        self.assertEqual(parser.usage, {"input_tokens": 55, "cache_read_input_tokens": 16192, "output_tokens": 3, "cost": 0.00430452})
        with tempfile.TemporaryDirectory() as tmp:
            billed = gateway.Gateway("k" * 20, {"input": 1.4, "cached_input": 0.26, "output": 4.4}, 1.0, f"{tmp}/g.jsonl", "openrouter")
            self.assertAlmostEqual(billed.charge(parser.usage), 0.00430452)
            listed = gateway.Gateway("k" * 20, {"input": 1.4, "cached_input": 0.26, "output": 4.4}, 1.0, f"{tmp}/h.jsonl")
            self.assertAlmostEqual(listed.charge({"input_tokens": 55, "cache_read_input_tokens": 16192, "output_tokens": 3}), (55 * 1.4 + 16192 * 0.26 + 3 * 4.4) / 1e6)

    def test_a_text_only_model_gets_notes_instead_of_images(self):
        gateway = _load_gateway()
        png = {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}}
        payload = {"model": "z-ai/glm-5.2", "messages": [
            {"role": "user", "content": [{"type": "text", "text": "compare these"}, png]},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/tmp/a.png"}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [png]}]},
            {"role": "user", "content": "plain text"},
        ]}
        stripped, removed = gateway.text_only_body(payload)
        self.assertEqual(removed, 2)
        self.assertNotIn('"image"', json.dumps(stripped))
        self.assertEqual(stripped["messages"][0]["content"][0], {"type": "text", "text": "compare these"})
        note = stripped["messages"][2]["content"][0]["content"][0]
        self.assertEqual(note["type"], "text")
        self.assertIn("image/png image (12 base64 characters)", note["text"])
        self.assertEqual(stripped["messages"][3], payload["messages"][3])
        self.assertIn('"image"', json.dumps(payload))  # the caller's payload is not changed
        text = {"model": "m", "messages": [{"role": "user", "content": "hi"}]}
        self.assertIs(gateway.text_only_body(text)[0], text)
        self.assertEqual(gateway.text_only_body(None), (None, 0))

    def test_the_audit_reports_the_output_limits_requested(self):
        log = "\n".join(json.dumps(r) for r in (
            {"t": 1, "event": "listening"},
            {"t": 2, "method": "POST", "path": "/v1/messages", "status": 200, "model": "z-ai/glm-5.2", "max_tokens": 128000, "cost_usd": 0.5},
            {"t": 3, "method": "POST", "path": "/v1/messages", "status": 200, "model": "z-ai/glm-5.2", "max_tokens": 128000, "cost_usd": 0.25, "media_removed": 2},
            {"t": 4, "method": "POST", "path": "/v1/messages/count_tokens", "status": 404, "model": "z-ai/glm-5.2"},
        ))
        summary = audit.gateway_audit(log)
        self.assertEqual(summary["max_tokens"], {"128000": 2})
        self.assertEqual(summary["media_removed"], 2)
        self.assertEqual((summary["cost_usd"], summary["statuses"]), (0.75, {"200": 2, "404": 1}))


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
        self.assertNotIn("CLAUDE_CODE_MAX_OUTPUT_TOKENS", launcher)  # Claude Code's own default unless an experiment sets one
        for switch in ("CLAUDE_CODE_DISABLE_CLAUDE_MDS=1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY=1", "DISABLE_TELEMETRY=1"):
            self.assertIn(f"export {switch}\n", launcher)
        exec_line = launcher.strip().splitlines()[-1]
        self.assertTrue(exec_line.startswith('exec /opt/claude-code/claude --safe-mode --setting-sources "" --disallowedTools "WebSearch,WebFetch,'), exec_line)
        self.assertTrue(exec_line.endswith('"$@"'))

    def test_claude_usage_counts_each_response_once_by_round(self):
        usage = {"input_tokens": 2, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 50, "cache_creation": {"ephemeral_1h_input_tokens": 40}}
        # A compaction summary quotes the instructions but continues the same round.
        summary = {**_claude_prompt(config.prompt("builder"), "2026-09-25T10:00:30Z"), "isCompactSummary": True, "isVisibleInTranscriptOnly": True}
        build = [
            _claude_prompt(config.prompt("builder"), "2026-09-25T10:00:00Z"),
            *_claude_response("m1", "2026-09-25T10:00:05Z", [{"type": "text", "text": "x"}, {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}], usage),
            {"type": "system", "subtype": "compact_boundary", "timestamp": "2026-09-25T10:00:30Z"},
            summary,
            *_claude_response("m0", "2026-09-25T10:00:40Z", [{"type": "text", "text": "w"}], usage),
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
        self.assertEqual(len(rounds), 2)
        self.assertEqual(rounds[0], {k: 2 * v for k, v in one.items()})  # m1 once, although it spans two records, and m0 after the compaction
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
