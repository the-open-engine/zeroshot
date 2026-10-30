"""Offline native SDK auth/refresh proofs; run with the installed Hermes interpreter."""
import asyncio
from datetime import datetime, timedelta, timezone
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

BASE = Path(__file__).resolve().parents[1]
MESSAGES = [{"role": "user", "content": "offline native authentication proof"}]


def load(name):
    spec = importlib.util.spec_from_file_location(name, BASE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def setup(home, provider, model):
    if os.environ.get("HERMES_SOURCE"):
        sys.path.insert(0, os.environ["HERMES_SOURCE"])
    os.environ.update(HOME=str(home), HERMES_HOME=str(home), AWS_EC2_METADATA_DISABLED="true")
    from agent.ssl_verify import install_truststore
    install_truststore()
    config = {"model": {"provider": provider, "default": model, "context_length": 65536},
              "memory": {"enabled": False, "memory_enabled": False, "user_profile_enabled": False},
              "auxiliary": {"transient_retries": 0, "compression": {"provider": "auto", "model": model}},
              "delegation": {"provider": provider, "model": model}}
    (home / "config.yaml").write_text(json.dumps(config))
    return config


def http_response(request):
    import httpx
    from native_auth_test import response
    body = json.loads(request.content) if request.content else {}
    body.setdefault("model", "fixture")
    if request.url.path.endswith("/responses"):
        provider = "responses"
    else:
        provider = "anthropic" if "/invoke" in request.url.path else "openai"
    return response(httpx, provider, body)


def sdk_transport(receipts):
    import httpx
    import openai
    from anthropic import AnthropicBedrock, AsyncAnthropicBedrock
    def handle(request):
        receipts.append(dict(request.headers))
        return http_response(request)
    def transport(kwargs, factory):
        client = kwargs.get("http_client") or factory()
        client._transport = httpx.MockTransport(handle)
        client._mounts = {}
        kwargs["http_client"] = client
    native, native_async = openai.OpenAI, openai.AsyncOpenAI
    class OpenAI(native):
        def __init__(self, *args, **kwargs):
            transport(kwargs, httpx.Client)
            super().__init__(*args, **kwargs)
    class AsyncOpenAI(native_async):
        def __init__(self, *args, **kwargs):
            transport(kwargs, httpx.AsyncClient)
            super().__init__(*args, **kwargs)
    openai.OpenAI, openai.AsyncOpenAI = OpenAI, AsyncOpenAI
    for cls in (AnthropicBedrock, AsyncAnthropicBedrock):
        native_init = cls.__init__
        def init(self, *args, original=native_init, **kwargs):
            factory = httpx.AsyncClient if isinstance(self, AsyncAnthropicBedrock) else httpx.Client
            transport(kwargs, factory)
            original(self, *args, **kwargs)
        cls.__init__ = init


def parent(authority, model):
    return authority.agent_class(model=model, enabled_toolsets=[], quiet_mode=True, skip_memory=True,
        skip_background_review=True, **authority.agent_kwargs)


def delegated(authority, agent):
    from tools import delegate_tool
    config = delegate_tool._load_config()
    credentials = delegate_tool._resolve_delegation_credentials(config, agent)
    child = delegate_tool._build_child_preserving_parent_tools(task_index=0, goal="Offline proof", context="native", toolsets=None,
        model=credentials["model"], max_iterations=2, task_count=1, parent_agent=agent,
        **delegate_tool._credential_overrides(credentials, config))
    assert isinstance(child, authority.agent_class) and child.provider == agent.provider
    return child


def auxiliary_requests(auxiliary):
    assert auxiliary.call_llm(task="compression", messages=MESSAGES, max_tokens=1).choices[0].message.content == "ok"
    assert asyncio.run(auxiliary.async_call_llm(task="compression", messages=MESSAGES, max_tokens=1)).choices[0].message.content == "ok"


def github(home):
    source = "ghu_synthetic_declared_source"
    config = setup(home, "copilot", "gpt-4o")
    config["model"]["api_mode"] = "chat_completions"
    (home / "config.yaml").write_text(json.dumps(config))
    (home / ".env").write_text("COPILOT_GITHUB_TOKEN=ghu_synthetic_foreign_source\n")
    (home / ".copilot_jwt.json").write_text("{}")
    private = home / "worker"
    private.mkdir()
    load("native_environment").install({}, {}, {"COPILOT_GITHUB_TOKEN": source})
    from hermes_cli import copilot_auth as native
    exchanges, receipts = [], []
    def exchange(request, timeout):
        assert request.get_header("Authorization") == "token " + source
        exchanges.append(1)
        value = {"token": f"synthetic-service-{len(exchanges)}", "expires_at": int(time.time()) + 3600,
                 "endpoints": {"api": "https://enterprise.githubcopilot.com"}}
        return io.BytesIO(json.dumps(value).encode())
    native._urlopen_bounded = exchange
    sdk_transport(receipts)
    auth = load("native_auth")
    authority = auth.install("github", source, None, config, model="gpt-4o", directory=private)
    agent = parent(authority, "gpt-4o")
    assert agent.api_key == "synthetic-service-1"
    assert agent.base_url == "https://enterprise.githubcopilot.com"
    client = agent._create_request_openai_client(reason="native-auth-proof")
    client.chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1)
    native.evict_cached_exchanged_token(source)
    assert agent._try_refresh_copilot_client_credentials()
    assert agent.api_key == "synthetic-service-2"
    agent._create_request_openai_client(reason="after-refresh").chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1)
    # The old issued client may share an HTTP pool with the refreshed one.
    client.chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1)
    native.evict_cached_exchanged_token(source)
    assert agent._try_recover_stale_copilot_credential()
    assert agent.api_key == "synthetic-service-3"
    github_helpers(authority, agent, auth, source, receipts)
    assert (home / ".copilot_jwt.json").read_text() == "{}"
    assert native._jwt_disk_path() == private / ".copilot_jwt.json"
    assert source not in native._jwt_disk_path().read_text()
    assert "COPILOT_GITHUB_TOKEN" not in os.environ or os.environ["COPILOT_GITHUB_TOKEN"] != source
    assert authority.sanitize_owned_error(agent.api_key) == "[REDACTED]"
    agent.close()


