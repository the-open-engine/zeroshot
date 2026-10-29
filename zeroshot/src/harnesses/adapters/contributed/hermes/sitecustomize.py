"""Private entry hook for the selected Hermes executable; metadata argv fails closed."""
import importlib.abc
import importlib.machinery
import importlib.util
import os
from pathlib import Path
import sys

_PRIVATE = str(Path(__file__).resolve().parent)
_RUNTIME = str(Path(_PRIVATE) / "native_runtime.py")

# Python selected this private startup module. First execute the user's original startup
# module with its original identity and search path, as ordinary native startup would.
_original_path = [entry for entry in sys.path if os.path.realpath(entry or os.getcwd()) != _PRIVATE]
_original = importlib.machinery.PathFinder.find_spec("sitecustomize", _original_path)
if _original is not None:
    _module = importlib.util.module_from_spec(_original)
    sys.modules["sitecustomize"] = _module
    _original.loader.exec_module(_module)


class _EntryLoader(importlib.abc.Loader):
    def __init__(self, original):
        self.original = original

    def create_module(self, spec):
        return self.original.create_module(spec)

    def exec_module(self, module):
        # Native bypass policies must bind before config/env/plugin imports. Only an
        # explicit user opt-out takes this native path; ordinary startup is untouched.
        truthy = lambda name: os.getenv(name, "").strip().lower() in {"1", "true", "yes", "on"}
        safe, ignored = truthy("HERMES_SAFE_MODE"), truthy("HERMES_IGNORE_USER_CONFIG")
        if safe or ignored:
            import hermes_bootstrap
            from agent.safe_worker_policy import _bind_safe_worker_policy
            _bind_safe_worker_policy(safe_mode=safe, ignore_user_config=True, config={})
            os.environ["HERMES_IGNORE_USER_CONFIG"] = "1"
        self.original.exec_module(module)
        def owned_main():
            os.environ.pop("ZEROSHOT_HERMES_BRIDGE", None)
            pythonpath = os.environ.get("PYTHONPATH", "").split(os.pathsep)
            retained = [entry for entry in pythonpath if os.path.realpath(entry or os.getcwd()) != _PRIVATE]
            if retained:
                os.environ["PYTHONPATH"] = os.pathsep.join(retained)
            else:
                os.environ.pop("PYTHONPATH", None)
            sys.path[:] = [entry for entry in sys.path if os.path.realpath(entry or os.getcwd()) != _PRIVATE]
            spec = importlib.util.spec_from_file_location("_zeroshot_hermes_runtime", _RUNTIME)
            runtime = importlib.util.module_from_spec(spec)
            sys.modules[spec.name] = runtime
            spec.loader.exec_module(runtime)
            return runtime.main()
        module.main = owned_main


class _EntryFinder(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path, target=None):
        if fullname != "hermes_cli.main":
            return None
        sys.meta_path.remove(self)
        spec = importlib.util.find_spec(fullname)
        if spec is not None:
            spec.loader = _EntryLoader(spec.loader)
        return spec


if os.environ.get("ZEROSHOT_HERMES_BRIDGE") == _RUNTIME:
    sys.meta_path.insert(0, _EntryFinder())
