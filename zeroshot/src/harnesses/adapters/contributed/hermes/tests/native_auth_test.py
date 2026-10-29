"""Offline SDK integration: HERMES_SOURCE=/path/to/hermes python native_auth_test.py."""
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import unittest

AUTH = Path(__file__).resolve().parents[1] / "native_auth.py"
ENDPOINTS = {"openai": "https://api.openai.com/v1", "anthropic": "https://api.anthropic.com", "openrouter": "https://openrouter.ai/api/v1"}
KEYS = {"openai": "sk-proj-declared-synthetic", "anthropic": "sk-ant-api03-declared-synthetic", "openrouter": "sk-or-v1-declared-synthetic"}
ENV_KEYS = {"openai": "OPENAI_API_KEY", "anthropic": "ANTHROPIC_API_KEY", "openrouter": "OPENROUTER_API_KEY"}


def response(httpx, provider, body):
    if provider == "responses":
        return httpx.Response(200, json={"id": "resp_fixture", "object": "response", "created_at": 1,
            "status": "completed", "model": body["model"], "output": [{"type": "message", "id": "msg_fixture",
            "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}})
    if provider == "anthropic":
        message = {"id": "msg_fixture", "type": "message", "role": "assistant", "model": body["model"],
                   "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn", "stop_sequence": None,
                   "usage": {"input_tokens": 1, "output_tokens": 1}}
        if body.get("stream"):
            events = [("message_start", {"type": "message_start", "message": {**message, "content": [], "stop_reason": None}}),
                      ("content_block_start", {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                      ("content_block_delta", {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "ok"}}),
                      ("content_block_stop", {"type": "content_block_stop", "index": 0}),
                      ("message_delta", {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}}),
                      ("message_stop", {"type": "message_stop"})]
            stream = "".join(f"event: {kind}\ndata: {json.dumps(value)}\n\n" for kind, value in events)
            return httpx.Response(200, content=stream.encode(), headers={"content-type": "text/event-stream", "set-cookie": "__cf_bm=synthetic-http-context; Path=/; Secure; HttpOnly"})
    else:
        message = {"id": "chatcmpl_fixture", "object": "chat.completion", "created": 1, "model": body["model"],
                   "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}
    return httpx.Response(200, json=message, headers={"set-cookie": "__cf_bm=synthetic-http-context; Path=/; Secure; HttpOnly"})


def sdk_fixtures(handle):
    import anthropic
    import httpx
    import openai
    native_openai, native_async, native_anthropic = openai.OpenAI, openai.AsyncOpenAI, anthropic.Anthropic

    class OpenAI(native_openai):
        def __init__(self, *args, **kwargs):
            kwargs["http_client"] = httpx.Client(transport=httpx.MockTransport(handle))
            super().__init__(*args, **kwargs)

    class AsyncOpenAI(native_async):
        def __init__(self, *args, **kwargs):
            kwargs["http_client"] = httpx.AsyncClient(transport=httpx.MockTransport(handle))
            super().__init__(*args, **kwargs)

    class Anthropic(native_anthropic):
        def __init__(self, *args, **kwargs):
            kwargs["http_client"] = httpx.Client(transport=httpx.MockTransport(handle))
            super().__init__(*args, **kwargs)

    openai.OpenAI, openai.AsyncOpenAI, anthropic.Anthropic = OpenAI, AsyncOpenAI, Anthropic


def settings(home, provider):
    native = "openai-api" if provider == "openai" else provider
    (home / ".env").write_text(f"{ENV_KEYS[provider]}=native-synthetic-key\n")
    config = {"model": {"provider": native, "default": "native-model", "default_headers": {"X-Native-Metadata": "kept", "Authorization": "Bearer wrong-stored-key"}},
              "memory": {"enabled": False, "memory_enabled": False, "user_profile_enabled": False},
              "auxiliary": {"transient_retries": 0, "compression": {"provider": provider, "model": "native-aux-model", "timeout": 321,
                              "fallback_chain": [{"provider": provider, "model": "native-fallback-model", "timeout": 17, "extra_body": {"native_fallback": True}}]}},
              "delegation": {"provider": provider, "model": "native-child-model", "reasoning_effort": "low"},
              "plugins": {"enabled": ["auth-proof"]}}
    (home / "config.yaml").write_text(json.dumps(config))
    for name in ("auth-proof", "disabled-proof"):
        plugin = home / "plugins" / name
        plugin.mkdir(parents=True)
        (plugin / "plugin.yaml").write_text(f"name: {name}\nversion: 0.1.0\n")
        (plugin / "__init__.py").write_text('''from pathlib import Path
import run_agent
def register(ctx):
    assert run_agent.AIAgent.__name__ == "OwnedAIAgent"
    with Path(__file__).with_name("registered").open("a") as output: output.write("once\\n")
    ctx.register_auxiliary_task("proof_aux", display_name="Native proof", description="Native plugin helper", defaults={"model":"native-plugin-model","timeout":43})
''')
    return native


def worker_config(home, provider, key_only):
    settings(home, provider)
    config = json.loads((home / "config.yaml").read_text())
    config["model"]["api_mode"] = "anthropic_messages" if provider == "anthropic" else "chat_completions"
    if key_only:
        suffix = "/anthropic" if provider == "anthropic" else "/v1"
        config["auxiliary"]["compression"]["base_url"] = "https://native-aux.example" + suffix
        config["auxiliary"]["compression"]["fallback_chain"][0]["base_url"] = "https://native-fallback.example" + suffix
        config["delegation"]["base_url"] = "https://native-child.example" + suffix
        config["delegation"]["api_mode"] = config["model"]["api_mode"]
    (home / "config.yaml").write_text(json.dumps(config))

def worker(provider, key_only=False):
    if os.environ.get("HERMES_SOURCE"):
        sys.path.insert(0, os.environ["HERMES_SOURCE"])
    with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-auth-") as directory:
        home = Path(directory)
        os.environ.update(HOME=directory, HERMES_HOME=directory)
        for name in list(ENV_KEYS.values()) + ["ANTHROPIC_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_AUTH_TOKEN"]:
            os.environ.pop(name, None)
        worker_config(home, provider, key_only)
        original_connect = socket.socket.connect
        def no_network(sock, address):
            if isinstance(address, tuple):
                raise AssertionError("offline native auth test attempted network")
            return original_connect(sock, address)
        socket.socket.connect = no_network
        import httpx
        receipts, hosts, cookies = [], [], []
        def handle(request):
            body = json.loads(request.content)
            header = "x-api-key" if provider == "anthropic" else "authorization"
            expected = KEYS[provider] if provider == "anthropic" else "Bearer " + KEYS[provider]
            assert request.headers.get(header) == expected
            allowed = {httpx.URL(ENDPOINTS[provider]).host}
            if key_only:
                allowed.update({"native-aux.example", "native-fallback.example", "native-child.example"})
            assert request.url.host in allowed
            hosts.append(request.url.host)
            cookies.append("cookie" in request.headers)
            receipts.append(body["model"])
            if body["model"] == "native-failing-model":
                return httpx.Response(400, json={"error": {"message": "model is not supported for this account"}})
            return response(httpx, provider, body)
        sdk_fixtures(handle)
        from hermes_cli.config import load_config_readonly
        spec = importlib.util.spec_from_file_location("native_auth", AUTH)
        auth = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(auth)
        address = httpx.URL(ENDPOINTS[provider])
        endpoint = f"https://{address.host.upper()}:443{address.path}"
        authority = auth.install(provider, KEYS[provider], None if key_only else endpoint, load_config_readonly(), model="native-model")
        from agent import auxiliary_client as aux
        from tools import delegate_tool as delegate
        messages = [{"role": "user", "content": "offline fixture"}]
        with authority.auxiliary_scope():
            assert aux.call_llm(task="compression", messages=messages, max_tokens=16).choices[0].message.content == "ok"
            assert aux.call_llm(task="proof_aux", messages=messages, max_tokens=16).choices[0].message.content == "ok"
            before = len(receipts)
            header = "x-api-key" if provider == "anthropic" else "Authorization"
            with unittest.TestCase().assertRaises(auth.AuthError):
                aux.call_llm(task="compression", messages=messages, max_tokens=16, extra_headers={header: "wrong-late-auth"})
            assert len(receipts) == before
            asyncio.run(async_routes(aux, auth, header, messages, receipts))
            assert aux.call_llm(task="compression", model="native-failing-model", messages=messages, max_tokens=16).choices[0].message.content == "ok"
            assert "native-fallback-model" in receipts
            sdk_guards(aux, auth, provider, receipts, key_only)
            before = len(receipts)
            with aux.aux_probe_mode():
                probe, _ = aux.resolve_provider_client("auto", model="native-model")
                assert probe is not None
            assert len(receipts) == before
            parent = authority.agent_class(model="native-model", enabled_toolsets=[], quiet_mode=True, skip_memory=True, skip_background_review=True, **authority.agent_kwargs)
            config = delegate._load_config()
            credentials = delegate._resolve_delegation_credentials(config, parent)
            child = delegate._build_child_preserving_parent_tools(task_index=0, goal="Offline proof", context="native", toolsets=None,
                model=credentials["model"], max_iterations=3, task_count=1, parent_agent=parent, **delegate._credential_overrides(credentials, config))
            assert isinstance(child, authority.agent_class) and child._credential_pool is None
            assert child.model == "native-child-model" and child._fallback_chain == []
            child_request(child, provider)
            assert not child._try_refresh_env_client_credentials()
            assert not child._try_refresh_anthropic_client_credentials()
            foreign = "openrouter" if provider != "openrouter" else "anthropic"
            with unittest.TestCase().assertRaises(auth.AuthError):
                delegate._resolve_delegation_credentials({"provider": foreign, "model": "foreign-opaque-model"}, parent)
            child.close()
            parent.close()
        assert (home / "plugins/auth-proof/registered").read_text() == "once\n"
        assert not (home / "plugins/disabled-proof/registered").exists()
        assert KEYS[provider] not in (home / "auth.json").read_text()
        assert "native-aux-model" in receipts and "native-plugin-model" in receipts
        assert any(cookies), "native HTTP cookie context did not reach the SDK transport"
        if key_only:
            assert {"native-aux.example", "native-fallback.example", "native-child.example"}.issubset(hosts)
        print(json.dumps({"provider": provider, "passed": True, "requests": len(receipts)}))


def sdk_guards(aux, auth, provider, receipts, key_only):
    import httpx
    client, _ = aux.resolve_provider_client("auto", model="native-clone-model")
    sdk = getattr(client, "_real_client", client)
    clone = sdk.with_options(timeout=2)
    create = clone.messages.create if provider == "anthropic" else clone.chat.completions.create
    messages = [{"role": "user", "content": "offline SDK clone"}]
    assert create(model="native-clone-model", messages=messages, max_tokens=16)
    assert clone._client.cookies.get("__cf_bm") == "synthetic-http-context"
    assert create(model="native-clone-model", messages=messages, max_tokens=16)
    configured_cookie = sdk.with_options(default_headers={"Cookie": "native-config=synthetic"})
    assert configured_cookie._custom_headers.get("Cookie") == "native-config=synthetic"
    configured_cookie.close()
    before = len(receipts)
    with unittest.TestCase().assertRaises(auth.AuthError):
        create(model="native-clone-model", messages=messages, max_tokens=16,
               extra_query={"api_key": "foreign-query"}, extra_body={"api_key": KEYS[provider]})
    with unittest.TestCase().assertRaises(auth.AuthError):
        clone.get(str(clone.base_url) + "models?api_key=foreign-query&api_key=" + KEYS[provider], cast_to=dict)
    with unittest.TestCase().assertRaises(auth.AuthError):
        create(model="native-clone-model", messages=messages, max_tokens=16, extra_query={"X-API-Key": "foreign-query"})
    with httpx.Client(transport=httpx.MockTransport(lambda _: (_ for _ in ()).throw(AssertionError("foreign clone HTTP")))) as fresh:
        with unittest.TestCase().assertRaises(auth.AuthError):
            sdk.with_options(api_key="foreign-key", base_url="https://foreign.example/v1", http_client=fresh)
        if not key_only:
            changed = sdk.with_options(base_url="https://foreign.example/v1", http_client=fresh)
            with unittest.TestCase().assertRaises(auth.AuthError):
                changed.get("https://foreign.example/v1/models", cast_to=dict)
            changed.close()
    assert len(receipts) == before
    clone.close()
    sdk.close()


def child_request(child, provider):
    messages = [{"role": "user", "content": "native child endpoint"}]
    if provider == "anthropic":
        client = child._create_request_anthropic_client(reason="offline-child")
        create = client.messages.create
    else:
        client = child._create_request_openai_client(reason="offline-child")
        create = client.chat.completions.create
    assert create(model=child.model, max_tokens=16, messages=messages)
    assert client._client.cookies.get("__cf_bm") == "synthetic-http-context"
    assert create(model=child.model, max_tokens=16, messages=messages)


async def async_routes(aux, auth, header, messages, receipts):
    import httpx
    assert (await aux.async_call_llm(task="compression", messages=messages, max_tokens=16)).choices[0].message.content == "ok"
    before = len(receipts)
    client, model = aux.resolve_provider_client("auto", async_mode=True)
    assert await client.chat.completions.create(model=model, messages=messages, max_tokens=16)
    assert await client.chat.completions.create(model=model, messages=messages, max_tokens=16)
    before = len(receipts)
    with unittest.TestCase().assertRaises(auth.AuthError):
        await client.chat.completions.create(model=model, messages=messages, max_tokens=16,
                                             extra_headers={header: "wrong-late-auth"})
    sdk = getattr(client, "_real_client", client)
    if hasattr(sdk, "with_options"):
        transport_type = httpx.Client if hasattr(client, "_real_client") else httpx.AsyncClient
        fresh = transport_type(transport=httpx.MockTransport(lambda _: (_ for _ in ()).throw(AssertionError("foreign async clone HTTP"))))
        with unittest.TestCase().assertRaises(auth.AuthError):
            sdk.with_options(api_key="foreign-key", base_url="https://foreign.example/v1", http_client=fresh)
        if isinstance(fresh, httpx.AsyncClient):
            await fresh.aclose()
        else:
            fresh.close()
    assert len(receipts) == before


def endpoint_worker():
    if os.environ.get("HERMES_SOURCE"):
        sys.path.insert(0, os.environ["HERMES_SOURCE"])
    with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-endpoint-") as directory:
        home = Path(directory)
        os.environ.update(HOME=directory, HERMES_HOME=directory)
        for name in list(ENV_KEYS.values()) + ["ANTHROPIC_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"]:
            os.environ.pop(name, None)
        settings(home, "openai")
        config = json.loads((home / "config.yaml").read_text())
        config["model"]["api_mode"] = "chat_completions"
        (home / "config.yaml").write_text(json.dumps(config))
        old, rotated = "sk-proj-native-synthetic", "sk-proj-rotated-synthetic"
        (home / ".env").write_text("OPENAI_API_KEY=" + old + "\n")
        original_connect = socket.socket.connect
        def no_network(sock, address):
            if isinstance(address, tuple):
                raise AssertionError("offline endpoint test attempted network")
            return original_connect(sock, address)
        socket.socket.connect = no_network
        import httpx
        requests = []
        def handle(request):
            assert request.url.host == "endpoint-only.example"
            assert request.headers["Authorization"] == "Bearer wrong-stored-key"
            assert request.headers["X-Native-Metadata"] == "kept"
            requests.append(True)
            return response(httpx, "openai", json.loads(request.content))
        sdk_fixtures(handle)
        from hermes_cli.config import invalidate_env_cache, load_config_readonly
        spec = importlib.util.spec_from_file_location("native_auth", AUTH)
        auth = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(auth)
        authority = auth.install("openai", None, "https://endpoint-only.example/v1", load_config_readonly(), model="native-model")
        with authority.auxiliary_scope():
            agent = authority.agent_class(model="native-model", enabled_toolsets=[], quiet_mode=True, skip_memory=True, skip_background_review=True, **authority.agent_kwargs)
        agent._try_refresh_env_client_credentials()
        (home / ".env").write_text("OPENAI_API_KEY=" + rotated + "\n")
        invalidate_env_cache()
        assert agent._try_refresh_env_client_credentials() and agent.api_key == rotated
        client = agent._create_request_openai_client(reason="offline-endpoint")
        assert client.chat.completions.create(model="native-model", messages=[{"role": "user", "content": "native endpoint auth"}], max_tokens=16)
        agent.close()
        assert requests


@unittest.skipUnless(os.environ.get("HERMES_SOURCE") or importlib.util.find_spec("run_agent"), "requires a native Hermes installation")
class NativeAuthorityTest(unittest.TestCase):
    def test_gateway_native_protocols_and_factories(self):
        for mode in ("chat_completions", "codex_responses", "anthropic_messages"):
            with self.subTest(mode=mode):
                completed = subprocess.run([sys.executable, __file__, "--gateway-worker", mode], stdout=subprocess.PIPE,
                                           stderr=subprocess.STDOUT, text=True, timeout=90)
                self.assertEqual(completed.returncode, 0, completed.stdout[-4000:])

    def test_endpoint_keeps_native_authentication(self):
        completed = subprocess.run([sys.executable, __file__, "--endpoint-worker"], stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, timeout=90)
        self.assertEqual(completed.returncode, 0, completed.stdout[-4000:])

    def test_native_sdk_routes(self):
        for provider in ENDPOINTS:
            for mode in ("key-base", "key-only"):
                with self.subTest(provider=provider, mode=mode):
                    completed = subprocess.run([sys.executable, __file__, "--worker", provider, mode], stdout=subprocess.PIPE,
                                               stderr=subprocess.STDOUT, text=True, timeout=90)
                    self.assertEqual(completed.returncode, 0, completed.stdout[-4000:])


def gateway_worker(mode):
    if os.environ.get("HERMES_SOURCE"):
        sys.path.insert(0, os.environ["HERMES_SOURCE"])
    with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-gateway-") as directory:
        home = Path(directory)
        os.environ.update(HOME=directory, HERMES_HOME=directory)
        config = {"model": {"provider": "custom", "default": "native-model", "api_mode": mode},
                  "auxiliary": {"compression": {"provider": "auto", "model": "native-model"}},
                  "memory": {"enabled": False, "memory_enabled": False, "user_profile_enabled": False}}
        (home / "config.yaml").write_text(json.dumps(config))
        import httpx
        receipts = []
        key, endpoint = "synthetic-gateway-secret", "https://gateway.example/owned/path"

        def handle(request):
            assert str(request.url).startswith(endpoint + "/")
            header = "x-api-key" if mode == "anthropic_messages" else "authorization"
            assert request.headers[header] == (key if mode == "anthropic_messages" else "Bearer " + key)
            body = json.loads(request.content)
            receipts.append(request.url.path)
            if mode == "codex_responses":
                assert request.url.path.endswith("/responses")
                result = response(httpx, "responses", body).json()
                events = [{"type": "response.output_item.done", "output_index": 0, "item": result["output"][0]},
                          {"type": "response.completed", "response": result}]
                stream = "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events)
                return httpx.Response(200, content=stream.encode(), headers={"content-type": "text/event-stream"})
            assert request.url.path.endswith("/messages" if mode == "anthropic_messages" else "/chat/completions")
            return response(httpx, "anthropic" if mode == "anthropic_messages" else "openai", body)

        sdk_fixtures(handle)
        spec = importlib.util.spec_from_file_location("native_auth", AUTH)
        auth = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(auth)
        authority = auth.install("gateway", key, endpoint, config, model="native-model")
        from agent import auxiliary_client as aux
        from tools import delegate_tool as delegate
        assert authority.agent_kwargs["api_mode"] == mode
        messages = [{"role": "user", "content": "offline gateway fixture"}]
        with authority.auxiliary_scope():
            result = aux.call_llm(task="compression", messages=messages, max_tokens=16)
            assert result.choices[0].message.content == "ok"
            before = len(receipts)
            header = "x-api-key" if mode == "anthropic_messages" else "Authorization"
            with unittest.TestCase().assertRaises(auth.AuthError):
                aux.call_llm(task="compression", messages=messages, max_tokens=16, extra_headers={header: "wrong-auth"})
            assert len(receipts) == before
            parent = authority.agent_class(model="native-model", enabled_toolsets=[], quiet_mode=True, skip_memory=True,
                                           skip_background_review=True, **authority.agent_kwargs)
            assert parent.api_mode == mode
            credentials = delegate._resolve_delegation_credentials({"provider": "auto"}, parent)
            assert credentials["api_mode"] == mode
            parent.close()
        assert receipts


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--endpoint-worker":
        endpoint_worker()
    elif len(sys.argv) > 2 and sys.argv[1] == "--gateway-worker":
        gateway_worker(sys.argv[2])
    elif len(sys.argv) > 2 and sys.argv[1] == "--worker":
        worker(sys.argv[2], len(sys.argv) > 3 and sys.argv[3] == "key-only")
    else:
        unittest.main()