def github_helpers(authority, agent, auth, source, receipts):
    from hermes_cli import copilot_auth as native
    from agent import auxiliary_client as auxiliary
    from tools import delegate_tool
    with authority.auxiliary_scope(agent):
        auxiliary_requests(auxiliary)
        native.evict_cached_exchanged_token(source)
        assert auxiliary._refresh_copilot_credentials()
        settings = delegate_tool._resolve_delegation_credentials({"provider": "copilot", "model": "gpt-4o"}, agent)
        assert settings["api_key"] in authority._provider_auth.issued
        child = delegated(authority, agent)
        assert child._credential_pool is None
        child._create_request_openai_client(reason="native-child").chat.completions.create(model=child.model, messages=MESSAGES, max_tokens=1)
        child.close()
        sdk, _ = auxiliary.resolve_provider_client("auto", model="gpt-4o")
        sdk = getattr(sdk, "_real_client", sdk)
        clone = sdk.with_options(timeout=3)
        clone.chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1)
        before = len(receipts)
        with unittest.TestCase().assertRaises(auth.AuthError):
            clone.chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1, extra_headers={"Authorization": "Bearer foreign"})
        with unittest.TestCase().assertRaises(auth.AuthError):
            clone.chat.completions.create(model="gpt-4o", messages=MESSAGES, max_tokens=1, extra_query={"api_key": "foreign"})
        with unittest.TestCase().assertRaises(auth.AuthError):
            sdk.with_options(api_key="foreign")
        assert len(receipts) == before
        with unittest.TestCase().assertRaises(auth.AuthError):
            delegate_tool._resolve_delegation_credentials({"provider": "openrouter"}, agent)
        clone.close()
    assert all(value["authorization"].startswith("Bearer synthetic-service-") for value in receipts)


def boto_transport(receipts):
    import boto3
    from botocore.awsrequest import AWSResponse
    native = boto3.Session.client
    def client(self, *args, **kwargs):
        result = native(self, *args, **kwargs)
        def send(request):
            receipts.append(dict(request.headers))
            response = {"output": {"message": {"role": "assistant", "content": [{"text": "ok"}]}},
                "stopReason": "end_turn", "usage": {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2}, "metrics": {"latencyMs": 1}}
            raw = io.BytesIO(json.dumps(response).encode())
            raw.stream = lambda **_: iter([raw.getvalue()])
            return AWSResponse(request.url, 200, {"content-type": "application/json"}, raw)
        result._endpoint.http_session.send = send
        return result
    boto3.Session.client = client


