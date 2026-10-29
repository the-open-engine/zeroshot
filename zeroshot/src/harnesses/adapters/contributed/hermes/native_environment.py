"""Missing-only shell context for native configuration and secret lookups."""
import os
from pathlib import Path
import sys

_captured = {}
_declared = {}
_private = {}
_installed = False


def _key(name):
    return name.upper() if os.name == "nt" else name


def _normalized(values):
    return {_key(name): value for name, value in (values or {}).items()}


def install(captured, declared=None, private=None):
    """Keep captured values private; only native consumers request individual names."""
    global _captured, _declared, _private, _installed
    _captured, _declared, _private = map(_normalized, (captured, declared, private))
    if not any((_captured, _declared, _private)) or _installed:
        return
    from agent import redact, secret_scope
    from hermes_cli import config, env_loader
    environ = secret_scope._environ_or
    secret = secret_scope.get_secret
    config_lookup = config._env_ref_lookup
    dotenv_lookup = env_loader._dotenv_lookup_env
    reload = env_loader.load_hermes_dotenv

    def remembered(value):
        redact.register_vault_redaction_value(value)
        return value

    def lookup(name, native, default=None):
        own = not secret_scope.serves_routed_profile()
        key = _key(name)
        explicit = _private.get(key, _declared.get(key))
        if explicit is not None:
            if own:
                return remembered(explicit)
            bound = secret_scope.current_secret_scope()
            if bound is None and secret_scope.is_multiplex_active():
                raise secret_scope.UnscopedSecretError(name)
            return bound.get(name, default) if bound is not None else default
        value = native()
        if value is not None or not own:
            return default if value is None else value
        captured = _captured.get(key)
        return default if captured is None else remembered(captured)

    def environ_or(name, default):
        return lookup(name, lambda: environ(name, None), default)

    def get_secret(name, default=None):
        return lookup(name, lambda: secret(name, None), default)

    def env_ref_lookup(name):
        return lookup(name, lambda: config_lookup(name))

    def dotenv_lookup_env(load_pass):
        native = dotenv_lookup(load_pass)
        if secret_scope.serves_routed_profile():
            return native
        return {**_captured, **native, **_declared, **_private}

    def load_hermes_dotenv(*args, **kwargs):
        before = dict(os.environ)
        result = reload(*args, **kwargs)
        if not secret_scope.serves_routed_profile():
            os.environ.update(_declared)
        for name, value in os.environ.items():
            if before.get(name) != value:
                remembered(value)
        return result

    secret_scope._environ_or = environ_or
    secret_scope.get_secret = get_secret
    config._env_ref_lookup = env_ref_lookup
    env_loader._dotenv_lookup_env = dotenv_lookup_env
    env_loader.load_hermes_dotenv = load_hermes_dotenv
    _installed = True
    # Canonical startup may have expanded a profile .env before the private init arrived.
    # Native reload peels its earlier published values back to their original baselines.
    project_root = getattr(sys.modules.get("hermes_cli.main"), "PROJECT_ROOT", None)
    env_loader.load_hermes_dotenv(
        project_env=Path(project_root) / ".env" if project_root is not None else None)
