"""Native source-token exchange and AWS sessions for the owned Hermes worker."""
from contextvars import ContextVar
from inspect import iscoroutinefunction
from pathlib import Path
from types import SimpleNamespace
import os


def _error(message):
    # Same private module instance as the owner; imports must not start another authority.
    return ValueError("Hermes " + message)


def _without_pool(authority):
    from agent.credential_pool import CredentialPool
    from hermes_cli import runtime_provider
    load = runtime_provider.load_pool
    runtime_provider.load_pool = lambda provider: (CredentialPool(provider, [])
        if provider == authority.provider else load(provider))


class Copilot:
    def __init__(self, owner, source, directory):
        from agent.secret_scope import get_secret
        from hermes_cli import copilot_auth as native
        self.owner, self.source, self.issued = owner, source, set()
        resolve, exchange, get_token = native.resolve_copilot_token, native.exchange_copilot_token, native.get_copilot_api_token
        if source:
            if not native.validate_copilot_token(source)[0]:
                raise _error("declared GitHub token is unsupported by native Copilot authentication")
            if directory is None:
                raise _error("declared GitHub authentication requires private token storage")
            native._jwt_disk_path = lambda: Path(directory) / ".copilot_jwt.json"
            _without_pool(owner)

        def selected_source():
            if source:
                return source, "COPILOT_GITHUB_TOKEN"
            any_set = False
            for name in native.COPILOT_ENV_VARS:
                value = get_secret(name, "") or ""
                any_set |= bool(value)
                if value and native.validate_copilot_token(value)[0]:
                    return value, name
            return ("", "") if any_set else resolve()

        def checked(raw):
            if source and raw != source:
                raise _error("GitHub credential conflicts with the selected source token")

        def exchanged(raw, *args, **kwargs):
            checked(raw)
            result = exchange(raw, *args, **kwargs)
            self._issued(result[0])
            return result

        def token(raw):
            checked(raw)
            result = get_token(raw)
            # Native exchange can deliberately return the same source token on failure.
            self._issued(result[0])
            return result

        native.resolve_copilot_token = selected_source
        native.exchange_copilot_token = exchanged
        native.get_copilot_api_token = token
        self._token = lambda: token(selected_source()[0])[0]

    def _issued(self, key):
        if isinstance(key, str) and key:
            self.issued.add(key)
            self.owner._remember(key)

    def client_kwargs(self, kwargs):
        if self.source and kwargs.get("api_key") not in self.issued:
            kwargs["api_key"] = self._token()
        return kwargs

    agent_kwargs = client_kwargs

    def authentication(self, headers):
        if self.source and (set(headers) != {"authorization"} or
                headers["authorization"] not in {"Bearer " + key for key in self.issued}):
            raise _error("model request overrides the selected GitHub authentication")

    def parameters(self, request):
        _parameters(request, self.issued)

    def guard_client(self, client):
        return False

    def delegation(self, settings):
        if self.source:
            settings.pop("api_key", None)
            settings["provider"] = self.owner.provider
        return settings


def _parameters(request, allowed):
    import json
    values = list(request.url.params.multi_items())
    try:
        body = json.loads(request.content)
    except (ValueError, UnicodeDecodeError):
        body = None
    if isinstance(body, dict):
        values.extend(body.items())
    names = {"api_key", "apikey", "x_api_key", "access_token", "auth_token", "authorization"}
    for name, value in values:
        if name.lower().replace("-", "_") in names and value not in (None, ""):
            if value not in allowed and value not in {"Bearer " + key for key in allowed}:
                raise _error("model request carries conflicting authentication")


