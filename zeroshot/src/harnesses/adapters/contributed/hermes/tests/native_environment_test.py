"""Private native-context fallback contract; no Hermes installation or provider needed."""
import importlib.util
import os
from pathlib import Path
import sys
from types import ModuleType
import unittest
from unittest.mock import patch


class NativeEnvironmentTest(unittest.TestCase):
    def setUp(self):
        self.routed = False
        self.bound = None
        self.multiplex = False
        self.registered, self.loads = [], []
        self.secrets = ModuleType("agent.secret_scope")
        self.redact = ModuleType("agent.redact")
        self.config = ModuleType("hermes_cli.config")
        self.loader = ModuleType("hermes_cli.env_loader")
        self.secrets._environ_or = lambda name, default: os.environ.get(name, default)
        self.secrets.serves_routed_profile = lambda: self.routed
        self.secrets.current_secret_scope = lambda: self.bound
        self.secrets.is_multiplex_active = lambda: self.multiplex
        self.secrets.UnscopedSecretError = RuntimeError
        self.secrets.get_secret = self.native_secret
        self.redact.register_vault_redaction_value = self.registered.append
        self.config._env_ref_lookup = lambda name: os.environ.get(name)
        self.loader._dotenv_lookup_env = lambda load_pass: dict(os.environ)
        self.loader.load_hermes_dotenv = lambda **kwargs: self.loads.append(kwargs)
        agent, cli = ModuleType("agent"), ModuleType("hermes_cli")
        agent.secret_scope, agent.redact = self.secrets, self.redact
        cli.config, cli.env_loader = self.config, self.loader
        modules = {"agent": agent, "agent.secret_scope": self.secrets, "agent.redact": self.redact,
                   "hermes_cli": cli, "hermes_cli.config": self.config,
                   "hermes_cli.env_loader": self.loader}
        self.patch_modules = patch.dict(sys.modules, modules)
        self.patch_modules.start()
        self.addCleanup(self.patch_modules.stop)
        self.patch_environment = patch.dict(os.environ, {}, clear=True)
        self.patch_environment.start()
        self.addCleanup(self.patch_environment.stop)
        source = Path(__file__).resolve().parents[1] / "native_environment.py"
        spec = importlib.util.spec_from_file_location("hermes_environment_subject", source)
        self.environment = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.environment)

    def native_secret(self, name, default=None):
        if self.bound is not None:
            value = self.bound.get(name)
            if value is not None:
                return value
            if self.routed:
                return default
        if self.multiplex and self.bound is None:
            raise self.secrets.UnscopedSecretError(name)
        return self.secrets._environ_or(name, default)

    def test_native_values_and_empty_values_win_over_capture(self):
        os.environ.update({"NATIVE_TOKEN": "native-authority", "NATIVE_EMPTY": ""})
        self.environment.install({"NATIVE_TOKEN": "captured", "NATIVE_EMPTY": "captured"})
        self.assertEqual(self.secrets._environ_or("NATIVE_TOKEN", None), "native-authority")
        self.assertEqual(self.config._env_ref_lookup("NATIVE_EMPTY"), "")
        self.assertEqual(self.registered, [])

    def test_mocked_windows_capture_is_case_insensitive_and_native_still_wins(self):
        os.environ["NATIVE_TOKEN"] = "native-authority"
        self.secrets._environ_or = lambda name, default: os.environ.get(name.upper(), default)
        self.config._env_ref_lookup = lambda name: os.environ.get(name.upper())
        with patch.object(self.environment.os, "name", "nt"):
            self.environment.install({"mCp_ToKeN": "private-mcp-value", "nAtIvE_ToKeN": "captured"})
            self.assertEqual(self.secrets._environ_or("MCP_token", None), "private-mcp-value")
            self.assertEqual(self.config._env_ref_lookup("mcp_TOKEN"), "private-mcp-value")
            self.assertEqual(self.config._env_ref_lookup("native_token"), "native-authority")
        self.assertNotIn("MCP_TOKEN", os.environ)

    def test_unix_capture_remains_case_sensitive(self):
        with patch.object(self.environment.os, "name", "posix"):
            self.environment.install({"Mixed_Token": "case-sensitive-value"})
            self.assertEqual(self.config._env_ref_lookup("Mixed_Token"), "case-sensitive-value")
            self.assertIsNone(self.config._env_ref_lookup("MIXED_TOKEN"))

    def test_only_requested_capture_is_returned_and_registered(self):
        self.environment.install({"MCP_TOKEN": "private-mcp-value", "UNRELATED": "unrelated-value"})
        self.assertEqual(self.secrets._environ_or("MCP_TOKEN", None), "private-mcp-value")
        self.assertEqual(self.config._env_ref_lookup("MCP_TOKEN"), "private-mcp-value")
        self.assertEqual(self.secrets._environ_or("MISSING", "default"), "default")
        self.assertEqual(self.registered, ["private-mcp-value", "private-mcp-value"])
        self.assertNotIn("MCP_TOKEN", os.environ)
        self.assertNotIn("UNRELATED", os.environ)

    def test_declared_fields_override_native_dotenv_after_every_reload(self):
        def reload(**kwargs):
            self.loads.append(kwargs)
            os.environ.update({"MCP_TOKEN": "native-file-value", "AMBIENT_TOKEN": "native-authority"})
        self.loader.load_hermes_dotenv = reload
        self.environment.install({"AMBIENT_TOKEN": "captured"}, {"MCP_TOKEN": "explicit-authority"})
        self.assertEqual(os.environ["MCP_TOKEN"], "explicit-authority")
        self.loader.load_hermes_dotenv()
        self.assertEqual(os.environ["MCP_TOKEN"], "explicit-authority")
        self.assertEqual(self.secrets.get_secret("AMBIENT_TOKEN"), "native-authority")
        lookup = self.loader._dotenv_lookup_env(None)
        self.assertEqual(lookup["MCP_TOKEN"], "explicit-authority")

    def test_explicit_named_values_override_own_profile_secret_scope(self):
        self.bound = {"MCP_TOKEN": "native-scoped-value", "OPENAI_API_KEY": "native-scoped-key"}
        self.environment.install({}, {"MCP_TOKEN": "explicit-authority"}, {"OPENAI_API_KEY": "private-key"})
        self.assertEqual(self.secrets.get_secret("MCP_TOKEN"), "explicit-authority")
        self.assertEqual(self.secrets.get_secret("OPENAI_API_KEY"), "private-key")
        self.assertEqual(self.config._env_ref_lookup("OPENAI_API_KEY"), "private-key")
        self.assertNotIn("OPENAI_API_KEY", os.environ)

    def test_private_provider_values_are_not_exported(self):
        self.environment.install({}, {"MCP_TOKEN": "explicit-authority"}, {"OPENAI_API_KEY": "private-key"})
        self.assertEqual(os.environ["MCP_TOKEN"], "explicit-authority")
        self.assertNotIn("OPENAI_API_KEY", os.environ)
        self.assertEqual(self.config._env_ref_lookup("OPENAI_API_KEY"), "private-key")
        self.assertEqual(self.loader._dotenv_lookup_env(None)["OPENAI_API_KEY"], "private-key")

    def test_foreign_profile_cannot_read_declared_or_private_launch_authority(self):
        self.environment.install({}, {"MCP_TOKEN": "launch-explicit"}, {"OPENAI_API_KEY": "launch-private"})
        self.routed = True
        self.assertIsNone(self.secrets.get_secret("MCP_TOKEN"))
        self.assertIsNone(self.config._env_ref_lookup("OPENAI_API_KEY"))
        self.bound = {"MCP_TOKEN": "foreign-own-value", "OPENAI_API_KEY": "foreign-own-key"}
        self.assertEqual(self.secrets.get_secret("MCP_TOKEN"), "foreign-own-value")
        self.assertEqual(self.config._env_ref_lookup("OPENAI_API_KEY"), "foreign-own-key")

    def test_foreign_explicit_name_keeps_native_unscoped_error(self):
        self.environment.install({}, {"MCP_TOKEN": "launch-explicit"})
        self.routed, self.multiplex = True, True
        with self.assertRaises(self.secrets.UnscopedSecretError):
            self.secrets.get_secret("MCP_TOKEN")

    def test_dotenv_interpolation_gets_private_fallback_without_export(self):
        os.environ["NATIVE_TOKEN"] = "native"
        self.environment.install({"NATIVE_TOKEN": "captured", "EXPORTED_CUSTOM": "private"})
        lookup = self.loader._dotenv_lookup_env(None)
        self.assertEqual(lookup["NATIVE_TOKEN"], "native")
        self.assertEqual(lookup["EXPORTED_CUSTOM"], "private")
        self.assertNotIn("EXPORTED_CUSTOM", os.environ)
        self.assertEqual(len(self.loads), 1)

    def test_foreign_profile_does_not_inherit_captured_launch_values(self):
        self.environment.install({"MCP_TOKEN": "launch-profile-value"})
        self.routed = True
        self.assertIsNone(self.config._env_ref_lookup("MCP_TOKEN"))
        self.assertEqual(self.secrets._environ_or("MCP_TOKEN", "missing"), "missing")
        self.assertNotIn("MCP_TOKEN", self.loader._dotenv_lookup_env(None))
        self.assertEqual(self.registered, [])

    def test_native_fail_closed_error_propagates(self):
        def denied(name):
            raise RuntimeError("native unscoped profile")
        self.config._env_ref_lookup = denied
        self.environment.install({"MCP_TOKEN": "launch-profile-value"})
        with self.assertRaisesRegex(RuntimeError, "native unscoped profile"):
            self.config._env_ref_lookup("MCP_TOKEN")

    def test_reinstall_updates_capture_without_nested_wrappers_or_reload(self):
        self.environment.install({"MCP_TOKEN": "first"})
        self.environment.install({"MCP_TOKEN": "second"})
        self.assertEqual(self.config._env_ref_lookup("MCP_TOKEN"), "second")
        self.assertEqual(self.registered, ["second"])
        self.assertEqual(len(self.loads), 1)

    def test_empty_hosted_context_leaves_native_functions_unchanged(self):
        original = self.secrets._environ_or
        self.environment.install({})
        self.assertIs(self.secrets._environ_or, original)
        self.assertEqual(self.loads, [])

    def test_native_reload_registers_only_changed_dotenv_outputs(self):
        def reload(**kwargs):
            self.loads.append(kwargs)
            os.environ["DERIVED_TOKEN"] = self.loader._dotenv_lookup_env(None)["EXPORTED_CUSTOM"]
        self.loader.load_hermes_dotenv = reload
        self.environment.install({"EXPORTED_CUSTOM": "native-derived-private", "UNRELATED": "unrelated"})
        self.assertEqual(self.registered, ["native-derived-private"])
        self.assertNotIn("UNRELATED", os.environ)


if __name__ == "__main__":
    unittest.main()
