"""Owned model authority over Hermes's native v0.22.5 embedding interfaces.

Only this worker's native model factories are wrapped. Configuration, plugins,
MCP, SDK transports and unrelated plugin HTTP requests remain Hermes-owned.
"""

from contextlib import contextmanager
from copy import deepcopy
from functools import partial
from inspect import signature
import json
import re
from urllib.parse import urlsplit

_PROVIDERS = {"openai": "openai-api", "anthropic": "anthropic", "openrouter": "openrouter", "gateway": "custom", "github": "copilot", "bedrock": "bedrock"}
_AUTH_HEADER = re.compile(r"^(authorization|proxy-authorization)$|(^|[-_])(api[-_]?key|auth|token|secret)([-_]|$)", re.I)
_AUTH_PARAMETER = {"api_key", "apikey", "x_api_key", "access_token", "auth_token", "authorization"}


class AuthError(ValueError):
    """A credential-free authority refusal that the runtime may show to its user."""


class _RequestRefused(BaseException):
    # SDKs relabel ordinary request-hook exceptions as retryable connection errors.
    def __init__(self, error):
        self.error = error


def gateway_mode(provider, config):
    if provider != "gateway":
        return None
    mode = (config.get("model") or {}).get("api_mode")
    if mode not in {"codex_responses", "chat_completions", "anthropic_messages"}:
        raise AuthError("Hermes gateway requires an explicit supported API mode")
    return mode


def declared_aws_auth(values):
    return any(name in values for name in (
        "AWS_BEARER_TOKEN_BEDROCK", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_PROFILE", "AWS_DEFAULT_PROFILE",
        "AWS_WEB_IDENTITY_TOKEN_FILE", "AWS_ROLE_ARN", "AWS_ROLE_SESSION_NAME", "AWS_SESSION_TOKEN",
        "AWS_CONTAINER_AUTHORIZATION_TOKEN", "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI", "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI"))


