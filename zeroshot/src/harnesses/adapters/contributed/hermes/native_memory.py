"""Attach host recall after native tool bookkeeping, using Hermes's retained notices."""
from contextvars import ContextVar
from functools import wraps
import json
import os
import socket
import stat
import struct
import time

_scope = ContextVar("zeroshot_hermes_tool", default=None)
_installed = False
_capability_path = None
_emit_tool = None


def _remaining(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("Active Memory callback deadline elapsed")
    return remaining


def _receive(peer, length, deadline):
    parts = bytearray()
    while len(parts) < length:
        peer.settimeout(_remaining(deadline))
        part = peer.recv(length - len(parts))
        if not part:
            raise EOFError("Active Memory callback closed")
        parts.extend(part)
    return bytes(parts)


def _capability(path):
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags), "rb") as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError("Active Memory capability must be a file")
        body = source.read(16385)
    if len(body) > 16384:
        raise ValueError("Active Memory capability too large")
    capability = json.loads(body)
    if set(capability) != {"endpoint", "token"} or not isinstance(capability["token"], str):
        raise ValueError("Invalid Active Memory capability")
    host, separator, port = capability["endpoint"].rpartition(":")
    if host != "127.0.0.1" or separator != ":" or not port.isascii() or not port.isdigit():
        raise ValueError("Active Memory callback must be local")
    if not 1 <= int(port) <= 65535:
        raise ValueError("Invalid Active Memory callback port")
    return capability["token"], (host, int(port))


def _retrieve(agent, ref):
    peer = None
    try:
        deadline = time.monotonic() + 0.25
        token, endpoint = _capability(_capability_path)
        event = {"toolName": ref.name, "input": ref.args, "cwd": os.getcwd(),
                 "sessionId": getattr(agent, "session_id", "")}
        body = json.dumps({"token": token, "event": event}, ensure_ascii=False,
                          allow_nan=False, separators=(",", ":")).encode("utf-8")
        if len(body) > 262144:
            return None
        peer = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        peer.settimeout(_remaining(deadline))
        peer.connect(endpoint)
        peer.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        peer.settimeout(_remaining(deadline))
        peer.sendall(struct.pack("!I", len(body)) + body)
        length = struct.unpack("!I", _receive(peer, 4, deadline))[0]
        if length > 32768:
            return None
        context = _receive(peer, length, deadline).decode("utf-8")
        if not context or "\x00" in context:
            return None
        delivery = (peer, context, deadline)
        peer = None
        return delivery
    except Exception:
        return None
    finally:
        if peer is not None:
            peer.close()


def _contains_context(content, context):
    if isinstance(content, str):
        return context in content
    if isinstance(content, list):
        return any(isinstance(part, dict) and isinstance(part.get("text"), str)
                   and context in part["text"] for part in content)
    return False


def _attach(notes, original, agent, message):
    selected = _scope.get()
    delivery = _retrieve(agent, selected[1]) if (
        _capability_path and selected is not None and selected[0] is agent
    ) else None
    if delivery is None:
        return original(agent, message)
    peer, context, deadline = delivery
    try:
        if isinstance(message.get("content"), (str, list)):
            notes.queue_plugin_notice(agent, "zeroshot.active-memory", context)
        result = original(agent, message)
        if _contains_context(message.get("content"), context):
            try:
                peer.settimeout(_remaining(deadline))
                peer.sendall(b"\x01")
            except OSError:
                pass
        return result
    finally:
        peer.close()


def _tool_receipt(name):
    if not isinstance(name, str) or not name or _emit_tool is None:
        return
    bounded = "".join(character for character in name if character.isprintable())[:128]
    if bounded:
        try:
            _emit_tool(bounded)
        except Exception:
            pass


def install(capability_path, emit_tool):
    """Install once for all native agents, including delegates; never touch native config."""
    global _installed, _capability_path, _emit_tool
    _capability_path, _emit_tool = capability_path, emit_tool
    if _installed:
        return
    from agent import plugin_live_notes as notes
    from agent import tool_executor as native
    commit, attach = native._commit_tool_result, notes.attach_to_tool_message

    @wraps(commit)
    def scoped_commit(agent, messages, ref, result, **kwargs):
        blocked = kwargs.get("blocked", False)
        if not blocked:
            _tool_receipt(ref.name)
        token = _scope.set(None if blocked else (agent, ref))
        try:
            return commit(agent, messages, ref, result, **kwargs)
        finally:
            _scope.reset(token)

    @wraps(attach)
    def attach_recall(agent, message):
        return _attach(notes, attach, agent, message)

    native._commit_tool_result = scoped_commit
    notes.attach_to_tool_message = attach_recall
    _installed = True
