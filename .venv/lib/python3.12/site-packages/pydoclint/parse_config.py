from __future__ import annotations

import logging
import sys
from pathlib import Path
from typing import Any, Sequence

import click

if sys.version_info >= (3, 11):
    import tomllib
else:
    import tomli as tomllib


def injectDefaultOptionsFromUserSpecifiedTomlFilePath(
        ctx: click.Context,
        param: click.Parameter,
        value: str | None,
) -> str | None:
    """
    Inject default objects from user-specified .toml file path.

    Parameters
    ----------
    ctx : click.Context
        The "click" context
    param : click.Parameter
        The "click" parameter; not used in this function; just a placeholder
    value : str | None
        The full path of the .toml file. (It needs to be named ``value``
        so that ``click`` can correctly use it as a callback function.)

    Returns
    -------
    str | None
        The full path of the .toml file
    """
    if not value:
        return None

    logging.info(f'Loading config from user-specified .toml file: {value}')
    config = parseOneTomlFile(tomlFilename=Path(value))
    updateCtxDefaultMap(ctx=ctx, config=config)
    return value


def parseToml(paths: Sequence[str] | None) -> dict[str, Any]:
    """Parse the pyproject.toml located in the common parent of ``paths``"""
    if paths is None:
        return {}

    commonParent: Path = findCommonParentFolder(paths)
    tomlFilename = commonParent / Path('pyproject.toml')
    logging.info(
        f'Loading config from inferred .toml file path: {tomlFilename}'
    )
    return parseOneTomlFile(tomlFilename)


def parseOneTomlFile(tomlFilename: Path) -> dict[str, Any]:
    """Parse a .toml file"""
    if not tomlFilename.exists():
        logging.info(f'File "{tomlFilename}" does not exist; nothing to load.')
        return {}

    try:
        with open(tomlFilename, 'rb') as fp:
            rawConfig = tomllib.load(fp)

        pydoclintSection = rawConfig['tool']['pydoclint']
        finalConfig = {
            k.replace('-', '_'): v for k, v in pydoclintSection.items()
        }
    except Exception:
        finalConfig = {}

    if len(finalConfig) > 0:
        logging.info(f'Found options defined in {tomlFilename}:')
        logging.info(finalConfig)
    else:
        logging.info(f'No config found in {tomlFilename}.')

    return finalConfig


def findCommonParentFolder(
        paths: Sequence[str],
        makeAbsolute: bool = True,  # allow makeAbsolute=False just for testing
) -> Path:
    """Find the common parent folder of the given ``paths``"""
    paths_: Sequence[Path] = [Path(path) for path in paths]

    common_parent = paths_[0]
    for path in paths_[1:]:
        if len(common_parent.parts) > len(path.parts):
            common_parent, path = path, common_parent

        for i, part in enumerate(common_parent.parts):
            if part != path.parts[i]:
                common_parent = Path(*common_parent.parts[:i])
                break

    if makeAbsolute:
        return common_parent.absolute()

    return common_parent


def updateCtxDefaultMap(ctx: click.Context, config: dict[str, Any]) -> None:
    """Update the ``click`` context default map with the provided ``config``"""
    default_map: dict[str, Any] = {}
    if ctx.default_map:
        default_map.update(ctx.default_map)

    default_map.update(config)
    ctx.default_map = default_map
