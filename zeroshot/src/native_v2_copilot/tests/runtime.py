#!/usr/bin/python3
"""Deterministic Copilot RPC peer; never contacts a provider."""
import json
import os
import sys
import time

mode = os.environ.get("TEST_MODE", "success")
expected_model = "gateway/fixture" if mode.startswith("registry") else "auto"
expected_session_model = "gpt-5.6-sol" if mode == "registry_command" else expected_model
turn = 0
session = ""

assert "--model" in sys.argv
assert sys.argv[sys.argv.index("--model") + 1] == expected_model
secret_environment = next(
    argument for argument in sys.argv if argument.startswith("--secret-env-vars=")
)
assert "DBUS_SESSION_BUS_ADDRESS" in secret_environment
assert "XDG_RUNTIME_DIR" in secret_environment
assert "COPILOT_PROVIDER_BASE_URL" in secret_environment
assert "COPILOT_PROVIDER_API_KEY_COMMAND" in secret_environment
assert "HOST_ONLY_SECRET" not in os.environ
if mode not in ("provider_command", "registry_command"):
    assert "COMMAND_DEPENDENCY" not in os.environ

github_host = os.environ.get("COPILOT_GH_HOST") or os.environ.get("GH_HOST") or "github.com"
if "://" not in github_host:
    github_host = "https://" + github_host
github_host = github_host.rstrip("/")

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
        assert params["model"] == expected_session_model
        assert params["requestPermission"] is True
        registration = params.get("gitHubTokenProviderRegistrationId")
        if params.get("gitHubTokenProviderRegistrationId"):
            send({"id": "auth-1", "method": "gitHubToken.getToken", "params": {
                "registrationId": params["gitHubTokenProviderRegistrationId"],
                "host": github_host, "sessionId": session, "reason": "initial"}})
            auth = read()
            assert auth["result"]["kind"] == "token"
            assert auth["result"]["expiresIn"] > 3600
        elif mode == "provider_command":
            assert "gitHubToken" not in params
            assert "gitHubTokenProviderRegistrationId" not in params
            assert params["provider"]["baseUrl"] == "https://gateway.example/v1"
            assert params["provider"]["apiKeyCommand"] == "provider-key-helper --fresh"
            assert params["provider"]["wireModel"] == "gateway-model"
            assert "apiKey" not in params["provider"]
            assert os.environ["COMMAND_DEPENDENCY"] == "private-command-context"
            assert "COMMAND_DEPENDENCY" in secret_environment
            for name in (
                "COPILOT_PROVIDER_BASE_URL",
                "COPILOT_PROVIDER_API_KEY",
                "COPILOT_PROVIDER_API_KEY_COMMAND",
                "COPILOT_PROVIDER_WIRE_MODEL",
            ):
                assert name not in os.environ
        elif mode == "provider":
            assert "gitHubToken" not in params
            assert "gitHubTokenProviderRegistrationId" not in params
            for name in (
                "COPILOT_PROVIDER_BASE_URL",
                "COPILOT_PROVIDER_TYPE",
                "COPILOT_PROVIDER_API_KEY",
                "COPILOT_PROVIDER_WIRE_API",
                "COPILOT_PROVIDER_WIRE_MODEL",
                "COPILOT_PROVIDER_MAX_OUTPUT_TOKENS",
                "COPILOT_PROVIDER_HEADERS",
            ):
                assert name not in os.environ
            assert params["provider"] == {
                "baseUrl": "https://gateway.example/v1",
                "type": "openai",
                "apiKey": "provider-sensitive-value",
                "wireApi": "responses",
                "modelId": "auto",
                "wireModel": "gateway-model",
                "maxOutputTokens": 4096,
                "headers": {"X-Tenant": "alpha", "X-Route": "beta"},
            }
        elif mode == "provider_overlay":
            assert "gitHubToken" not in params
            assert params["provider"]["baseUrl"] == "https://declared.example/v1"
            assert params["provider"]["apiKey"] == "declared-provider-key"
            assert params["provider"]["wireModel"] == "declared-wire-model"
            assert params["provider"]["modelId"] == "ambient-capability-model"
            assert "apiKeyCommand" not in params["provider"]
            assert "bearerToken" not in params["provider"]
            assert "headers" not in params["provider"]
            for name in (
                "COPILOT_PROVIDER_BASE_URL",
                "COPILOT_PROVIDER_API_KEY",
                "COPILOT_PROVIDER_MODEL_ID",
                "COPILOT_PROVIDER_WIRE_MODEL",
            ):
                assert name not in os.environ
        elif mode == "settings":
            assert os.environ["COPILOT_OFFLINE"] == "true"
            assert os.environ["COPILOT_PROVIDER_BASE_URL"] == "https://gateway.example/v1"
            assert params["provider"]["apiKey"] == "provider-sensitive-value"
            assert "gitHubToken" not in params
        elif mode == "registry":
            assert "provider" not in params
            assert params["providers"][0]["name"] == "gateway"
            assert params["providers"][0]["apiKey"] == "registry-sensitive-value"
            assert params["models"][0]["provider"] == "gateway"
            assert "COPILOT_PROVIDERS_CONFIG" not in os.environ
            assert "COPILOT_PROVIDER_BASE_URL" not in os.environ
        elif mode == "registry_command":
            assert "providers" not in params
            assert "models" not in params
            assert params["provider"]["baseUrl"] == "https://registry.example/v1"
            assert params["provider"]["apiKeyCommand"] == "registry-key-helper --fresh"
            assert params["provider"]["modelId"] == "gpt-5.6-sol"
            assert params["provider"]["wireModel"] == "wire-registry-model"
            assert os.environ["COMMAND_DEPENDENCY"] == "private-command-context"
            assert "COMMAND_DEPENDENCY" in secret_environment
            assert "COPILOT_PROVIDERS_CONFIG" not in os.environ
        elif mode == "declared_token":
            assert params["gitHubToken"] == "declared-copilot-token"
            assert "provider" not in params
            assert "providers" not in params
            assert "models" not in params
            assert "COPILOT_PROVIDERS_CONFIG" not in os.environ
            assert "COPILOT_OFFLINE" not in os.environ
        elif mode == "native":
            assert "gitHubToken" not in params
            assert os.environ["NATIVE_CONTEXT"] == "preserved"
            assert os.environ["HOME"].endswith("/user-home")
            assert os.environ["COPILOT_HOME"].endswith("/copilot-home")
            assert "COPILOT_DISABLE_KEYTAR" not in os.environ
        else:
            assert params["gitHubToken"] == "gho_fake-secret"
        result = {"sessionId": "wrong" if mode == "identity" else session}
    elif method == "session.send":
        turn += 1
        if mode == "refresh":
            send({"id": "auth-refresh", "method": "gitHubToken.getToken", "params": {
                "registrationId": registration, "host": github_host,
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