def bedrock_bearer(home):
    key = "synthetic-declared-bedrock-bearer"
    model = "us.anthropic.claude-sonnet-4-6"
    config = setup(home, "bedrock", model)
    config["bedrock"] = {"region": "eu-west-1", "guardrail": {"guardrail_identifier": "native-guardrail", "guardrail_version": "1"}}
    (home / "config.yaml").write_text(json.dumps(config))
    os.environ.update(AWS_PROFILE="foreign-missing-profile", AWS_ACCESS_KEY_ID="FOREIGN", AWS_SECRET_ACCESS_KEY="foreign")
    load("native_environment").install({}, {"AWS_REGION": "us-east-1"}, {"AWS_BEARER_TOKEN_BEDROCK": key})
    receipts = []
    boto_transport(receipts)
    auth = load("native_auth")
    authority = auth.install("bedrock", key, None, config, model=model, aws={"AWS_BEARER_TOKEN_BEDROCK": key, "AWS_REGION": "us-east-1"})
    assert authority.runtime["api_mode"] == "bedrock_converse"
    assert authority.runtime["region"] == "us-east-1"
    agent = parent(authority, model)
    assert agent.api_mode == "bedrock_converse"
    child = delegated(authority, agent)
    assert child.api_mode == "bedrock_converse" and child._bedrock_region == "us-east-1"
    child.close()
    from agent import bedrock_adapter as native, auxiliary_client as auxiliary
    client = native._get_bedrock_runtime_client("us-east-1")
    assert client.meta.region_name == "us-east-1"
    client.converse(modelId=model, messages=[{"role": "user", "content": [{"text": "ok"}]}])
    with authority.auxiliary_scope(agent):
        auxiliary_requests(auxiliary)
    assert all(value["Authorization"] == ("Bearer " + key).encode() for value in receipts)
    before = len(receipts)
    def foreign(request, **_):
        request.headers["Authorization"] = b"Bearer foreign"
    client.meta.events.register_first("before-send.bedrock-runtime.Converse", foreign)
    with unittest.TestCase().assertRaises(auth.AuthError):
        client.converse(modelId=model, messages=[{"role": "user", "content": [{"text": "ok"}]}])
    assert len(receipts) == before
    with unittest.TestCase().assertRaises(auth.AuthError):
        authority._provider_auth.session(aws_access_key_id="foreign", aws_secret_access_key="foreign")
    agent.close()


def bedrock_mantle(home):
    key, model = "synthetic-mantle-bearer", "openai.gpt-5.6-sol"
    config = setup(home, "bedrock", model)
    load("native_environment").install({}, {"AWS_REGION": "us-east-1"}, {"AWS_BEARER_TOKEN_BEDROCK": key})
    receipts = []
    sdk_transport(receipts)
    auth = load("native_auth")
    authority = auth.install("bedrock", key, None, config, model=model, aws={"AWS_BEARER_TOKEN_BEDROCK": key, "AWS_REGION": "us-east-1"})
    assert authority.runtime["api_mode"] == "codex_responses"
    agent = parent(authority, model)
    client = agent._create_request_openai_client(reason="native-mantle")
    assert client.responses.create(model=model, input="proof").output_text == "ok"
    clone = client.with_options(timeout=2)
    assert clone.responses.create(model=model, input="proof").output_text == "ok"
    with unittest.TestCase().assertRaises(auth.AuthError):
        clone.responses.create(model=model, input="proof", extra_headers={"Authorization": "Bearer foreign"})
    assert all(value["authorization"] == "Bearer " + key for value in receipts)
    agent.close()


def native_profile(home):
    profile = home / "aws-config"
    program = home / "credential-process.py"
    counter = home / "credential-count"
    program.write_text(f'''import json,time\nfrom pathlib import Path\np=Path({str(counter)!r})\nn=int(p.read_text())+1 if p.exists() else 1\np.write_text(str(n))\nprint(json.dumps({{"Version":1,"AccessKeyId":"AKIAPROCESS"+str(n),"SecretAccessKey":"synthetic-secret","SessionToken":"synthetic-session-"+str(n),"Expiration":time.strftime("%Y-%m-%dT%H:%M:%SZ",time.gmtime(time.time()+3600))}}))\n''')
    profile.write_text(f'[profile native]\nregion=us-east-1\ncredential_process = "{sys.executable}" "{program}"\n')
    return {"AWS_PROFILE": "native", "AWS_CONFIG_FILE": str(profile)}


