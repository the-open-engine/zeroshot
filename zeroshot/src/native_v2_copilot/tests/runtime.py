#!/usr/bin/python3
"""Deterministic Copilot RPC peer; never contacts a provider."""
import json
import os
import sys
import time

mode = os.environ.get("TEST_MODE", "success")
turn = 0
session = ""

def send(message):
    body = json.dumps({"jsonrpc": "2.0", **message}).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()

def event(kind, data):
    send({"method": "session.event", "params": {
        "sessionId": session, "event": {"type": kind, "data": data}}})

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        if line == b"\r\n":
            break
        name, value = line.decode().split(":", 1)
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))

while (message := read()) is not None:
    method = message.get("method")
    params = message.get("params", {})
    if "CAPTURE_PATH" in os.environ:
        with open(os.environ["CAPTURE_PATH"], "a", encoding="utf-8") as capture:
            capture.write(json.dumps(message) + "\n")
    if method == "connect":
        result = {"protocolVersion": 99 if mode == "version" else 3}
    elif method in ("session.create", "session.resume"):
        assert "COPILOT_GITHUB_TOKEN" not in os.environ
        assert "COPILOT_GITHUB_TOKEN_EXPIRES_AT" not in os.environ
        assert "GH_TOKEN" not in os.environ
        session = params["sessionId"]
        assert params["model"] == "auto"
        assert params["requestPermission"] is True
        registration = params.get("gitHubTokenProviderRegistrationId")
        if params.get("gitHubTokenProviderRegistrationId"):
            send({"id": "auth-1", "method": "gitHubToken.getToken", "params": {
                "registrationId": params["gitHubTokenProviderRegistrationId"],
                "host": "https://github.com", "sessionId": session, "reason": "initial"}})
            auth = read()
            assert auth["result"]["kind"] == "token"
            assert auth["result"]["expiresIn"] > 3600
        else:
            assert params["gitHubToken"] == "gho_fake-secret"
        result = {"sessionId": "wrong" if mode == "identity" else session}
    elif method == "session.send":
        turn += 1
        if mode == "refresh":
            send({"id": "auth-refresh", "method": "gitHubToken.getToken", "params": {
                "registrationId": registration, "host": "https://github.com",
                "sessionId": session, "reason": "refresh"}})
            auth = read()
            assert auth["result"]["accessToken"] == "rotated-sensitive-value"
            assert auth["result"]["expiresIn"] > 3600
        assert params["responseFormat"]["jsonSchema"]["strict"] is True
        assert params["sessionId"] == session
        event("future.event", {"invalid": [1, 2, 3]})
        event("assistant.usage", {"inputTokens": 7, "outputTokens": 3})
        if mode == "cancel":
            event("tool.execution_start", {"toolName": "ready"})
            time.sleep(60)
        if mode == "error":
            event("session.error", {"message": "rejected gho_fake-secret"})
        if mode == "permission":
            event("permission.requested", {"requestId": "allow-shell", "permissionRequest": {
                "kind": "shell", "commands": [{"identifier": "echo", "readOnly": True}],
                "hasWriteFileRedirection": False}})
            decision = read()
            assert decision["params"]["result"]["kind"] == "approve-once"
            send({"id": decision["id"], "result": {"success": True}})
        if mode == "duplex":
            for _ in range(80):
                event("future.event", {"padding": "x" * 1048576})
        answer = "invalid" if mode == "malformed" or (mode == "correction" and turn == 1) else 42
        event("assistant.message", {"content": json.dumps({"response": {"answer": answer}})})
        result = {"messageId": "message"}
    elif method == "session.detach":
        result = {"success": True}
    else:
        raise AssertionError(method)
    send({"id": message["id"], "result": result})