_AWS_AUTH = ("AWS_BEARER_TOKEN_BEDROCK", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN",
             "AWS_PROFILE", "AWS_DEFAULT_PROFILE", "AWS_WEB_IDENTITY_TOKEN_FILE", "AWS_ROLE_ARN", "AWS_ROLE_SESSION_NAME",
             "AWS_CONTAINER_AUTHORIZATION_TOKEN", "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
             "AWS_CONTAINER_CREDENTIALS_FULL_URI", "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
_AWS_SETTINGS = ("AWS_REGION", "AWS_DEFAULT_REGION", "AWS_CONFIG_FILE", "AWS_SHARED_CREDENTIALS_FILE", "AWS_EC2_METADATA_DISABLED")
_sdk_session = ContextVar("zeroshot_hermes_aws_session", default=None)


class Bedrock:
    def __init__(self, owner, source, declared):
        from agent import bedrock_adapter as native
        from agent.secret_scope import get_secret
        from hermes_cli import runtime_provider_backends
        self.owner, self.native, self.declared = owner, native, declared
        self.bound = owner._declared_auth
        self._validate_declaration()
        self.environ = dict(os.environ)
        for name in (*_AWS_AUTH, *_AWS_SETTINGS):
            value = get_secret(name, None)
            if value is not None:
                self.environ[name] = value
        if self.bound:
            for name in _AWS_AUTH:
                self.environ.pop(name, None)
            self.environ.update(declared)
        self.bearer = source or self.environ.get("AWS_BEARER_TOKEN_BEDROCK", "").strip()
        self.owner._remember(self.bearer)
        for name in _AWS_AUTH:
            self.owner._remember(self.environ.get(name))
        if declared.get("AWS_REGION"):
            owner.config.setdefault("bedrock", {})["region"] = declared["AWS_REGION"]
        # Native routing reads bearer presence directly; give only that module a private view.
        runtime_provider_backends.os = SimpleNamespace(environ=self.environ)
        native.resolve_bedrock_bearer_token = lambda env=None: (self.environ if env is None else env).get("AWS_BEARER_TOKEN_BEDROCK", "").strip()
        native.resolve_bedrock_region = lambda env=None: self._region(env)
        runtime_region = native.resolve_bedrock_runtime_region
        native.resolve_bedrock_runtime_region = lambda config=None: declared.get("AWS_REGION") or runtime_region(config)
        self.boto = native._require_boto3()
        native._require_boto3 = lambda: SimpleNamespace(Session=self.session, client=self.client)
        native.reset_client_cache()
        self._bind_anthropic()
        if self.bound:
            _without_pool(owner)
        self._bind_auxiliary()

    def _validate_declaration(self):
        if not self.bound:
            return
        values = self.declared
        complete = any((values.get("AWS_BEARER_TOKEN_BEDROCK"), values.get("AWS_PROFILE"), values.get("AWS_DEFAULT_PROFILE"),
                        values.get("AWS_ACCESS_KEY_ID") and values.get("AWS_SECRET_ACCESS_KEY"),
                        values.get("AWS_WEB_IDENTITY_TOKEN_FILE") and values.get("AWS_ROLE_ARN"),
                        values.get("AWS_CONTAINER_CREDENTIALS_FULL_URI"), values.get("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")))
        if not complete:
            raise _error("declared AWS authentication is incomplete")

    def _region(self, env=None):
        values = self.environ if env is None else env
        return values.get("AWS_REGION") or values.get("AWS_DEFAULT_REGION") or self.session().region_name or "us-east-1"

    def session(self, **kwargs):
        import botocore.session
        from botocore.tokens import ScopedEnvTokenProvider, SSOTokenProvider, TokenProviderChain
        session = botocore.session.Session()
        self._session_variables(session)
        self._check_kwargs(kwargs)
        result = self.boto.Session(botocore_session=session, **kwargs)
        session.register_component("credential_provider", self._credentials(session))
        tokens = [ScopedEnvTokenProvider(session, environ=self.environ)]
        if not self.bound:
            tokens.append(SSOTokenProvider(session))
        session.register_component("token_provider", TokenProviderChain(tokens))
        self._redact_credentials(session)
        native_client = result.client
        def client(service, **options):
            self._check_kwargs(options)
            if self.bearer and service in {"bedrock-runtime", "bedrock"}:
                from botocore.config import Config
                options["config"] = (options.get("config") or Config()).merge(Config(signature_version="bearer"))
            value = native_client(service, **options)
            if self.bound:
                self._bind_boto_signer(value, result)
            return value
        result.client = client
        return result

    def _redact_credentials(self, session):
        native = session.get_credentials
        def credentials():
            value = native()
            if value is not None and getattr(value, "_zeroshot_redactions", None) is not self.owner:
                freeze = value.get_frozen_credentials
                def frozen():
                    selected = freeze()
                    for field in ("access_key", "secret_key", "token"):
                        self.owner._remember(getattr(selected, field, None))
                    return selected
                value.get_frozen_credentials = frozen
                value._zeroshot_redactions = self.owner
            return value
        session.get_credentials = credentials

    def _session_variables(self, session):
        store = session.get_component("config_store")
        if self.bound:
            store.set_config_variable("profile", self.environ.get("AWS_DEFAULT_PROFILE") or self.environ.get("AWS_PROFILE"))
        variables = {"region": "AWS_DEFAULT_REGION", "config_file": "AWS_CONFIG_FILE",
                     "credentials_file": "AWS_SHARED_CREDENTIALS_FILE"}
        for setting, name in variables.items():
            if self.environ.get(name):
                store.set_config_variable(setting, self.environ[name])
        profile = self.environ.get("AWS_DEFAULT_PROFILE") or self.environ.get("AWS_PROFILE")
        if profile:
            store.set_config_variable("profile", profile)
        if self.environ.get("AWS_REGION"):
            store.set_config_variable("region", self.environ["AWS_REGION"])

    def _credentials(self, session):
        from botocore.credentials import CredentialResolver, EnvProvider
        if self.bound and self.bearer:
            return CredentialResolver([])
        if self.bound and self.declared.get("AWS_ACCESS_KEY_ID"):
            return CredentialResolver([EnvProvider(environ=self.environ)])
        resolver = session.get_component("credential_provider")
        self._bind_environment(resolver)
        return self._selected_resolver(resolver)

    def _bind_environment(self, resolver):
        for provider in resolver.providers:
            self._provider_environment(provider)

    def _provider_environment(self, provider):
        if hasattr(provider, "environ"):
            provider.environ = self.environ
        if hasattr(provider, "_environ"):
            provider._environ = self.environ
        if provider.METHOD == "assume-role-with-web-identity":
            provider._get_env_config = lambda key: (None if provider._disable_env_vars else self.environ.get(provider._CONFIG_TO_ENV_VAR.get(key)))
        sourcer = getattr(provider, "_credential_sourcer", None)
        for source in getattr(sourcer, "_providers", []):
            self._provider_environment(source)
        builder = getattr(provider, "_profile_provider_builder", None)
        if builder is not None:
            native = builder.providers
            def providers(*args, **kwargs):
                values = native(*args, **kwargs)
                for value in values:
                    self._provider_environment(value)
                return values
            builder.providers = providers

    def _selected_resolver(self, resolver):
        from botocore.credentials import CredentialResolver
        if not self.bound:
            return resolver
        if self.declared.get("AWS_WEB_IDENTITY_TOKEN_FILE"):
            allowed = {"assume-role-with-web-identity"}
        elif self.declared.get("AWS_PROFILE") or self.declared.get("AWS_DEFAULT_PROFILE"):
            allowed = {"assume-role", "assume-role-with-web-identity", "sso", "shared-credentials-file", "custom-process", "config-file", "login"}
        else:
            allowed = {"container-role"}
        return CredentialResolver([provider for provider in resolver.providers if provider.METHOD in allowed])

    def _check_kwargs(self, kwargs):
        if not self.bound:
            return
        fields = {"aws_access_key_id": "AWS_ACCESS_KEY_ID", "aws_secret_access_key": "AWS_SECRET_ACCESS_KEY",
                  "aws_session_token": "AWS_SESSION_TOKEN", "profile_name": "AWS_PROFILE"}
        for key, field in fields.items():
            expected = (self.declared.get("AWS_DEFAULT_PROFILE") or self.declared.get(field)) if key == "profile_name" else self.declared.get(field)
            if kwargs.get(key) is not None and kwargs[key] != expected:
                raise _error("native AWS client overrides declared authentication")
        region = self.declared.get("AWS_REGION")
        if region and kwargs.get("region_name", region) != region:
            raise _error("native AWS client overrides declared region")

    def client(self, service, **kwargs):
        return self.session().client(service, **kwargs)

    def _bind_boto_signer(self, client, session):
        signer = client._request_signer
        native_sign, native_auth = signer.sign, signer.get_auth_instance
        prepare = client._endpoint.prepare_request
        def auth(**kwargs):
            kwargs["request_credentials"] = session.get_credentials()
            return native_auth(**kwargs)
        def sign(operation, request, **kwargs):
            result = native_sign(operation, request, **kwargs)
            request.context["zeroshot_aws_auth"] = self._authentication(request.headers)
            return result
        def prepared(request):
            value = prepare(request)
            value._zeroshot_aws_auth = request.context.get("zeroshot_aws_auth")
            return value
        signer.sign, signer.get_auth_instance = sign, auth
        client._endpoint.prepare_request = prepared
        client.meta.events.register_last("before-send", self._signed_request)

    def _signed_request(self, request, **_):
        received = self._authentication(request.headers)
        expected = getattr(request, "_zeroshot_aws_auth", None)
        self._signed_headers(expected, received)

    def _authentication(self, headers):
        values = {str(name).lower(): value.decode() if isinstance(value, bytes) else value
                  for name, value in headers.items()}
        return {name: value for name, value in values.items() if not self.owner._headers({name: value})}

    def _signed_headers(self, expected, received):
        if not expected or expected != received:
            raise _error("model request overrides the selected AWS authentication")
        if self.bearer:
            self.authentication(received)
        elif set(received) - {"authorization", "x-amz-security-token"}:
            raise _error("model request carries conflicting AWS authentication headers")
        elif not received.get("authorization", "").startswith("AWS4-HMAC-SHA256 Credential="):
            raise _error("model request lacks native AWS signing")

    def parameters(self, request):
        _parameters(request, {self.bearer} if self.bearer else set())

    def _bind_anthropic(self):
        from anthropic.lib.bedrock import _auth
        native_session = _auth._get_session
        def session(**kwargs):
            selected = _sdk_session.get()
            if selected is None:
                return native_session(**kwargs)
            credentials = selected.get_credentials()
            frozen = credentials.get_frozen_credentials() if credentials is not None else None
            return SimpleNamespace(region_name=selected.region_name, get_credentials=lambda: frozen)
        _auth._get_session = session

    def _bind_auxiliary(self):
        auxiliary = self.owner._auxiliary
        build = auxiliary._build_bedrock_client

        def client(provider, model, *, raw_codex):
            if self.bearer and not self.native.bedrock_openai_uses_mantle(model or ""):
                region = self.native.resolve_bedrock_runtime_region()
                return auxiliary.BedrockAuxiliaryClient(region, model), model
            value, model = build(provider, model, raw_codex=raw_codex)
            if value is not None:
                self.owner._guard_client(value)
            return value, model

        auxiliary._build_bedrock_client = client

    def client_kwargs(self, kwargs):
        if self.bearer:
            kwargs["api_key"] = self.bearer
        return kwargs

    def agent_kwargs(self, kwargs):
        if self.bearer and kwargs.get("api_mode") == "codex_responses":
            kwargs["api_key"] = self.bearer
        return kwargs

    def authentication(self, headers):
        if self.bound and not self.bearer:
            raise _error("declared IAM authentication requires a native AWS model client")
        if self.bound and self.bearer and headers != {"authorization": "Bearer " + self.bearer}:
            raise _error("model request overrides the selected Bedrock bearer")

    def guard_client(self, client):
        from anthropic import AnthropicBedrock, AsyncAnthropicBedrock
        from agent import auxiliary_client as auxiliary
        if isinstance(client, (auxiliary.BedrockAuxiliaryClient, auxiliary.AsyncBedrockAuxiliaryClient)):
            return True
        if isinstance(client, (AnthropicBedrock, AsyncAnthropicBedrock)):
            if getattr(client, "_zeroshot_aws_owner", None) is self:
                return True
            session = self.session()
            if self.bound:
                client._custom_headers = self.owner._headers(client._custom_headers)
            prepare = client._prepare_request
            if iscoroutinefunction(prepare):
                async def owned(request):
                    token = _sdk_session.set(session)
                    try:
                        result = await prepare(request)
                        request.extensions["zeroshot_aws_auth"] = self._authentication(request.headers)
                        return result
                    finally:
                        _sdk_session.reset(token)
            else:
                def owned(request):
                    token = _sdk_session.set(session)
                    try:
                        result = prepare(request)
                        request.extensions["zeroshot_aws_auth"] = self._authentication(request.headers)
                        return result
                    finally:
                        _sdk_session.reset(token)
            client._prepare_request = owned
            if self.bound:
                self._guard_iam_transport(client)
            client._zeroshot_aws_owner = self
            self.owner._protect_sdk(client)
            return True
        if getattr(client, "api_key", None) == "aws-sdk":
            # Native async auxiliary rebuild drops its sync HTTP client; reinstall its native signer.
            client._client.auth = self._mantle_signer()
            if self.bound:
                self._guard_iam_transport(client)
            self.owner._protect_sdk(client)
            return True
        return False

    def _mantle_signer(self):
        import httpx
        native = self.native.BedrockOpenAISigV4Auth(self._region())
        owner = self
        class Signer(httpx.Auth):
            requires_request_body = True
            def auth_flow(self, request):
                for signed in native.auth_flow(request):
                    signed.extensions["zeroshot_aws_auth"] = owner._authentication(signed.headers)
                    yield signed
        return Signer()

    def _guard_iam_transport(self, client):
        if getattr(client._client, "_zeroshot_aws_owner", None) is self:
            return
        import httpx
        def guard(request):
            try:
                self._signed_headers(request.extensions.get("zeroshot_aws_auth"), self._authentication(request.headers))
                self.parameters(request)
            except ValueError as error:
                self.owner.refuse(error)
        if isinstance(client._client, httpx.AsyncClient):
            async def async_guard(request):
                guard(request)
            hook = async_guard
        else:
            hook = guard
        client._client.event_hooks.setdefault("request", []).append(hook)
        client._client._zeroshot_aws_owner = self

    def delegation(self, settings):
        settings.pop("api_key", None)
        settings["provider"] = self.owner.provider
        return settings


def install(owner, source, directory, declared):
    global _error
    _error = owner.error
    if owner.product_provider == "github":
        return Copilot(owner, source, directory)
    return Bedrock(owner, source, declared)
