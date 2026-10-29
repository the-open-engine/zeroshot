"""Execution-owned bridge to Hermes' native Python agent and launch-policy helpers."""
import importlib.util
from functools import partial, wraps
import json
import os
from pathlib import Path
import queue
import signal
import sys
import threading
import time

MAX_FRAME = 64 * 1024 * 1024
MAX_COUNTER = 2**64 - 1
COUNTERS = ("session_input_tokens", "session_output_tokens", "session_cache_read_tokens", "session_cache_write_tokens")


class BridgeError(Exception):
    """A credential-free diagnostic that may cross the private driver protocol."""


def read_frame(stream):
    line = stream.readline(MAX_FRAME + 1)
    if not line:
        return None
    if len(line) > MAX_FRAME or not line.endswith(b"\n"):
        raise BridgeError("Hermes bridge received an oversized or incomplete frame.")
    value = json.loads(line)
    if not isinstance(value, dict):
        raise BridgeError("Hermes bridge expected a JSON object.")
    return value


def sibling(name):
    path = Path(__file__).with_name(name + ".py")
    spec = importlib.util.spec_from_file_location("_zeroshot_hermes_" + name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def snapshot(agent):
    values = tuple(getattr(agent, name, None) for name in COUNTERS)
    if any(type(value) is not int or not 0 <= value <= MAX_COUNTER for value in values):
        return None
    return values


class Usage:
    """Native counters remain useful when additional native requests make totals unknown."""

    def __init__(self, manager, emit):
        self.emit, self.agent = emit, None
        self.invalid = self.active = self.initialized = False
        self.before = None
        self.calls_before = None
        self.started, self.finished = set(), set()
        self.lock = threading.RLock()
        observers = {"pre_api_request": self.start, "post_api_request": self.finish,
                     "api_request_error": self.unknown, "pre_auxiliary_call": self.unknown}
        for name, callback in observers.items():
            manager._hooks.setdefault(name, []).append(callback)

    def unknown(self, **_):
        with self.lock:
            first = not self.invalid
            self.invalid = True
            if first and self.initialized and not self.active:
                self.emit("usage", usage=None)

    def start(self, **event):
        with self.lock:
            ident = event.get("api_request_id")
            if (self.agent is None or event.get("session_id") != self.agent.session_id
                    or not isinstance(ident, str) or not ident or len(self.started) >= 4096):
                self.unknown()
            else:
                self.started.add(ident)

    def finish(self, **event):
        with self.lock:
            usage, ident = event.get("usage"), event.get("api_request_id")
            if (not isinstance(usage, dict) or not isinstance(ident, str) or ident not in self.started
                    or any(type(usage.get(key)) is not int or not 0 <= usage[key] <= MAX_COUNTER
                           for key in ("input_tokens", "output_tokens"))):
                self.unknown()
            elif len(self.finished) < 4096:
                self.finished.add(ident)
            else:
                self.unknown()

    def begin(self, agent):
        with self.lock:
            self.agent, self.active = agent, True
            self.started.clear()
            self.finished.clear()
            self.before = snapshot(agent)
            self.calls_before = getattr(agent, "session_api_calls", None)
            if self.before is None or type(self.calls_before) is not int:
                self.unknown()

    def settled(self, result):
        calls = getattr(self.agent, "session_api_calls", None)
        if type(calls) is not int or type(self.calls_before) is not int:
            return False
        return all((self.started == self.finished,
                    calls - self.calls_before == len(self.finished),
                    result.get("completed") is True,
                    not any(result.get(flag) for flag in ("failed", "interrupted", "partial"))))

    def result(self, result):
        with self.lock:
            if not self.settled(result):
                self.unknown()
            delta = counter_delta(self.before, snapshot(self.agent))
            if delta is None:
                self.unknown()
            self.active = False
            if self.invalid and delta is not None:
                self.emit("usage", usage=delta)
            return None if self.invalid else delta


def counter_delta(before, after):
    if before is None or after is None:
        return None
    counts = tuple(end - start for start, end in zip(before, after))
    if any(not 0 <= value <= MAX_COUNTER for value in counts):
        return None
    return dict(zip(("input", "output", "cacheRead", "cacheWrite"), counts))


REMOTE_PROCESS_PROBE = r'''
import json, os, subprocess, sys
owned = set(json.loads(sys.argv[1]))
processes = {}
if sys.platform.startswith("linux"):
    for name in os.listdir("/proc"):
        if not name.isdigit():
            continue
        try:
            with open("/proc/" + name + "/stat") as stream:
                fields = stream.read().rsplit(") ", 1)[1].split()
            processes[int(name)] = (int(fields[1]), fields[0])
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
else:
    result = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,stat="],
                            check=True, capture_output=True, text=True, timeout=2)
    for row in result.stdout.splitlines():
        pid, parent, status = row.split()
        processes[int(pid)] = (int(parent), status)
while True:
    expanded = owned | {pid for pid, (parent, _) in processes.items() if parent in owned}
    if expanded == owned:
        break
    owned = expanded
if len(owned) > 4096:
    raise RuntimeError("owned process evidence exceeds its bound")
for pid in owned - processes.keys():
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        continue
    raise RuntimeError("owned process state is unavailable")
live = [pid for pid in owned if pid in processes and processes[pid][1][0].upper() not in "ZX"]
print(json.dumps({"kind": "zeroshot.remote-processes/v1", "pids": sorted(owned), "live": sorted(live)}))
'''


class RemoteKernels:
    """Keep native retired-kernel ownership until checked transport proves settlement."""

    def __init__(self):
        from tools import code_kernel_remote
        self.pending = {}
        self.unknown = False
        self.lock = threading.RLock()
        original_init, original_kill = code_kernel_remote.RemoteKernel.__init__, code_kernel_remote.RemoteKernel.kill
        @wraps(original_init)
        def construct(kernel, *args, **kwargs):
            result = original_init(kernel, *args, **kwargs)
            try:
                self.retain(kernel, [int(kernel.pid)], False)
            except Exception:
                self.unknown = True
            return result
        @wraps(original_kill)
        def kill(kernel):
            return self.kill(original_kill, kernel)
        code_kernel_remote.RemoteKernel.__init__, code_kernel_remote.RemoteKernel.kill = construct, kill

    def probe(self, kernel, pids):
        import shlex
        from tools.code_execution_rpc import _execute_checked
        command = "python3 -c " + shlex.quote(REMOTE_PROCESS_PROBE) + " " + shlex.quote(json.dumps(pids))
        result = _execute_checked(kernel.env, command, "Hermes remote kernel settlement", timeout=3)
        reply = json.loads(result.get("output", ""))
        owned, live = reply.get("pids"), reply.get("live")
        if (reply.get("kind") != "zeroshot.remote-processes/v1" or not valid_pids(owned)
                or not valid_pids(live) or not set(pids) <= set(owned) or not set(live) <= set(owned)):
            raise BridgeError("Hermes remote kernel settlement returned invalid evidence.")
        return owned, live

    def kill(self, original, kernel):
        with self.lock:
            pending = self.pending.get(id(kernel), (kernel, [int(kernel.pid)], False))
        self.retain(kernel, pending[1], True)
        try:
            pids, _ = self.probe(kernel, pending[1])
            self.retain(kernel, pids, True)
        except Exception:
            with self.lock:
                self.unknown = True
        try:
            return original(kernel)
        finally:
            self.refresh(kernel)

    def retain(self, kernel, pids, retired):
        with self.lock:
            others = sum(len(value[1]) for key, value in self.pending.items() if key != id(kernel))
            if others + len(pids) > 4096:
                self.unknown = True
            else:
                self.pending[id(kernel)] = (kernel, pids, retired)

    def refresh(self, kernel):
        with self.lock:
            pending = self.pending.get(id(kernel))
        if pending is None:
            return
        try:
            owned, live = self.probe(kernel, pending[1])
        except Exception:
            return
        with self.lock:
            if live:
                self.retain(kernel, owned, pending[2])
            else:
                self.pending.pop(id(kernel), None)

    def retire_unregistered(self):
        with self.lock:
            active = [kernel for kernel, _, retired in self.pending.values() if not retired]
        for kernel in active:
            try:
                kernel.kill()
            except Exception:
                self.unknown = True

    def confirm(self):
        self.retire_unregistered()
        deadline = time.monotonic() + 2
        while True:
            with self.lock:
                pending = list(self.pending.values())
            for kernel, _, _ in pending:
                self.refresh(kernel)
            with self.lock:
                if not self.unknown and not self.pending:
                    return
                if self.unknown or time.monotonic() >= deadline:
                    raise BridgeError("Hermes remote code kernels did not confirm settlement.")
            time.sleep(0.05)


def valid_pids(values):
    return (isinstance(values, list) and len(values) <= 4096
            and all(type(pid) is int and 0 < pid <= MAX_COUNTER for pid in values))


def launch_policy(frame, config, provider, base_url):
    from dataclasses import replace
    from gateway.session_policy import build_policy
    params = {"source": "oneshot", "cwd": str(Path.cwd()), "cwd_explicit": True,
              "model": frame["model"], "provider": provider}
    if base_url:
        params["base_url"] = base_url
    if frame.get("effort"):
        params["reasoning"] = frame["effort"]
    from utils import env_var_enabled
    params.update({name: env_var_enabled(variable) for name, variable in (
        ("safe_mode", "HERMES_SAFE_MODE"), ("ignore_user_config", "HERMES_IGNORE_USER_CONFIG"),
        ("ignore_rules", "HERMES_IGNORE_RULES"))})
    private = {}
    if params["safe_mode"] or params["ignore_user_config"]:
        from gateway.session_local import _bypass_policy
        policy = _bypass_policy(params, private_secrets=private, client_local=not frame["contained"])
    else:
        policy = build_policy(params, config, private_secrets=private,
                              workspace_posture=True, client_local=not frame["contained"])
    config, terminal = json.loads(policy.config_json), json.loads(policy.terminal_json)
    # Native admission extracts private leaves for its shared owner. Restore only those leaves
    # inside this owned process; the profile and its config/secret resolvers stay native.
    for path, value in private.items():
        target, keys = (terminal, path[1:]) if path[0] is None else (config, path)
        for key in keys[:-1]:
            target = target[key]
        target[keys[-1]] = value
    return replace(policy, config_json=json.dumps(config), terminal_json=json.dumps(terminal))


class Runtime:
    def __init__(self, emit):
        self.emit = emit
        self.agent = self.db = self.usage = self.authority = self.policy = None
        self.stop = threading.Event()
        self.turn_active = threading.Event()
        self.closing = threading.Event()
        self.reader = None
        self.commands = queue.Queue(maxsize=4)
        self.control_failed = False
        self.auth_module = None
        self.remote_kernels = None

    def initialize(self, frame):
        from agent.ssl_verify import install_truststore
        install_truststore()
        sibling("native_environment").install(
            frame.get("nativeEnvironment") or {}, frame.get("declaredEnvironment") or {},
            frame.get("providerEnvironment") or {})
        from hermes_cli.config import load_config_readonly, require_parseable_user_config
        from gateway.session_policy import present_sections, register_worker_hooks
        from gateway.session_worker_construct import checkpoint_agent_kwargs, routing_agent_kwargs
        from hermes_cli.cli_config_load import _load_prefill_messages, _resolve_prefill_messages_file
        from hermes_cli.personality import resolve_ephemeral_system_prompt
        from hermes_constants import get_hermes_home
        from hermes_state import SessionDB
        from utils import env_var_enabled
        require_parseable_user_config(ignore_user_config=env_var_enabled("HERMES_IGNORE_USER_CONFIG"))
        authority = frame.get("authority") or {}
        config = present_sections(load_config_readonly())
        if frame["provider"] == "gateway":
            config.setdefault("model", {})["api_mode"] = authority.get("apiMode")
        self.auth_module = sibling("native_auth")
        self.authority = self.auth_module.install(
            frame["provider"], authority.get("apiKey"), authority.get("baseUrl"),
            config, model=frame["model"], directory=Path(frame["dbPath"]).parent,
            aws=authority.get("aws") or {})
        self.remote_kernels = RemoteKernels()
        kwargs = self.authority.agent_kwargs
        self.policy = launch_policy(frame, self.authority.config, self.authority.provider, kwargs.get("base_url"))
        register_worker_hooks(self.policy)
        from hermes_cli.plugins import get_plugin_manager
        self.usage = Usage(get_plugin_manager(), self.emit)
        sibling("native_memory").install(frame.get("memoryCapability"), self.tool)
        self.db = SessionDB(db_path=Path(frame["dbPath"]))
        session_id = self.db.resolve_resume_session_id(frame["sessionId"])
        from gateway.session_policy import policy_scope
        from agent.managed_worker import discover_profile_mcp
        config = self.policy.config()
        with policy_scope(self.policy), self.authority.auxiliary_scope():
            discover_profile_mcp(self.policy)
            self.agent = self.authority.agent_class(
                model=frame["model"], session_id=session_id, session_db=self.db,
                cwd=self.policy.cwd, enabled_toolsets=list(self.policy.toolsets),
                max_iterations=self.policy.max_turns, run_budget_seconds=self.policy.run_budget_seconds,
                reasoning_config=self.policy.reasoning_config, platform="cli", quiet_mode=True,
                skip_context_files=self.policy.ignore_rules, load_soul_identity=not self.policy.ignore_rules,
                skip_memory=self.policy.ignore_rules, pass_session_id=self.policy.pass_session_id,
                ephemeral_system_prompt=os.getenv("HERMES_EPHEMERAL_SYSTEM_PROMPT") or resolve_ephemeral_system_prompt(config),
                prefill_messages=_load_prefill_messages(_resolve_prefill_messages_file(config), get_hermes_home()),
                stream_delta_callback=lambda text: self.emit("text", text=text) if text else None,
                **kwargs, **checkpoint_agent_kwargs(config),
                **routing_agent_kwargs(config, frame["model"], self.authority.provider, kwargs.get("base_url")))
        from tools.approval_context import set_current_session_key
        set_current_session_key(self.agent.session_id)
        if frame["contained"]:
            from tools.approval_yolo import apply_launch_yolo
            apply_launch_yolo(self.agent.session_id)
        self.usage.agent, self.usage.initialized = self.agent, True
        self.emit("initialized", sessionId=frame["sessionId"])

    def tool(self, name):
        if self.usage is not None and name == "delegate_task":
            self.usage.unknown()
        self.emit("tool", name=name)

    def interrupt(self):
        from agent.interrupt_compat import request_hard_interrupt
        while self.turn_active.is_set() and not self.closing.is_set():
            request_hard_interrupt(self.agent)
            self.closing.wait(0.05)

    def request_stop(self):
        second = self.stop.is_set()
        self.stop.set()
        if second:
            from tools.environments.base import kill_live_foreground_processes
            kill_live_foreground_processes(now=True)
        threading.Thread(target=self.interrupt, daemon=True).start()

    def controls(self, stream):
        try:
            while (frame := read_frame(stream)) is not None:
                if frame.get("type") == "stop":
                    break
                if frame.get("type") != "prompt" or not isinstance(frame.get("text"), str):
                    raise BridgeError("Hermes bridge expected a prompt or stop frame.")
                self.commands.put_nowait(frame["text"])
        except Exception:
            self.control_failed = True
        finally:
            self.stop.set()
            self.interrupt()

    def prompt(self, text):
        from gateway.session_policy import policy_scope
        self.turn_active.set()
        try:
            if self.stop.is_set():
                return
            self.usage.begin(self.agent)
            with policy_scope(self.policy), self.authority.auxiliary_scope(self.agent):
                history = self.db.get_messages_as_conversation(self.agent.session_id)
                result = self.agent.run_conversation(text, conversation_history=history)
                self.wait_for_titles()
            if self.stop.is_set():
                result.update(interrupted=True, completed=False)
            self.emit("result", message=result.get("final_response") or "", usage=self.usage.result(result),
                      failed=bool(result.get("failed")), interrupted=bool(result.get("interrupted")),
                      completed=result.get("completed") is True and not result.get("partial"),
                      **self.failure_error(result))
        finally:
            self.turn_active.clear()

    def failure_error(self, result):
        if not any(result.get(flag) for flag in ("failed", "partial", "interrupted")):
            return {}
        error = result.get("error")
        if error is None:
            return {}
        return {"error": self.authority.sanitize_owned_error(error)[:4096]}

    def wait_for_titles(self):
        from agent import title_generator
        deadline = time.monotonic() + 10
        while any(thread.is_alive() for thread in list(title_generator._UPGRADE_THREADS)):
            if self.stop.is_set() or time.monotonic() >= deadline:
                return
            title_generator.wait_for_title_upgrades(timeout=0.1)

    def settle_delegations(self):
        from tools import async_delegation
        if self.agent is not None:
            async_delegation.refuse_for_session(
                self, session_key=self.agent.session_id, parent_session_ids=(self.agent.session_id,))
        async_delegation.interrupt_all(reason="execution ended")
        deadline = time.monotonic() + 2
        while True:
            with async_delegation._records_lock:
                pending = any(record.get("status") in (*async_delegation._ACTIVE_STATES, "finalizing")
                              for record in async_delegation._records.values())
            if not pending:
                return
            if time.monotonic() >= deadline:
                raise BridgeError("Hermes delegated work did not confirm cancellation.")
            time.sleep(0.05)

    def close(self):
        import psutil
        self.closing.set()
        children = psutil.Process().children(recursive=True)
        failures = []
        clean = partial(cleanup_action, failures)
        if self.agent is not None:
            from agent.background_review import cancel_background_review_for_live_turn
            clean("background-review-cancel", lambda: cancel_background_review_for_live_turn(
                self.agent, message="execution ended", tool_reason="execution ended"))
        from tools.environments.base import kill_live_foreground_processes
        from tools.process_registry import process_registry
        from tools.terminal_tool_lifecycle import cleanup_all_environments
        from tools.mcp_tool_lifecycle import shutdown_mcp_servers
        clean("delegations", self.settle_delegations)
        clean("foreground-tools", kill_live_foreground_processes)
        clean("background-tools", lambda: process_registry.kill_all(source="zeroshot.shutdown", consume_output=True))
        clean("agent", lambda: self.close_agent(clean, failures))
        clean("language-servers", shutdown_native_lsp)
        clean("code-kernels", shutdown_native_kernels)
        if self.remote_kernels is not None:
            clean("remote-code-kernels", self.remote_kernels.confirm)
        clean("terminal-environments", cleanup_all_environments)
        clean("mcp-servers", lambda: shutdown_mcp_servers(timeout=5))
        from agent import title_generator
        from agent.auxiliary_client import shutdown_cached_clients
        clean("auxiliary-clients", shutdown_cached_clients)
        clean("titles", lambda: title_generator.wait_for_title_upgrades(timeout=2))
        clean("foreground-fence", lambda: kill_live_foreground_processes(now=True))
        clean("background-tools", lambda: process_registry.kill_all(source="zeroshot.shutdown", consume_output=True))
        clean("environment-inspection", settle_environment_probe)
        clean("native-helpers", lambda: self.confirm_helpers(process_registry))
        clean("session-store", lambda: self.close_store(clean, failures))
        clean("children", lambda: confirm_children(children, self.sanitize_child))
        if failures:
            raise BridgeError("Hermes native runtime could not confirm cleanup (" + "; ".join(failures) + ").")
        self.emit("closed", clean=True)

    def confirm_helpers(self, registry):
        from agent import title_generator
        if any(thread.is_alive() for thread in list(title_generator._UPGRADE_THREADS)):
            raise BridgeError("Hermes title work did not confirm cancellation.")
        if any(process["status"] == "running" for process in registry.list_sessions()):
            raise BridgeError("Hermes tool work did not confirm cancellation.")
        if self.reader is not None:
            self.reader.join(timeout=0.1)
            if self.reader.is_alive() and self.control_failed:
                raise BridgeError("Hermes control transport did not settle.")

    def close_agent(self, clean, failures):
        if self.agent is None:
            return
        from tools.browser_tool_lifecycle import cleanup_browser
        for owner in getattr(self.agent, "_process_owner_task_ids", ()):
            clean("browser-session", lambda owner=owner: cleanup_browser(owner))
        clean("agent-close", self.agent.close)
        pending = getattr(self.agent, "_background_review_run", None)
        if pending is not None and not pending.request_done.is_set():
            failures.append("background-review-pending")

    def close_store(self, clean, failures):
        if self.db is not None:
            if not self.db.flush_token_counts():
                failures.append("usage-writer")
            clean("database-close", self.db.close)

    def sanitize_child(self, name):
        if self.authority is None:
            return "unavailable"
        return self.authority.sanitize_owned_error(name)

    def diagnostic(self, error):
        if isinstance(error, BridgeError):
            return str(error)
        if self.auth_module is not None and isinstance(error, self.auth_module.AuthError):
            return str(error)
        return f"Hermes native runtime failed ({type(error).__name__})."


def cleanup_action(failures, stage, action):
    try:
        action()
    except BridgeError as error:
        failures.append(stage + ": " + str(error))
    except Exception:
        failures.append(stage)


def _live_child_detail(child, sanitize):
    import psutil
    try:
        status = child.status()
    except psutil.NoSuchProcess:
        return None
    except psutil.AccessDenied:
        status = "unavailable"
    if status in (psutil.STATUS_ZOMBIE, psutil.STATUS_DEAD):
        try:
            child.wait(timeout=0)
        except (psutil.Error, ChildProcessError):
            pass
        return None
    try:
        name = child.name()
    except psutil.NoSuchProcess:
        return None
    except psutil.AccessDenied:
        name = "unavailable"
    return f"PID {child.pid}, comm={sanitize(name)[:64]!r}, status={status!r}"


def confirm_children(children, sanitize):
    import psutil
    remaining = psutil.wait_procs(children, timeout=1)[1]
    remaining.extend(psutil.Process().children(recursive=True))
    by_pid = {child.pid: child for child in remaining}
    details = [detail for pid in sorted(by_pid)
               if (detail := _live_child_detail(by_pid[pid], sanitize)) is not None]
    if details:
        sample = "; ".join(details[:16])
        raise BridgeError(f"{len(details)} live native child process(es) remain ({sample}).")


def shutdown_native_kernels():
    # Persistent execute_code kernels belong to this process and normally close at atexit.
    # End them before acknowledging cleanup, while remote terminal transports still exist.
    for name, shutdown in (("tools.code_kernel", "shutdown_all_kernels"),
                           ("tools.code_kernel_remote", "shutdown_all_remote_kernels")):
        native = sys.modules.get(name)
        if native is not None:
            getattr(native, shutdown)()


def shutdown_native_lsp():
    service = sys.modules.get("agent.lsp")
    if service is not None:
        service.shutdown_service()


def settle_environment_probe():
    # Hermes eagerly warms its native pip/Python capability probe in a daemon thread.
    # Its children share our process group; let its bounded native probe settle first.
    probe = sys.modules.get("tools.env_probe")
    thread = getattr(probe, "_PROBE_THREAD", None)
    if thread is not None:
        thread.join(timeout=probe._PROBE_WAIT_TIMEOUT)
        if thread.is_alive():
            raise BridgeError("Hermes environment inspection did not settle.")


def main():
    output = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1, encoding="utf-8")
    os.dup2(sys.stderr.fileno(), sys.stdout.fileno())
    lock = threading.Lock()
    def emit(kind, **fields):
        with lock:
            output.write(json.dumps({"type": kind, **fields}, ensure_ascii=False) + "\n")
    runtime = Runtime(emit)
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda _sig, _frame: runtime.request_stop())
    emit("ready", protocol=1)
    status = 0
    home_token = None
    try:
        from hermes_constants import get_hermes_home, set_hermes_home_override, reset_hermes_home_override
        # Native prompt/skills loaders otherwise infer the profile from our private session DB.
        home_token = set_hermes_home_override(get_hermes_home())
        frame = read_frame(sys.stdin.buffer)
        if frame is None:
            raise BridgeError("Hermes bridge initialization was missing.")
        runtime.initialize(frame)
        runtime.reader = threading.Thread(target=runtime.controls, args=(sys.stdin.buffer,), daemon=True)
        runtime.reader.start()
        while not runtime.stop.is_set():
            try:
                text = runtime.commands.get(timeout=0.1)
            except queue.Empty:
                continue
            runtime.prompt(text)
        if runtime.control_failed:
            raise BridgeError("Hermes bridge control transport failed.")
    except Exception as error:
        status = 1
        if runtime.usage is not None:
            runtime.usage.unknown()
        emit("error", message=runtime.diagnostic(error))
    finally:
        try:
            runtime.close()
        except Exception as error:
            status = 1
            emit("error", message=runtime.diagnostic(error))
        if home_token is not None:
            reset_hermes_home_override(home_token)
        output.close()
    return status
