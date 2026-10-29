"""Offline contracts for Hermes framing, usage completeness, and startup custody."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch

RUNTIME_PATH = Path(__file__).resolve().parents[1] / "native_runtime.py"
spec = importlib.util.spec_from_file_location("hermes_native_runtime_test_subject", RUNTIME_PATH)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


def _processes(snapshot, current):
    return SimpleNamespace(wait_procs=lambda *_args, **_kwargs: ([], snapshot.copy()),
                           Process=lambda: SimpleNamespace(children=lambda **_: current),
                           STATUS_ZOMBIE="zombie", STATUS_DEAD="dead", Error=Exception,
                           NoSuchProcess=type("Gone", (Exception,), {}),
                           AccessDenied=type("Denied", (Exception,), {}))


class RuntimeProtocolTests(unittest.TestCase):
    def test_rejects_truncated_and_oversized_control_frames(self):
        for wire in (b'{"type":"stop"}', b'x' * (runtime.MAX_FRAME + 1)):
            with self.assertRaises(runtime.BridgeError):
                runtime.read_frame(io.BytesIO(wire))
        self.assertIsNone(runtime.read_frame(io.BytesIO(b"")))
        self.assertEqual(runtime.read_frame(io.BytesIO(b'{"type":"stop"}\n')), {"type": "stop"})

    def test_rejects_non_object_protocol_frames(self):
        with self.assertRaises(runtime.BridgeError):
            runtime.read_frame(io.BytesIO(b'[]\n'))

    def test_malformed_prompt_stops_without_queuing_model_work(self):
        bridge = runtime.Runtime(lambda *_args, **_kwargs: None)
        bridge.interrupt = lambda: None
        bridge.controls(io.BytesIO(b'{"type":"prompt","text":{}}\n'))
        self.assertTrue(bridge.stop.is_set())
        self.assertTrue(bridge.control_failed)
        self.assertTrue(bridge.commands.empty())


class FailureDiagnosticTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("hermes_diagnostic_auth", RUNTIME_PATH.with_name("native_auth.py"))
        auth = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(auth)
        self.bridge = runtime.Runtime(lambda *_args, **_kwargs: None)
        self.bridge.authority = auth.Authority.__new__(auth.Authority)
        self.bridge.authority.redactions = {"fixture-private-credential"}
        self.native_redact = ModuleType("agent.redact")
        self.native_redact.redact_sensitive_text = lambda message, **_: message

    def test_native_failure_is_preserved_bounded_and_credential_redacted(self):
        error = "HTTP 502 from native provider: fixture-private-credential " + "x" * 5000
        with patch.dict("sys.modules", {"agent.redact": self.native_redact}):
            fields = self.bridge.failure_error({"failed": True, "error": error})
        self.assertTrue(fields["error"].startswith("HTTP 502 from native provider: [REDACTED]"))
        self.assertNotIn("fixture-private-credential", fields["error"])
        self.assertEqual(len(fields["error"]), 4096)

    def test_success_and_missing_native_errors_never_use_final_response(self):
        for result in ({"completed": True, "error": "stale", "final_response": "successful output"},
                       {"failed": True, "final_response": "response is not a diagnostic"}):
            self.assertIsNone(self.bridge.failure_error(result).get("error"))
        with patch.dict("sys.modules", {"agent.redact": self.native_redact}):
            for flag in ("partial", "interrupted"):
                self.assertEqual(self.bridge.failure_error({flag: True, "error": "native stop reason"}),
                                 {"error": "native stop reason"})


class CleanupDiagnosticTests(unittest.TestCase):
    def test_cleanup_reports_safe_stages_and_preserves_confirmed_private_details(self):
        failures = []
        def native_failure():
            raise RuntimeError("unsafe native failure contains fixture-private-credential")
        def owned_failure():
            raise runtime.BridgeError("2 native child process(es) remain (PIDs: 71, 73).")
        runtime.cleanup_action(failures, "native-clients", native_failure)
        runtime.cleanup_action(failures, "children", owned_failure)
        self.assertEqual(failures, ["native-clients", "children: 2 native child process(es) remain (PIDs: 71, 73)."])
        self.assertNotIn("fixture-private-credential", str(failures))
        bridge = runtime.Runtime(lambda *_args, **_kwargs: None)
        self.assertEqual(bridge.diagnostic(runtime.BridgeError(failures[-1])), failures[-1])

    def test_live_child_details_are_sanitized_and_dead_children_are_reaped(self):
        reaped = []
        dead = SimpleNamespace(pid=71, status=lambda: "zombie", wait=lambda **_: reaped.append(71))
        live = SimpleNamespace(pid=73, status=lambda: "sleeping", name=lambda: "fixture-private-credential")
        children = [dead, live]
        psutil = _processes(children, [dead])
        sanitize = lambda name: name.replace("fixture-private-credential", "[REDACTED]")
        with patch.dict("sys.modules", {"psutil": psutil}):
            with self.assertRaises(runtime.BridgeError) as refusal:
                runtime.confirm_children(children, sanitize)
        self.assertEqual(str(refusal.exception),
                         "1 live native child process(es) remain (PID 73, comm='[REDACTED]', status='sleeping').")
        self.assertEqual(reaped, [71])
        self.assertNotIn("fixture-private-credential", str(refusal.exception))

    def test_only_dead_children_confirm_cleanup(self):
        reaped = []
        dead = SimpleNamespace(pid=71, status=lambda: "dead", wait=lambda **_: reaped.append(71))
        psutil = _processes([], [dead])
        with patch.dict("sys.modules", {"psutil": psutil}):
            runtime.confirm_children([], str)
        self.assertEqual(reaped, [71])


def native_case(*arguments, input_text=None):
    with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-kernel-") as home:
        result = subprocess.run([sys.executable, str(Path(__file__).resolve()), *arguments],
                                cwd=home, env={**os.environ, "HERMES_HOME": home},
                                input=input_text, capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        raise AssertionError(result.stdout + result.stderr)
    return result.stdout


@unittest.skipUnless(os.environ.get("HERMES_SOURCE"), "Set HERMES_SOURCE to exercise installed native Hermes.")
class NativeKernelTests(unittest.TestCase):
    def test_private_database_keeps_native_soul_and_skills_through_cleanup(self):
        self.assertIn("native SOUL and skills preserved", native_case("--native-profile", input_text="{}\n"))

    def test_native_state_survives_turns_and_owned_shutdown_reaps_kernel(self):
        self.assertIn("native kernel reused and reaped", native_case("--native-kernel"))

    @unittest.skipUnless(os.name == "posix", "Native remote kernels use a POSIX command transport.")
    def test_native_remote_settlement_tracks_retired_and_unregistered_candidates(self):
        scenarios = ["success", "retired-live", "dropped", "malformed", "nonzero", "unregistered"]
        if sys.platform == "linux":
            scenarios.append("descendant-live")
        for scenario in scenarios:
            with self.subTest(scenario=scenario):
                report = json.loads(native_case("--native-remote-kernel", scenario))
                self.assertTrue(report["nativeRegistryRetired"])
                self.assertEqual(report["refused"], scenario not in ("success", "unregistered"))


def native_kernel_proof():
    sys.path.insert(0, os.environ["HERMES_SOURCE"])
    home = Path(os.environ["HERMES_HOME"])
    (home / "config.yaml").write_text(json.dumps({"code_execution": {"timeout": 5}, "approvals": {"mode": "manual"}}))
    from tools.approval_context import set_current_session_key
    from tools.code_execution_tool import execute_code
    from tools import code_kernel
    set_current_session_key("native-kernel-regression")
    try:
        first = json.loads(execute_code("counter=40\nprint(counter)", task_id="first-native-turn"))
        assert "40" in first["output"], first
        kernel = next(iter(code_kernel._KERNELS.values()))
        process = kernel.proc
        second = json.loads(execute_code("counter+=2\nprint(counter)", task_id="second-native-turn"))
        assert "42" in second["output"] and process.poll() is None, second
        assert next(iter(code_kernel._KERNELS.values())).proc is process
        runtime.shutdown_native_kernels()
        assert process.poll() is not None and not code_kernel._KERNELS
        print("native kernel reused and reaped")
    finally:
        code_kernel.shutdown_all_kernels()


def native_profile_proof():
    import contextvars
    import threading
    sys.path.insert(0, os.environ["HERMES_SOURCE"])
    from agent import system_prompt
    from agent.memory_provider import spawn_context_thread
    from hermes_constants import get_hermes_home_override
    from hermes_state import SessionDB
    from tools.daemon_pool import DaemonThreadPoolExecutor
    home = Path(os.environ["HERMES_HOME"])
    original_environment = {name: os.environ.get(name) for name in ("HOME", "HERMES_HOME")}
    original_override = get_hermes_home_override()
    soul_marker, skill_marker = "native-profile-soul-proof", "native-profile-skill-proof"
    (home / "config.yaml").write_text("{}\n")
    (home / "SOUL.md").write_text(soul_marker)
    skill = home / "skills" / "profile-proof"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("---\nname: profile-proof\ndescription: " + skill_marker + "\n---\nNative profile skill.\n")
    with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-private-db-") as private:
        db = SessionDB(db_path=Path(private) / "hermes.sqlite")
        agent = SimpleNamespace(_session_db=db, load_soul_identity=True, skip_context_files=False,
                                valid_tool_names={"skills_list", "skill_view"}, platform="cli")
        assert system_prompt._agent_home(agent) == Path(private)
        assert soul_marker not in "\n".join(system_prompt._identity_parts(agent, None)[0])
        assert skill_marker not in system_prompt._skills_prompt(agent)
        def inspect():
            assert system_prompt._agent_home(agent) == home
            assert soul_marker in "\n".join(system_prompt._identity_parts(agent, None)[0])
            assert skill_marker in system_prompt._skills_prompt(agent)
        class ProbeRuntime:
            def __init__(self, emit):
                self.emit, self.stop, self.usage = emit, threading.Event(), None
                self.control_failed = False
            def initialize(self, _frame):
                inspect()
                with DaemonThreadPoolExecutor(max_workers=1) as pool:
                    pool.submit(contextvars.copy_context().run, inspect).result()
                observed = []
                thread = spawn_context_thread(lambda: observed.append(system_prompt._agent_home(agent)),
                                              name="native-profile-helper-proof")
                thread.start()
                thread.join(timeout=5)
                assert not thread.is_alive() and observed == [home]
                self.stop.set()
            def controls(self, _stream):
                pass
            def request_stop(self):
                self.stop.set()
            def diagnostic(self, error):
                return type(error).__name__
            def close(self):
                inspect()
                db.close()
        original_stdout = os.dup(sys.stdout.fileno())
        try:
            with patch.object(runtime, "Runtime", ProbeRuntime):
                assert runtime.main() == 0
            assert get_hermes_home_override() == original_override
            assert {name: os.environ.get(name) for name in original_environment} == original_environment
            assert db.db_path == Path(private) / "hermes.sqlite"
            os.write(original_stdout, b"native SOUL and skills preserved\n")
        finally:
            os.close(original_stdout)
            db.close()


class LocalRemoteTransport:
    """Exercise the native remote protocol through local commands, without SSH."""

    def __init__(self, scenario):
        self.scenario = scenario

    def execute(self, command, *, cwd, timeout):
        if self.scenario == "dropped" or (self.scenario == "unregistered" and command.startswith("kill -0")):
            raise ConnectionError("fixture transport dropped")
        if "zeroshot.remote-processes/v1" in command:
            if self.scenario == "malformed":
                return {"returncode": 0, "output": "not a process receipt"}
            if self.scenario == "nonzero":
                return {"returncode": 1, "output": "state inaccessible"}
        result = subprocess.run(command, shell=True, cwd=cwd, timeout=timeout, capture_output=True, text=True)
        return {"returncode": result.returncode, "output": result.stdout + result.stderr}


def remote_test_process(scenario):
    code = 'import time; print("ready",flush=True); time.sleep(60)'
    if scenario == "retired-live":
        code = 'import signal; signal.signal(signal.SIGTERM,signal.SIG_IGN); ' + code
    if scenario == "descendant-live":
        import ctypes
        assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
        child = 'import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print("ready",flush=True); time.sleep(60)'
        code = ('import subprocess,sys,time; c=subprocess.Popen([sys.executable,"-u","-c",' + repr(child)
                + '],stdout=subprocess.PIPE,text=True); c.stdout.readline(); print(c.pid,flush=True); time.sleep(60)')
    process = subprocess.Popen([sys.executable, "-u", "-c", code], stdout=subprocess.PIPE,
                               text=True, start_new_session=True)
    ready = process.stdout.readline().strip()
    return process, int(ready) if scenario == "descendant-live" else None


def native_remote_kernel_proof(scenario):
    import signal
    sys.path.insert(0, os.environ["HERMES_SOURCE"])
    from tools import code_kernel_remote as native
    tracker = runtime.RemoteKernels()
    process, descendant = remote_test_process(scenario)
    pids = [process.pid] + ([descendant] if descendant else [])
    try:
        kernel = native.RemoteKernel(env=LocalRemoteTransport(scenario), env_type="ssh", pid=str(process.pid),
                                     kernel_dir=str(Path(os.environ["HERMES_HOME"]) / "kernel"),
                                     rpc_token="fixture-rpc-token", owner="native-remote-proof")
        if scenario == "unregistered":
            assert kernel.is_alive() is False and process.poll() is None
        else:
            native._REMOTE_KERNELS[("proof",)] = kernel
        if scenario == "retired-live":
            native._REGISTRY.discard(("proof",), kernel)
        else:
            native.shutdown_all_remote_kernels()
        assert not native._REMOTE_KERNELS
        refused = False
        try:
            tracker.confirm()
        except runtime.BridgeError:
            refused = True
        assert refused == (scenario not in ("success", "unregistered"))
        if scenario in ("success", "unregistered"):
            assert process.poll() is not None
        print(json.dumps({"scenario": scenario, "nativeRegistryRetired": True, "refused": refused}))
    finally:
        for pid in reversed(pids):
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        process.wait(timeout=5)
        if descendant:
            os.waitpid(descendant, 0)


class UsageTests(unittest.TestCase):
    def setUp(self):
        self.events = []
        self.manager = SimpleNamespace(_hooks={"pre_auxiliary_call": [lambda **_: None]})
        self.usage = runtime.Usage(self.manager, lambda kind, **fields: self.events.append({"type": kind, **fields}))
        self.agent = SimpleNamespace(session_id="worker", session_api_calls=0, session_input_tokens=0,
                                     session_output_tokens=0, session_cache_read_tokens=0, session_cache_write_tokens=0)
        self.usage.initialized = True

    def call(self, *, reported=True, session="worker", input_tokens=11, output_tokens=7):
        ident = str(self.agent.session_api_calls + 1)
        self.usage.start(api_request_id=ident, session_id=session)
        self.agent.session_api_calls += 1
        self.agent.session_input_tokens += input_tokens
        self.agent.session_output_tokens += output_tokens
        value = {"input_tokens": input_tokens, "output_tokens": output_tokens} if reported else None
        self.usage.finish(api_request_id=ident, usage=value)

    def test_complete_native_turn_reports_only_its_delta(self):
        for _ in range(2):
            self.usage.begin(self.agent)
            self.call()
            self.assertEqual(self.usage.result({"completed": True}),
                             {"input": 11, "output": 7, "cacheRead": 0, "cacheWrite": 0})
        self.assertEqual(self.events, [])
        self.assertEqual(len(self.manager._hooks["pre_auxiliary_call"]), 2)

    def test_unreported_request_preserves_counts_without_claiming_completeness(self):
        self.usage.begin(self.agent)
        self.call(reported=False)
        self.assertIsNone(self.usage.result({"completed": True}))
        self.assertEqual(self.events[-1]["usage"]["input"], 11)
        self.usage.begin(self.agent)
        self.call()
        self.assertIsNone(self.usage.result({"completed": True}))

    def test_late_auxiliary_and_child_calls_invalidate_a_complete_turn(self):
        self.usage.begin(self.agent)
        self.call()
        self.assertIsNotNone(self.usage.result({"completed": True}))
        self.usage.unknown(aux_task="title_generation")
        self.assertEqual(self.events[-1], {"type": "usage", "usage": None})
        self.usage.begin(self.agent)
        self.call(session="child")
        self.assertIsNone(self.usage.result({"completed": True}))

    def test_failed_turn_and_counter_corruption_never_report_complete_zero(self):
        for result in ({"completed": False}, {"completed": True, "interrupted": True}):
            self.usage.begin(self.agent)
            self.assertIsNone(self.usage.result(result))
        self.agent.session_input_tokens = True
        self.usage.begin(self.agent)
        self.assertIsNone(self.usage.result({"completed": True}))


if __name__ == "__main__":
    if sys.argv[1:] == ["--native-profile"]:
        native_profile_proof()
    elif sys.argv[1:] == ["--native-kernel"]:
        native_kernel_proof()
    elif len(sys.argv) == 3 and sys.argv[1] == "--native-remote-kernel":
        native_remote_kernel_proof(sys.argv[2])
    else:
        unittest.main()