def bedrock_iam(home):
    model = "us.anthropic.claude-sonnet-4-6"
    config = setup(home, "bedrock", model)
    captured = native_profile(home)
    load("native_environment").install(captured)
    receipts = []
    sdk_transport(receipts)
    auth = load("native_auth")
    authority = auth.install("bedrock", None, None, config, model=model, aws={**captured, "AWS_REGION": "us-east-1"})
    assert authority.runtime["api_mode"] == "anthropic_messages"
    agent = parent(authority, model)
    client = agent._create_request_anthropic_client(reason="native-iam-proof")
    assert client.messages.create(model=model, messages=MESSAGES, max_tokens=1).content[0].text == "ok"
    strategy = authority._provider_auth
    from inspect import getclosurevars
    session = getclosurevars(client._prepare_request).nonlocals["session"]
    credentials = session.get_credentials()
    first = credentials.get_frozen_credentials().access_key
    credentials._expiry_time = datetime.now(timezone.utc) + timedelta(seconds=1)
    client.messages.create(model=model, messages=MESSAGES, max_tokens=1)
    second = credentials.get_frozen_credentials().access_key
    assert first != second
    assert first in receipts[0]["authorization"] and second in receipts[1]["authorization"]
    assert authority.sanitize_owned_error(credentials.get_frozen_credentials().token) == "[REDACTED]"
    # The SDK obtains a refreshed native credential every request through the scoped session.
    child = delegated(authority, agent)
    child._create_request_anthropic_client(reason="native-iam-child").messages.create(model=model, messages=MESSAGES, max_tokens=1)
    child.close()
    sdk_iam_clients(authority, client, model, receipts)
    assert all(value["authorization"].startswith("AWS4-HMAC-SHA256 Credential=AKIAPROCESS") for value in receipts)
    agent.close()


def sdk_iam_clients(authority, client, model, receipts):
    from anthropic import AsyncAnthropicBedrock
    clone = client.with_options(timeout=2, http_client=client._client)
    assert clone._client is client._client
    clone.messages.create(model=model, messages=MESSAGES, max_tokens=1)
    asynchronous = authority._guard_client(AsyncAnthropicBedrock(aws_region="us-east-1", max_retries=0))
    async def call():
        value = await asynchronous.messages.create(model=model, messages=MESSAGES, max_tokens=1)
        assert value.content[0].text == "ok"
        await asynchronous.close()
    asyncio.run(call())
    assert len(receipts) >= 3
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda _: clone.messages.create(model=model, messages=MESSAGES, max_tokens=1), range(8)))
    before = len(receipts)
    with unittest.TestCase().assertRaises(ValueError):
        clone.messages.create(model=model, messages=MESSAGES, max_tokens=1, extra_headers={"X-Api-Key": "foreign"})
    assert len(receipts) == before
    clone.close()


def bedrock_mantle_iam(home):
    model = "openai.gpt-5.6-sol"
    config = setup(home, "bedrock", model)
    declared = {**native_profile(home), "AWS_REGION": "us-east-1"}
    load("native_environment").install({}, declared)
    receipts = []
    sdk_transport(receipts)
    auth = load("native_auth")
    authority = auth.install("bedrock", None, None, config, model=model, aws=declared)
    agent = parent(authority, model)
    client = agent._create_request_openai_client(reason="native-mantle-iam")
    assert client.responses.create(model=model, input="proof").output_text == "ok"
    from openai import AsyncOpenAI, OpenAI
    with unittest.TestCase().assertRaises(auth.AuthError):
        authority._guard_client(OpenAI(api_key="foreign", base_url=authority.runtime["base_url"]))
    asynchronous = authority._guard_client(AsyncOpenAI(api_key="aws-sdk", base_url=authority.runtime["base_url"]))
    async def call():
        assert (await asynchronous.responses.create(model=model, input="proof")).output_text == "ok"
        await asynchronous.close()
    asyncio.run(call())
    assert all(value["authorization"].startswith("AWS4-HMAC-SHA256 Credential=AKIAPROCESS") for value in receipts)
    with unittest.TestCase().assertRaises(auth.AuthError):
        client.responses.create(model=model, input="proof", extra_headers={"X-Api-Key": "foreign"})
    agent.close()


