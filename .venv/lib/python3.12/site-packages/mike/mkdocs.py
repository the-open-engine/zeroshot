import os
import re
import subprocess
from ruamel import yaml


def site_dir(config_file):
    with open(config_file) as f:
        config = yaml.load(f, Loader=yaml.Loader)
        site = config.get('site_dir', 'site')
    return os.path.join(os.path.dirname(config_file), site)


def build(config_file, verbose=True):
    command = (
        ['mkdocs', 'build', '--clean'] +
        (['--config-file', config_file] if config_file else [])
    )

    if verbose:
        subprocess.check_call(command)
    else:
        subprocess.check_output(command, stderr=subprocess.STDOUT)


def version():
    output = subprocess.check_output(
        ['mkdocs', '--version'], universal_newlines=True
    ).rstrip()
    m = re.search('^mkdocs, version (\\S*)', output)
    return m.group(1)
