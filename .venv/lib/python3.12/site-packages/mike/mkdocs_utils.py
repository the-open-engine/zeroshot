import os
import subprocess
import sys
from collections.abc import Iterable, Mapping
from contextlib import contextmanager
from tempfile import NamedTemporaryFile

import mkdocs.config
import mkdocs.plugins
import mkdocs.utils
import yaml
import yaml_env_tag

if sys.version_info < (3, 10):
    import importlib_metadata as metadata
else:
    from importlib import metadata

docs_version_var = 'MIKE_DOCS_VERSION'
default_config_file = ['mkdocs.yml', 'mkdocs.yaml']

try:
    _version = metadata.version('properdocs')
    _program_name = 'properdocs'
    _brand = 'ProperDocs'
    default_config_file = (['properdocs.yml', 'properdocs.yaml'] +
                           default_config_file)
except metadata.PackageNotFoundError:
    _version = metadata.version('mkdocs')
    _program_name = 'mkdocs'
    _brand = 'MkDocs'


class RoundTrippableTag:
    def __init__(self, node):
        self.node = node

    def __repr__(self):
        return repr(self.node)

    @staticmethod
    def constructor(loader, suffix, node):
        return RoundTrippableTag(node)

    @staticmethod
    def representer(dumper, data):
        return data.node


class RoundTripLoader(yaml.Loader):
    pass


# We need to expand environment variables in our round trip loader (making it
# less of a "round trip"), or else `INHERIT: !ENV ...` will fail when injecting
# the mike plugin. MkDocs really doesn't make this easy on us...
yaml.add_constructor('!ENV', yaml_env_tag.construct_env_tag,
                     Loader=RoundTripLoader)

yaml.add_multi_constructor('!', RoundTrippableTag.constructor,
                           Loader=RoundTripLoader)
yaml.add_multi_representer(RoundTrippableTag, RoundTrippableTag.representer)


def _open_config(config_file=None):
    if config_file is None:
        config_file = default_config_file
    elif not isinstance(config_file, Iterable) or isinstance(config_file, str):
        config_file = [config_file]

    exc = None
    for i in config_file:
        try:
            return open(i, 'rb')
        except FileNotFoundError as e:
            if not exc:
                exc = e
    raise exc


def load_config(config_file=None, **kwargs):
    with _open_config(config_file) as f:
        cfg = mkdocs.config.load_config(f, **kwargs)

        if 'startup' in mkdocs.plugins.EVENTS:
            cfg['plugins'].run_event('startup', command='mike', dirty=False)
        cfg = cfg['plugins'].run_event('config', cfg)
        if 'shutdown' in mkdocs.plugins.EVENTS:
            cfg['plugins'].run_event('shutdown')

        return cfg


@contextmanager
def inject_plugin(config_file):
    with _open_config(config_file) as f:
        config_file = f.name
        config = mkdocs.utils.yaml_load(f, loader=RoundTripLoader)

    plugins = config.setdefault('plugins', ['search'])
    for i in plugins:
        if ( (isinstance(i, str) and i == 'mike') or
             (isinstance(i, dict) and 'mike' in i) ):
            yield config_file
            return

    if isinstance(plugins, Mapping):
        config['plugins'] = {'mike': {}}
        config['plugins'].update(plugins)
    else:
        plugins.insert(0, 'mike')

    with NamedTemporaryFile(mode='w', dir=os.path.dirname(config_file),
                            prefix='mike-mkdocs', suffix='.yml',
                            delete=False) as f:
        yaml.dump(config, f, sort_keys=False)

    try:
        yield f.name
    finally:
        os.remove(f.name)


def build(config_file, version, *, quiet=False, output=None):
    command = (
        [_program_name] +
        (['--quiet'] if quiet else []) +
        ['build', '--clean'] +
        (['--config-file', config_file] if config_file else [])
    )

    env = os.environ.copy()
    env[docs_version_var] = version

    subprocess.run(command, check=True, env=env, stdout=output, stderr=output)


def version_info():
    return '{} {}'.format(_brand, _version)
