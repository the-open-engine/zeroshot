"""Installed distribution version."""

from importlib.metadata import PackageNotFoundError, version

try:
    __version__ = version("the-open-engine-zeroshot")
except PackageNotFoundError:
    __version__ = "0.0.0.dev0"