class Authority:
    def __init__(self, provider, credentials, config, **native):
        declared_key, declared_base = credentials
        model = native.get("model")
        from agent import auxiliary_client as auxiliary
        from gateway.session_policy import _apply_launch_overrides
        from hermes_cli.runtime_provider import frozen_runtime_config, resolve_runtime_provider

        self.product_provider, self.provider = provider, _PROVIDERS[provider]
        self._api_mode = gateway_mode(provider, config)
        self._pin_endpoint = bool(declared_base)
        self._guard_required = bool(declared_key or declared_base)
        self.declared_key = declared_key if provider not in {"github", "bedrock"} else None
        self._declared_auth = bool(declared_key or declared_aws_auth(native.get("aws", {})))
        self.declared_base = declared_base
        self.config = deepcopy(config)
        self.redactions = set()
        self._auxiliary = auxiliary
        self._model = model
        self._main_agent = None
        self._remember(declared_key)
        self._provider_auth = self._install_provider(declared_key, native)
        self._guard_required |= self._declared_auth
        self._validate_config(self.config)
        picks = {"provider": self.provider}
        if declared_base:
            picks["base_url"] = declared_base
        _apply_launch_overrides(picks, self.config)
        self._check(self.config["model"].get("provider"), "model.provider")
        if model is not None:
            self.config["model"]["default"] = model
        self.config.update(fallback_providers=[], fallback_model=None)
        with frozen_runtime_config(self.config):
            self.runtime = resolve_runtime_provider(requested=self.provider, target_model=model,
                explicit_api_key=self.declared_key, explicit_base_url=declared_base)
        if self._api_mode:
            self.runtime["api_mode"] = self._api_mode
            self.config["model"]["api_mode"] = self._api_mode
        self._check(self.runtime["provider"], "resolved provider")
        self._remember(self.runtime.get("api_key"))
        self.agent_kwargs = {key: self.runtime.get(key) for key in ("provider", "api_key", "base_url", "api_mode", "credential_pool")}
        self.agent_kwargs["fallback_model"] = {}
        if self._declared_auth:
            self.agent_kwargs["credential_pool"] = None
        self._install_agent()
        self._install_auxiliary()
        self._install_delegation()

    @staticmethod
    def refuse(error):
        raise _RequestRefused(AuthError(str(error)))

    @staticmethod
    def error(message):
        return AuthError("Hermes " + message)

    def _install_provider(self, source, native):
        if self.product_provider not in {"github", "bedrock"}:
            return None
        import importlib.util
        from pathlib import Path
        path = Path(__file__).with_name("native_providers.py")
        spec = importlib.util.spec_from_file_location("_zeroshot_hermes_providers", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.install(self, source, native.get("directory"), native.get("aws") or {})

    def _remember(self, value):
        if isinstance(value, str) and value:
            self.redactions.add(value)
            from agent.redact import register_vault_redaction_value
            register_vault_redaction_value(value)

    def sanitize_owned_error(self, message):
        from agent.redact import redact_sensitive_text
        message = str(message)
        for value in sorted(self.redactions, key=len, reverse=True):
            message = message.replace(value, "[REDACTED]")
        return redact_sensitive_text(message, force=True)

    def _normalize(self, provider):
        if not provider or str(provider).strip().lower() in {"auto", "main"}:
            return self.provider
        from hermes_cli.runtime_provider_custom import expand_direct_api_alias
        normalized = self._auxiliary._normalize_aux_provider(provider)
        expanded, _ = expand_direct_api_alias(normalized, None)
        if normalized == self.product_provider and expanded == "custom":
            return self.provider
        return normalized

    def _check(self, provider, field):
        if self._normalize(provider) != self.provider:
            raise AuthError(f"Hermes {field} conflicts with the selected harness provider")

    def _validate_config(self, config):
        for task, settings in (config.get("auxiliary") or {}).items():
            if isinstance(settings, dict):
                self._validate_auxiliary(task, settings)
        self._check((config.get("delegation") or {}).get("provider"), "delegation.provider")

    def _runtime(self, agent=None, supplied=None):
        runtime = {**self.runtime, "model": self._model}
        # Native turn context carries a child's current key and native OAuth rotations.
        native = self._auxiliary._normalize_main_runtime(supplied)
        if native:
            self._check(native.get("provider"), "auxiliary provider")
            runtime.update(native)
        agent = agent or self._main_agent
        if agent is not None and not native:
            runtime.update({key: getattr(agent, key, runtime.get(key))
                            for key in ("provider", "model", "api_key", "base_url", "api_mode")})
        runtime["provider"] = self.provider
        if self._api_mode:
            runtime["api_mode"] = self._api_mode
        if self.declared_key:
            runtime["api_key"] = self.declared_key
        if self._pin_endpoint:
            runtime["base_url"] = self.runtime["base_url"]
        self._remember(runtime.get("api_key"))
        return runtime

    def _scoped_endpoint(self, endpoint):
        from agent.auxiliary_task_config import _SESSION_AUXILIARY
        from hermes_cli.fallback_config import get_fallback_chain
        section = _SESSION_AUXILIARY.get() or {}
        settings = (value for value in section.values() if isinstance(value, dict))
        routes = [route for value in settings for route in
                  [value, *get_fallback_chain({"fallback_providers": value.get("fallback_chain")})]]
        return any(route.get("base_url") == endpoint and self._normalize(route.get("provider")) == self.provider
                   for route in routes)

    def _client_kwargs(self, kwargs):
        kwargs = dict(kwargs)
        if self.declared_key:
            kwargs["api_key"] = self.declared_key
        if self._pin_endpoint:
            kwargs["base_url"] = self.runtime["base_url"]
        if self._declared_auth:
            kwargs["default_headers"] = self._headers(kwargs.get("default_headers"))
        if self._provider_auth:
            kwargs = self._provider_auth.client_kwargs(kwargs)
        self._remember(kwargs.get("api_key"))
        return kwargs

    @staticmethod
    def _headers(headers):
        return {name: value for name, value in (headers or {}).items()
                if not _AUTH_HEADER.search(name) and name.lower() != "x-provider-api-keys"}

    def _guard_client(self, client):
        if self._native_probe(client):
            return client
        if self._provider_auth and self._provider_auth.guard_client(client):
            return client
        if hasattr(client, "_real_client"):
            self._guard_client(client._real_client)
            return client
        self._remember(getattr(client, "api_key", None))
        self._remember(getattr(client, "auth_token", None))
        if not self._guard_required:
            return client
        self._require_model_client(client)
        # The SDK has already merged native beta/identity/metadata headers. Remove
        # inherited authentication only; a later per-request override is refused.
        if self._declared_auth:
            client._custom_headers = self._headers(client._custom_headers)
        transport = client._client
        expected = self._client_authentication(client)
        if getattr(transport, "_zeroshot_model_authority", None) is self:
            self._protect_sdk(client)
            return client
        import httpx
        endpoint = urlsplit(str(httpx.URL(self.runtime["base_url"]))) if self.declared_base else None
        if isinstance(transport, httpx.AsyncClient):
            async def guard(request):
                self._request_guard(endpoint, expected, request)
        else:
            guard = partial(self._request_guard, endpoint, expected)
        transport.event_hooks.setdefault("request", []).append(guard)
        transport._zeroshot_model_authority = self
        self._protect_sdk(client)
        return client

    def _native_probe(self, client):
        return self._auxiliary._aux_probe_active() and isinstance(client, self._auxiliary._AuxProbeClientStub)

    def _client_authentication(self, client):
        expected = {name.lower(): value for name, value in client.auth_headers.items()
                    if isinstance(value, str) and _AUTH_HEADER.search(name)}
        if self.declared_key and (not expected or any(value not in (
                self.declared_key, "Bearer " + self.declared_key) for value in expected.values())):
            raise AuthError("Hermes model client overrides declared authentication")
        if self._provider_auth:
            self._provider_auth.authentication(expected)
        return expected

    @staticmethod
    def _require_model_client(client):
        fields = ("_custom_headers", "_client", "auth_headers", "base_url", "request", "copy", "with_options")
        if not all(hasattr(client, field) for field in fields) or not hasattr(client._client, "event_hooks"):
            raise AuthError("Hermes native model client lacks required authentication guard support")

    def _request_guard(self, endpoint, expected, request):
        try:
            self._guard_request(endpoint, expected, request)
        except AuthError as error:
            raise _RequestRefused(error) from None

    def _protect_sdk(self, client):
        if getattr(client, "_zeroshot_sdk_authority", None) is self:
            return
        import httpx
        native_request = client.request
        if isinstance(client._client, httpx.AsyncClient):
            async def request(*args, **kwargs):
                try:
                    return await native_request(*args, **kwargs)
                except _RequestRefused as refused:
                    raise refused.error from None
        else:
            def request(*args, **kwargs):
                try:
                    return native_request(*args, **kwargs)
                except _RequestRefused as refused:
                    raise refused.error from None
        client.request = request
        client.copy = partial(self._copy_client, client.copy)
        client.with_options = partial(self._copy_client, client.with_options)
        client._zeroshot_sdk_authority = self

    def _copy_client(self, native_copy, *args, **kwargs):
        return self._guard_client(native_copy(*args, **kwargs))

    def _guard_request(self, endpoint, expected, request):
        if endpoint is not None:
            self._guard_endpoint(endpoint, request)
        if self._declared_auth:
            received = {name.lower(): value for name, value in request.headers.items()
                        if _AUTH_HEADER.search(name) or name.lower() == "x-provider-api-keys"}
            if self._provider_auth:
                self._provider_auth.authentication(received)
                self._provider_auth.parameters(request)
                return
            if received != expected:
                names = self._conflicting_headers(expected, received)
                raise AuthError(f"Hermes model request overrides declared authentication (headers: {names})")
            self._guard_parameters(request)

    def _conflicting_headers(self, expected, received):
        names = sorted(name for name in expected.keys() | received.keys()
                       if expected.get(name) != received.get(name))
        return self.sanitize_owned_error(", ".join(names))[:256]

    @staticmethod
    def _guard_endpoint(endpoint, request):
        actual = urlsplit(str(request.url))
        prefix = endpoint.path.rstrip("/")
        if (actual.scheme, actual.netloc) != (endpoint.scheme, endpoint.netloc) or not (
                actual.path == prefix or actual.path.startswith(prefix + "/")):
            raise AuthError("Hermes model request conflicts with the selected endpoint")

    def _guard_parameters(self, request):
        parameters = list(request.url.params.multi_items())
        if request.content:
            try:
                body = json.loads(request.content)
            except (ValueError, UnicodeDecodeError):
                body = None
            if isinstance(body, dict):
                parameters.extend(body.items())
        for name, value in parameters:
            if name.lower().replace("-", "_") in _AUTH_PARAMETER and value not in (
                    None, "", self.declared_key, "Bearer " + self.declared_key):
                raise AuthError("Hermes model request carries conflicting authentication")

    def _install_agent(self):
        from hermes_cli import plugins
        # Native model_tools discovers plugins during import. Publish ownership
        # first, then let native build_policy perform ordinary discovery.
        discover = plugins.discover_plugins
        plugins.discover_plugins = lambda *args, **kwargs: None
        try:
            import run_agent
        finally:
            plugins.discover_plugins = discover
        authority = self
        native_agent = run_agent.AIAgent

        class OwnedAIAgent(native_agent):
            def __init__(self, *args, **kwargs):
                authority._check(kwargs.get("provider"), "agent provider")
                kwargs["provider"] = authority.provider
                kwargs["fallback_model"] = {}
                if authority._api_mode:
                    kwargs["api_mode"] = authority._api_mode
                if authority.declared_key:
                    kwargs["api_key"] = authority.declared_key
                if authority._declared_auth:
                    kwargs["credential_pool"] = None
                if authority._provider_auth:
                    kwargs = authority._provider_auth.agent_kwargs(kwargs)
                if authority._pin_endpoint:
                    kwargs["base_url"] = authority.runtime["base_url"]
                super().__init__(*args, **kwargs)
                if authority._main_agent is None:
                    authority._main_agent = self
                if hasattr(self, "_anthropic_client") and self._anthropic_client is not None:
                    authority._guard_client(self._anthropic_client)

            def _create_openai_client(self, kwargs, *, reason, shared):
                authority._check(self.provider, "agent provider")
                return authority._guard_client(super()._create_openai_client(
                    authority._client_kwargs(kwargs), reason=reason, shared=shared))

            def _resolve_env_credentials(self):
                result = super()._resolve_env_credentials()
                if result is not None and authority.declared_base:
                    result = (result[0], authority.runtime["base_url"], result[2])
                return result

            def _try_refresh_env_client_credentials(self):
                if authority.declared_key:
                    return False
                result = super()._try_refresh_env_client_credentials()
                authority._remember(self.api_key)
                return result

            def _try_refresh_anthropic_client_credentials(self):
                if authority.declared_key:
                    return False
                result = super()._try_refresh_anthropic_client_credentials()
                authority._remember(self.api_key)
                return result

            def _build_anthropic_client_for_key(self, key):
                return authority._guard_client(super()._build_anthropic_client_for_key(key))

            def _build_direct_anthropic_client(self, token, base_url):
                return authority._guard_client(super()._build_direct_anthropic_client(token, base_url))

        run_agent.AIAgent = self.agent_class = OwnedAIAgent
    def _install_auxiliary(self):
        from agent import anthropic_adapter
        from agent.credential_pool import CredentialPool
        authority, auxiliary = self, self._auxiliary
        resolve, pool, refresh = auxiliary.resolve_provider_client, auxiliary.load_pool, auxiliary._refresh_provider_credentials
        openai_client = auxiliary._create_openai_client

        resolve_signature = signature(resolve)

        def resolve_client(*args, **kwargs):
            bound = resolve_signature.bind(*args, **kwargs)
            bound.apply_defaults()
            params = bound.arguments
            runtime = authority._runtime(supplied=params["main_runtime"])
            # Native complete task routes use the technical "custom" identity.
            if params["provider"] == "custom":
                same_route = params["explicit_base_url"] == runtime["base_url"]
                scoped = authority._scoped_endpoint(params["explicit_base_url"])
                if (same_route or scoped) and (not authority.declared_key or params["explicit_api_key"] == authority.declared_key):
                    params["provider"] = authority.provider
            authority._check(params["provider"], "auxiliary provider")
            if str(params["provider"]).lower() not in {"auto", "main", "none"}:
                params["provider"] = authority.provider
            if authority.declared_key:
                params["explicit_api_key"] = authority.declared_key
            if authority._pin_endpoint:
                params["explicit_base_url"] = runtime["base_url"]
            if authority._api_mode:
                params["api_mode"] = authority._api_mode
            params["main_runtime"] = runtime
            return resolve(**params)

        def selected_pool(provider):
            authority._check(provider, "auxiliary provider")
            return CredentialPool(provider, []) if authority._declared_auth else pool(provider)

        def selected_refresh(provider, **kwargs):
            if provider != "custom":
                authority._check(provider, "auxiliary provider")
            return False if authority.declared_key else refresh(provider, **kwargs)

        auxiliary.resolve_provider_client = resolve_client
        auxiliary.load_pool = selected_pool
        auxiliary._refresh_provider_credentials = selected_refresh
        auxiliary._create_openai_client = lambda **kwargs: authority._guard_client(openai_client(**authority._client_kwargs(kwargs)))
        native_anthropic = anthropic_adapter.build_anthropic_client

        def anthropic_client(api_key, base_url=None, *args, **kwargs):
            if authority.declared_key:
                api_key = authority.declared_key
            if authority._pin_endpoint:
                base_url = authority.runtime["base_url"]
            return authority._guard_client(native_anthropic(api_key, base_url, *args, **kwargs))

        anthropic_adapter.build_anthropic_client = anthropic_client
        to_async = auxiliary._to_async_client

        def async_client(*args, **kwargs):
            client, model = to_async(*args, **kwargs)
            return authority._guard_client(client), model

        auxiliary._to_async_client = async_client

    def _install_delegation(self):
        from tools import delegate_tool
        authority = self
        credentials, child_pool = delegate_tool._resolve_delegation_credentials, delegate_tool._resolve_child_credential_pool

        def delegation_credentials(config, parent):
            authority._check(config.get("provider"), "delegation.provider")
            bound = dict(config)
            if authority._provider_auth:
                bound = authority._provider_auth.delegation(bound)
            elif authority._guard_required:
                # Native direct-endpoint resolution with no provider lookup keeps
                # its wire/model/settings and never refreshes a stored login.
                bound["provider"] = None
                if authority.declared_key:
                    bound["api_key"] = authority.declared_key
                if authority.declared_base:
                    bound["base_url"] = authority.runtime["base_url"]
            elif bound.get("provider"):
                bound["provider"] = authority.provider
            result = credentials(bound, parent)
            if authority._api_mode:
                result["api_mode"] = authority._api_mode
            if result.get("provider") == "custom":
                result["provider"] = authority.provider
            return result

        def delegation_pool(provider, parent, base_url=None, **kwargs):
            authority._check(provider, "delegated provider")
            return None if authority._declared_auth else child_pool(provider, parent, base_url, **kwargs)

        delegate_tool._resolve_delegation_credentials = delegation_credentials
        delegate_tool._resolve_child_credential_pool = delegation_pool

    def _validate_auxiliary(self, task, settings):
        from hermes_cli.fallback_config import get_fallback_chain
        self._check(settings.get("provider"), f"auxiliary.{task}.provider")
        chain = get_fallback_chain({"fallback_providers": settings.get("fallback_chain")})
        for entry in chain:
            self._check(entry.get("provider"), f"auxiliary.{task}.fallback_chain.provider")
        return {**settings, "fallback_chain": chain}

    def _auxiliary_settings(self, task, settings, runtime):
        model = settings.get("model")
        if not model or str(model).strip().lower() == "auto":
            model = self._auxiliary._main_route_target(runtime, task)[1]
        settings = {**settings, "provider": self.provider, "model": model}
        if self._api_mode:
            settings["api_mode"] = self._api_mode
        if self.declared_key:
            settings["api_key"] = self.declared_key
        if self._pin_endpoint:
            settings["base_url"] = runtime["base_url"]
        return settings

    @contextmanager
    def auxiliary_scope(self, agent=None):
        from agent.auxiliary_task_config import _get_auxiliary_task_config, session_auxiliary_scope
        from hermes_cli.plugins import get_plugin_auxiliary_tasks
        auxiliary = self.config.get("auxiliary") or {}
        tasks = {key for key, value in auxiliary.items() if isinstance(value, dict)}
        tasks.update(entry["key"] for entry in get_plugin_auxiliary_tasks())
        runtime = self._runtime(agent)
        with session_auxiliary_scope(auxiliary):
            effective = {task: self._validate_auxiliary(task, _get_auxiliary_task_config(task))
                         for task in sorted(tasks)}
            bound = {**auxiliary, **{task: self._auxiliary_settings(task, settings, runtime)
                                    for task, settings in effective.items()}}
        with session_auxiliary_scope(bound), self._auxiliary.scoped_runtime_main(runtime):
            yield


def install(provider, declared_key, declared_base, config, **options):
    """Install once in an owned native worker, before native plugin discovery."""
    if provider not in _PROVIDERS:
        raise AuthError("Hermes provider is unsupported")
    return Authority(provider, (declared_key, declared_base), config, **options)
