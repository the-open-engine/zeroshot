"""Offline contract tests: no Hermes installation, credentials, or model calls required."""
import copy
import importlib.util
import json
from pathlib import Path
import socket
import struct
import sys
import tempfile
import threading
import time
from types import ModuleType, SimpleNamespace
import unittest
from unittest.mock import patch


def receive(peer, length):
    result = bytearray()
    while len(result) < length:
        part = peer.recv(length - len(result))
        if not part:
            raise EOFError()
        result.extend(part)
    return bytes(result)


class Broker:
    def __init__(self, directory, response=b"RECALLED_MEMORY", calls=1, drip=False):
        self.response, self.calls, self.drip = response, calls, drip
        self.requests, self.acks, self.errors = [], [], []
        self.server = socket.socket()
        self.server.bind(("127.0.0.1", 0))
        self.server.listen()
        self.server.settimeout(1)
        self.path = Path(directory) / "capability.json"
        self.path.write_text(json.dumps({"endpoint": "127.0.0.1:" + str(self.server.getsockname()[1]),
                                         "token": "synthetic-private-capability"}))
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        try:
            for _ in range(self.calls):
                with self.server.accept()[0] as peer:
                    peer.settimeout(1)
                    length = struct.unpack("!I", receive(peer, 4))[0]
                    self.requests.append(json.loads(receive(peer, length)))
                    self.reply(peer)
        except OSError as error:
            if not self.drip:
                self.errors.append(error)
        finally:
            self.server.close()

    def reply(self, peer):
        frame = struct.pack("!I", len(self.response)) + self.response
        if self.drip:
            for byte in frame:
                time.sleep(0.06)
                peer.sendall(bytes([byte]))
        else:
            peer.sendall(frame)
        try:
            self.acks.append(peer.recv(1))
        except ConnectionResetError:
            self.acks.append(b"")

    def finish(self):
        self.thread.join(timeout=2)
        assert not self.thread.is_alive(), "fixture broker did not stop"
        assert not self.errors, self.errors


class NativeMemoryTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.receipts, self.raw, self.flushed, self.attachments = [], [], [], []
        self.notes = ModuleType("agent.plugin_live_notes")
        self.native = ModuleType("agent.tool_executor")
        self.notes.queue_plugin_notice = self.queue
        self.notes.attach_to_tool_message = self.attach
        def native_commit(agent, messages, ref, result, **kwargs):
            return self.commit(agent, messages, ref, result)
        self.native._commit_tool_result = native_commit
        package = ModuleType("agent")
        package.plugin_live_notes, package.tool_executor = self.notes, self.native
        modules = {"agent": package, "agent.plugin_live_notes": self.notes,
                   "agent.tool_executor": self.native}
        self.module_patch = patch.dict(sys.modules, modules)
        self.module_patch.start()
        self.addCleanup(self.module_patch.stop)
        path = Path(__file__).resolve().parents[1] / "native_memory.py"
        spec = importlib.util.spec_from_file_location("hermes_memory_test_subject", path)
        self.memory = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.memory)
        self.agent = SimpleNamespace(session_id="native-session", notices={})

    @staticmethod
    def queue(agent, key, text):
        agent.notices[key] = text

    def attach(self, agent, message):
        self.attachments.append(agent.session_id)
        text = "\n\n".join(agent.notices.values())
        agent.notices.clear()
        if text:
            content = message["content"]
            message["content"] = ([*content, {"type": "text", "text": text}]
                                  if isinstance(content, list) else content + "\n\n" + text)
        return "native-return"

    def commit(self, agent, messages, ref, result):
        self.raw.append(copy.deepcopy(result))
        content = result["content"] if isinstance(result, dict) and result.get("_multimodal") else result
        if isinstance(content, str) and len(content) > 1000:
            content = "[native spilled preview]"
        message = {"tool_call_id": ref.call_id, "content": content}
        self.notes.attach_to_tool_message(agent, message)
        messages.append(message)
        self.flushed.append(copy.deepcopy(message))
        return "native-commit-return"

    def call(self, result="native result", name="read_file", agent=None, **kwargs):
        ref = SimpleNamespace(name=name, args={"path": "src/private.rs"}, call_id="tool-1")
        messages = []
        value = self.native._commit_tool_result(agent or self.agent, messages, ref, result, **kwargs)
        self.assertEqual(value, "native-commit-return")
        return messages[-1]

    def install(self, broker=None):
        self.memory.install(str(broker.path) if broker else None, self.receipts.append)

    def test_native_json_and_notices_survive_before_first_flush(self):
        broker = Broker(self.directory.name)
        self.install(broker)
        self.queue(self.agent, "user-plugin", "USER_NATIVE_NOTICE")
        raw = json.dumps({"success": True, "path": "src/private.rs"})
        message = self.call(raw)
        broker.finish()
        self.assertEqual(self.raw, [raw])
        self.assertIn("USER_NATIVE_NOTICE", message["content"])
        self.assertIn("RECALLED_MEMORY", self.flushed[0]["content"])
        self.assertEqual(broker.acks, [b"\x01"])
        self.assertEqual(broker.requests[0]["event"]["input"], {"path": "src/private.rs"})
        self.assertEqual(self.receipts, ["read_file"])
        self.assertEqual(self.attachments, ["native-session"])

    def test_multimodal_image_and_large_native_spill_survive(self):
        broker = Broker(self.directory.name, calls=2)
        self.install(broker)
        image = {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
        raw = {"_multimodal": True, "content": [image], "metadata": {"native": True}}
        message = self.call(raw)
        self.assertEqual(message["content"][0], image)
        self.assertIn("RECALLED_MEMORY", message["content"][-1]["text"])
        spilled = self.call("x" * 2000)
        self.assertIn("[native spilled preview]", spilled["content"])
        self.assertIn("RECALLED_MEMORY", spilled["content"])
        broker.finish()
        self.assertEqual(self.raw[0], raw)
        self.assertEqual(broker.acks, [b"\x01", b"\x01"])

    def test_unknown_native_shape_stays_unmodified_without_ack(self):
        broker = Broker(self.directory.name)
        self.install(broker)
        raw = {"unknown_native_shape": True}
        self.assertEqual(self.call(raw)["content"], raw)
        broker.finish()
        self.assertEqual(broker.acks, [b""])

    def test_delegated_agent_uses_same_carrier_and_own_native_session(self):
        broker = Broker(self.directory.name, calls=2)
        self.install(broker)
        delegated = SimpleNamespace(session_id="delegated-native-session", notices={})
        self.call()
        thread = threading.Thread(target=lambda: self.call(agent=delegated))
        thread.start()
        thread.join(timeout=1)
        self.assertFalse(thread.is_alive())
        broker.finish()
        self.assertEqual([request["event"]["sessionId"] for request in broker.requests],
                         ["native-session", "delegated-native-session"])
        self.assertEqual(broker.acks, [b"\x01", b"\x01"])

    def test_receipts_work_without_memory_are_bounded_and_install_is_idempotent(self):
        self.install()
        self.install()
        self.call(name="terminal\n" + "x" * 500)
        self.assertEqual(self.receipts, ["terminal" + "x" * 120])
        self.assertNotIn("src/private.rs", str(self.receipts))
        self.assertEqual(self.attachments, ["native-session"])

    def test_blocked_call_has_no_receipt_or_recall(self):
        self.install()
        self.call(blocked=True)
        self.assertEqual(self.receipts, [])
        self.assertEqual(self.attachments, ["native-session"])

    def test_native_exception_propagates_without_ack_and_resets_scope(self):
        broker = Broker(self.directory.name)
        def fail(*args):
            raise RuntimeError("native failure")
        self.notes.attach_to_tool_message = fail
        self.install(broker)
        with self.assertRaisesRegex(RuntimeError, "native failure"):
            self.call()
        broker.finish()
        self.assertEqual(broker.acks, [b""])
        self.assertIsNone(self.memory._scope.get())

    def test_empty_oversized_invalid_utf8_and_nul_responses_fail_open(self):
        for response in [b"", b"x" * 32769, b"\xff", b"invalid\x00context"]:
            with self.subTest(response_length=len(response)):
                broker = Broker(self.directory.name, response=response)
                self.install(broker)
                self.assertEqual(self.call()["content"], "native result")
                broker.finish()
                self.assertEqual(broker.acks, [b""])

    def test_partial_frame_has_one_absolute_callback_deadline(self):
        broker = Broker(self.directory.name, drip=True)
        self.install(broker)
        started = time.monotonic()
        self.assertEqual(self.call()["content"], "native result")
        self.assertLess(time.monotonic() - started, 0.65)
        broker.finish()

    def test_oversized_request_and_nonlocal_endpoint_never_connect(self):
        self.install()
        path = Path(self.directory.name) / "nonlocal.json"
        path.write_text(json.dumps({"endpoint": "example.com:1234", "token": "synthetic"}))
        self.memory.install(str(path), self.receipts.append)
        with patch.object(socket.socket, "connect", side_effect=AssertionError("must not connect")):
            self.assertEqual(self.call()["content"], "native result")
        path.write_text(json.dumps({"endpoint": "127.0.0.1:1234", "token": "synthetic"}))
        ref = SimpleNamespace(name="read_file", args={"input": "x" * 262144})
        with patch.object(socket.socket, "connect", side_effect=AssertionError("must not connect")):
            self.assertIsNone(self.memory._retrieve(self.agent, ref))


if __name__ == "__main__":
    unittest.main()