def aws_sources(home):
    config = setup(home, "bedrock", "opaque-model")
    os.environ.update(AWS_PROFILE="foreign-missing", AWS_ACCESS_KEY_ID="FOREIGN", AWS_SECRET_ACCESS_KEY="foreign")
    from types import SimpleNamespace
    auth = load("native_auth")
    source_path = BASE / "native_providers.py"
    spec = importlib.util.spec_from_file_location("source_auth", source_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    from agent import auxiliary_client
    owner = SimpleNamespace(_declared_auth=True, config=config, _auxiliary=auxiliary_client, provider="bedrock",
        _remember=lambda _: None, error=auth.Authority.error, _headers=auth.Authority._headers)
    declared = {"AWS_WEB_IDENTITY_TOKEN_FILE": str(home / "web-token"), "AWS_ROLE_ARN": "arn:aws:iam::123456789012:role/owned", "AWS_REGION": "us-east-1"}
    own = module.Bedrock(owner, None, declared)
    resolver = own.session()._session.get_component("credential_provider")
    assert [provider.METHOD for provider in resolver.providers] == ["assume-role-with-web-identity"]
    assert resolver.providers[0]._get_env_config("role_arn") == declared["AWS_ROLE_ARN"]
    assert resolver.providers[0]._get_env_config("web_identity_token_file") == declared["AWS_WEB_IDENTITY_TOKEN_FILE"]
    assert "AWS_PROFILE" not in own.environ and "AWS_ACCESS_KEY_ID" not in own.environ
    own.declared = {"AWS_CONTAINER_CREDENTIALS_FULL_URI": "http://127.0.0.1/owned"}
    own.environ = dict(own.declared)
    resolver = own.session()._session.get_component("credential_provider")
    assert [provider.METHOD for provider in resolver.providers] == ["container-role"]
    profile = home / "empty-config"
    profile.write_text("[profile empty]\nregion=us-east-1\n")
    own.declared = {"AWS_PROFILE": "empty", "AWS_CONFIG_FILE": str(profile)}
    own.environ = dict(own.declared)
    session = own.session()
    assert session.get_credentials() is None, "declared profile borrowed an ambient credential source"
    for fragment in ({"AWS_SESSION_TOKEN": "partial"}, {"AWS_CONTAINER_AUTHORIZATION_TOKEN": "partial"}):
        assert auth.declared_aws_auth(fragment)
        with unittest.TestCase().assertRaises(ValueError):
            module.Bedrock(owner, None, fragment)


def aws_settings(home):
    setup(home, "bedrock", "opaque-model")
    ca = home / "native-ca.pem"
    ca.write_text("synthetic CA path; no network request")
    os.environ.update(AWS_CA_BUNDLE=str(ca), AWS_ENDPOINT_URL_BEDROCK_RUNTIME="http://127.0.0.1:1234/native",
                      AWS_MAX_ATTEMPTS="2", AWS_RETRY_MODE="standard")
    declared = {"AWS_ENDPOINT_URL_BEDROCK_RUNTIME": "http://127.0.0.1:2345/declared", "AWS_MAX_ATTEMPTS": "5", "AWS_REGION": "us-east-1"}
    private = {"AWS_ACCESS_KEY_ID": "AKIAPRIVATE_SETTINGS", "AWS_SECRET_ACCESS_KEY": "private-secret"}
    load("native_environment").install(private, declared)
    auth = load("native_auth")
    assert not auth.declared_aws_auth(declared)
    authority = auth.install("bedrock", None, None, {}, model="opaque-model", aws=declared)
    client = authority._provider_auth.session().client("bedrock-runtime", region_name="us-east-1")
    assert client.meta.endpoint_url == declared["AWS_ENDPOINT_URL_BEDROCK_RUNTIME"]
    assert client._endpoint.http_session._verify == str(ca)
    assert client.meta.config.retries == {"total_max_attempts": 5, "mode": "standard"}
    assert client._request_signer._credentials.access_key == private["AWS_ACCESS_KEY_ID"]
    assert "AWS_ACCESS_KEY_ID" not in os.environ


def aws_nested_profile(home):
    setup(home, "bedrock", "opaque-model")
    config = home / "native-aws-config"
    config.write_text("[profile role]\nregion=us-east-1\nrole_arn=arn:aws:iam::123456789012:role/owned\ncredential_source=Environment\n")
    captured = {"AWS_PROFILE": "role", "AWS_CONFIG_FILE": str(config), "AWS_ACCESS_KEY_ID": "AKIAPRIVATE_SOURCE",
                "AWS_SECRET_ACCESS_KEY": "private-source-secret", "AWS_SESSION_TOKEN": "private-source-session"}
    load("native_environment").install(captured)
    from types import SimpleNamespace
    from agent import auxiliary_client
    import botocore.session
    requests = []
    native_client = botocore.session.Session.create_client
    def client(session, service, **kwargs):
        if service != "sts":
            return native_client(session, service, **kwargs)
        assert kwargs["aws_access_key_id"] == captured["AWS_ACCESS_KEY_ID"]
        assert kwargs["aws_secret_access_key"] == captured["AWS_SECRET_ACCESS_KEY"]
        assert kwargs["aws_session_token"] == captured["AWS_SESSION_TOKEN"]
        def assume_role(**params):
            assert params["RoleArn"].endswith("role/owned")
            requests.append(1)
            return {"Credentials": {"AccessKeyId": f"AKIAPRIVATEROLE{len(requests)}", "SecretAccessKey": "private-role-secret",
                "SessionToken": f"private-role-session-{len(requests)}", "Expiration": datetime.now(timezone.utc) + timedelta(hours=1)}}
        return SimpleNamespace(assume_role=assume_role)
    botocore.session.Session.create_client = client
    module = load("native_providers")
    owner = SimpleNamespace(_declared_auth=False, config={}, _auxiliary=auxiliary_client, provider="bedrock", _remember=lambda _: None)
    strategy = module.Bedrock(owner, None, {})
    session = strategy.session(profile_name="role")
    credentials = session.get_credentials()
    first = credentials.get_frozen_credentials()
    assert first.access_key == "AKIAPRIVATEROLE1"
    credentials._expiry_time = datetime.now(timezone.utc) + timedelta(seconds=1)
    for cached in credentials._refresh_using.__self__._cache.values():
        cached["Credentials"]["Expiration"] = datetime.now(timezone.utc) + timedelta(seconds=1)
    second = credentials.get_frozen_credentials()
    assert second.access_key == "AKIAPRIVATEROLE2" and len(requests) == 2
    assert not any(name in os.environ for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN"))


def github_native_aliases(home):
    setup(home, "copilot", "opaque-model")
    from hermes_cli import copilot_auth as native
    from types import SimpleNamespace
    environment = load("native_environment")
    environment.install({}, {}, {"COPILOT_GITHUB_TOKEN": "invalid", "GH_TOKEN": "gho_synthetic_valid"})
    module = load("native_providers")
    native.resolve_copilot_token = lambda: (_ for _ in ()).throw(AssertionError("unexpected gh fallback"))
    owner = SimpleNamespace(_remember=lambda _: None)
    module.Copilot(owner, None, None)
    assert native.resolve_copilot_token() == ("gho_synthetic_valid", "GH_TOKEN")
    environment.install({}, {}, {"COPILOT_GITHUB_TOKEN": "invalid"})
    assert native.resolve_copilot_token() == ("", "")



@unittest.skipUnless(os.environ.get("HERMES_SOURCE") or importlib.util.find_spec("run_agent"), "requires a native Hermes installation")
class NativeProviderTest(unittest.TestCase):
    def test_native_authentication_and_refresh(self):
        for scenario in ("github", "github_native_aliases", "bedrock_bearer", "bedrock_mantle", "bedrock_iam", "bedrock_mantle_iam", "aws_sources", "aws_nested_profile", "aws_settings"):
            with self.subTest(scenario=scenario):
                result = subprocess.run([sys.executable, __file__, "--worker", scenario], capture_output=True, text=True, timeout=90)
                self.assertEqual(result.returncode, 0, result.stdout[-2000:] + result.stderr[-6000:])


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--worker":
        with tempfile.TemporaryDirectory(prefix="zeroshot-hermes-provider-") as directory:
            globals()[sys.argv[2]](Path(directory))
    else:
        unittest.main()
