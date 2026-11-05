#!/usr/bin/env python
# -*- coding: utf-8 -*-

"""
    nodeenv
    ~~~~~~~
    Node.js virtual environment

    :copyright: (c) 2014 by Eugene Kalinin
    :license: BSD, see LICENSE for more details.
"""

import base64
import concurrent.futures
import contextlib
import io
import json
import sys
import os
import re
import ssl
import stat
import logging
import operator
import argparse
import subprocess
import tarfile
import time
from shlex import quote as _quote
import platform
import zipfile
import shutil
import socket
import sysconfig
import glob

from configparser import ConfigParser, InterpolationSyntaxError
from http.client import IncompleteRead
import urllib.request as urllib2
from urllib.parse import unquote, urlsplit, urlunsplit

nodeenv_version = '1.12.0'

join = os.path.join
abspath = os.path.abspath
src_base_url = None

is_WIN = platform.system() == 'Windows'
is_CYGWIN = platform.system().startswith(('CYGWIN', 'MSYS'))

ignore_ssl_certs = False
# SSL context backed by the certifi bundle, built once by main()
# when --with-certifi is given and certifi is importable
certifi_context = None
# Authorization header for src_base_url, built by main() from the
# user:password@ part of --mirror, which is cut off src_base_url
src_auth = None
# Seconds to wait for a server to answer or to send more of the body,
# so that a stalled download fails instead of hanging forever
download_timeout = 60
# Files of the node.js zip archive extracted at once: on Windows, scanners
# that check every new file make a one by one extraction slow
extract_workers = 32

# ---------------------------------------------------------
# Utils


class Config(object):
    """
    Configuration namespace.
    """

    # Defaults
    node = 'latest'
    npm = 'latest'
    with_npm = False
    jobs = '2'
    without_ssl = False
    debug = False
    profile = False
    make = 'make'
    prebuilt = True
    ignore_ssl_certs = False
    with_certifi = False
    mirror = None
    prefer_system = False
    isolate_npm = False
    clean_src = True
    parallel_extract = True

    @classmethod
    def _load(cls, configfiles, verbose=False):
        """
        Load configuration from the given files in reverse order,
        if they exist and have a [nodeenv] section.
        Additionally, load version from .node-version if file exists.
        """
        for configfile in reversed(configfiles):
            configfile = os.path.expanduser(configfile)
            if not os.path.exists(configfile):
                continue

            ini_file = ConfigParser()
            ini_file.read(configfile)
            section = "nodeenv"
            if not ini_file.has_section(section):
                continue

            for attr, val in vars(cls).items():
                if attr.startswith('_') or not \
                   ini_file.has_option(section, attr):
                    continue

                if isinstance(val, bool):
                    val = ini_file.getboolean(section, attr)
                else:
                    try:
                        val = ini_file.get(section, attr)
                    except InterpolationSyntaxError:
                        # A percent-encoded mirror login, #440
                        val = ini_file.get(section, attr, raw=True)
                        if '%(' in val:
                            raise
                    # ConfigParser keeps the quotes the README shows, #321
                    if len(val) > 1 and val[0] == val[-1] and val[0] in '\'"':
                        val = val[1:-1]

                if verbose:
                    print('CONFIG {0}: {1} = {2}'.format(
                        os.path.basename(configfile), attr, val))
                setattr(cls, attr, val)

        if os.path.exists(".node-version"):
            with open(".node-version", "r") as v_file:
                setattr(cls, "node", v_file.readline().strip().lstrip("v"))

    @classmethod
    def _dump(cls):
        """
        Print defaults for the README.
        """
        print("    [nodeenv]")
        print("    " + "\n    ".join(
            "%s = %s" % (k, v) for k, v in sorted(vars(cls).items())
            if not k.startswith('_')))


Config._default = dict(
    (attr, val) for attr, val in vars(Config).items()
    if not attr.startswith('_')
)


def clear_output(out):
    """
    Remove new-lines and
    """
    return out.decode('utf-8').replace('\n', '')


def remove_env_bin_from_path(env, env_bin_dir):
    """
    Remove bin directory of the current environment from PATH
    """
    env_bin_dir = os.path.realpath(env_bin_dir)
    return ':'.join(
        p for p in env.split(':') if os.path.realpath(p) != env_bin_dir)


def find_system_node(env_bin_dir=None):
    """
    Find system-wide nodejs or node in PATH, ignoring the environment
    bin directory when it is given
    """
    path_var = os.environ['PATH']
    if env_bin_dir is not None:
        path_var = remove_env_bin_from_path(path_var, env_bin_dir)
    for candidate in ("nodejs", "node"):
        found = shutil.which(candidate, path=path_var)
        if found is not None:
            return found
    return None


def parse_version(version_str):
    """
    Parse version string to a tuple of integer parts
    """
    v = version_str.replace('v', '').split('.')[:3]
    # remove all after '+' or '-' in the PATCH part of the version,
    # e.g. 27.0.0-alpha.0.0.0
    if len(v) >= 3:
        v[2] = re.split('[+-]', v[2])[0]
    return tuple(map(int, v))


_EXACT_VERSION_RE = re.compile(r'^v?\d+\.\d+\.\d+(\+\S*)?$')

_COMPARATOR_RE = re.compile(
    r'^(?P<op>\^|~|>=|<=|>|<|=)?\s*'
    r'v?(?P<major>\d+|[xX*])'
    r'(?:\.(?P<minor>\d+|[xX*]))?'
    r'(?:\.(?P<patch>\d+|[xX*]))?$'
)

_OPERATORS = {
    '>=': operator.ge,
    '>': operator.gt,
    '<=': operator.le,
    '<': operator.lt,
    '=': operator.eq,
}


def _pad_version(version):
    """
    Pad a version tuple to (major, minor, patch)
    """
    parts = tuple(version)[:3]
    return parts + (0,) * (3 - len(parts))


def _is_exact_version(version_str):
    """
    Check that the string is a complete version and needs no resolving
    """
    return _EXACT_VERSION_RE.match(version_str) is not None


def _is_wildcard(part):
    """
    Check that a version part is missing or a wildcard
    """
    return part is None or part in ('x', 'X', '*')


def _comparator_constraints(match):
    """
    Expand a single semver comparator to a list of (operator, version)

    Partial versions round up to the next release, as npm does:
    `>4.3` means `>=4.4.0` and `<=4.3` means `<4.4.0`.
    """
    op = match.group('op') or '='
    major, minor, patch = (
        match.group('major'), match.group('minor'), match.group('patch'))

    if _is_wildcard(major):
        return []

    major = int(major)
    has_minor = not _is_wildcard(minor)
    has_patch = has_minor and not _is_wildcard(patch)
    minor = int(minor) if has_minor else 0
    patch = int(patch) if has_patch else 0
    low = (major, minor, patch)

    next_major = (major + 1, 0, 0)
    next_minor = (major, minor + 1, 0)

    if op == '^':
        # allow changes that do not modify the leftmost non-zero part
        if not has_minor or major > 0:
            return [('>=', low), ('<', next_major)]
        if not has_patch or minor > 0:
            return [('>=', low), ('<', next_minor)]
        return [('>=', low), ('<', (0, 0, patch + 1))]

    if op == '~':
        if not has_minor:
            return [('>=', low), ('<', next_major)]
        return [('>=', low), ('<', next_minor)]

    if op == '=':
        if not has_minor:
            return [('>=', low), ('<', next_major)]
        if not has_patch:
            return [('>=', low), ('<', next_minor)]
        return [('>=', low), ('<=', low)]

    if op == '>':
        if not has_minor:
            return [('>=', next_major)]
        if not has_patch:
            return [('>=', next_minor)]
        return [('>', low)]

    if op == '<=':
        if not has_minor:
            return [('<', next_major)]
        if not has_patch:
            return [('<', next_minor)]
        return [('<=', low)]

    # '>=' and '<' take the version padded with zeros
    return [(op, low)]


def _parse_comparator(token):
    """
    Parse one comparator, return None if it is not valid
    """
    match = _COMPARATOR_RE.match(token)
    if match is None:
        return None
    return _comparator_constraints(match)


def parse_node_range(spec):
    """
    Parse an npm-style semver range

    Return a list of alternatives, each a list of (operator, version)
    constraints that must all hold, or None if `spec` is not a range.
    """
    if not spec:
        return None

    ranges = []
    for alternative in spec.split('||'):
        tokens = alternative.split()
        if not tokens:
            return None

        if '-' in tokens:
            # hyphen range: `4.3.1 - 6.2.0`
            if len(tokens) != 3 or tokens[1] != '-':
                return None
            groups = [
                _parse_comparator('>=' + tokens[0]),
                _parse_comparator('<=' + tokens[2]),
            ]
        else:
            groups = [_parse_comparator(token) for token in tokens]

        constraints = []
        for group in groups:
            if group is None:
                return None
            constraints.extend(group)
        ranges.append(constraints)

    return ranges


def match_node_range(version, ranges):
    """
    Check that a version tuple satisfies any of the parsed alternatives
    """
    version = _pad_version(version)
    return any(
        all(_OPERATORS[op](version, other) for op, other in constraints)
        for constraints in ranges
    )


def node_version_from_args(args):
    """
    Parse the node version from the argparse args
    """
    if args.node == 'system':
        node_bin = find_system_node() or 'node'
        out, err = subprocess.Popen(
            [node_bin, "--version"], stdout=subprocess.PIPE).communicate()
        return parse_version(clear_output(out))

    return parse_version(args.node)


def get_installed_node_version(env_dir):
    """
    Return version of node installed in env_dir, None if there is none
    """
    bin_dir = join(env_dir, 'Scripts' if is_WIN else 'bin')
    node_bin = join(bin_dir, 'node.exe' if is_WIN else 'node')
    if not os.path.exists(node_bin):
        return None

    with open(node_bin, 'rb') as f:
        # a shim runs the system node, it is not an installed one
        if f.read(2) == b'#!':
            return None

    try:
        out, _ = subprocess.Popen(
            [node_bin, "--version"], stdout=subprocess.PIPE).communicate()
        return parse_version(clear_output(out))
    except (OSError, ValueError):
        return None


def create_logger():
    """
    Create logger for diagnostic
    """
    # create logger
    loggr = logging.getLogger("nodeenv")
    loggr.setLevel(logging.INFO)

    # monkey patch
    def emit(self, record):
        msg = self.format(record)
        fs = "%s" if getattr(record, "continued", False) else "%s\n"
        self.stream.write(fs % msg)
        self.flush()
    logging.StreamHandler.emit = emit

    # create console handler and set level to debug
    ch = logging.StreamHandler()
    ch.setLevel(logging.DEBUG)

    # create formatter
    formatter = logging.Formatter(fmt="%(message)s")

    # add formatter to ch
    ch.setFormatter(formatter)

    # add ch to logger
    loggr.addHandler(ch)
    return loggr


logger = create_logger()


class PrintCompletionAction(argparse.Action):
    """
    Print the shell completion script made by shtab and exit.

    shtab is imported only here, so nodeenv works without it.
    """
    # options completed with file and with directory names
    file_dests = ('requirements', 'local_requirements', 'config_file',
                  'make_path')
    dir_dests = ('python_virtualenv', 'env_dir')

    def __call__(self, parser, namespace, values, option_string=None):
        try:
            import shtab
        except ImportError:
            parser.error('shtab is not installed, run: '
                         'pip install "nodeenv[completion]"')
        if values not in shtab.SUPPORTED_SHELLS or not hasattr(shtab, 'cmd'):
            parser.error('the installed shtab cannot complete %s, run: '
                         'pip install -U "nodeenv[completion]"' % values)

        node_versions = 'echo latest; echo lts'
        if not is_WIN:
            # system node.js is not supported on Windows
            node_versions += '; echo system'
        for action in parser._actions:
            if action.dest in self.file_dests:
                action.complete = shtab.FILE
            elif action.dest in self.dir_dests:
                action.complete = shtab.DIRECTORY
            elif action.dest == 'node':
                action.complete = shtab.cmd(node_versions)
            elif action.dest == 'npm':
                action.complete = shtab.cmd('echo latest')

        # complete `nodeenv`, whatever command line started this run
        parser.prog = 'nodeenv'
        print(shtab.complete(parser, values))
        parser.exit()


def make_parser():
    """
    Make a command line argument parser.
    """
    parser = argparse.ArgumentParser(
        usage="%(prog)s [OPTIONS] DEST_DIR")

    parser.add_argument(
        '--version', action='version', version=nodeenv_version)

    parser.add_argument(
        '--print-completion', action=PrintCompletionAction,
        choices=('bash', 'zsh', 'fish'), metavar='SHELL',
        help='Print the completion script for SHELL (bash, zsh or fish) '
        'and exit. Needs shtab: pip install "nodeenv[completion]"')

    parser.add_argument(
        '-n', '--node', dest='node', metavar='NODE_VER', default=Config.node,
        help='The node.js version to use, e.g., '
        '--node=0.4.3 will use the node-v0.4.3 '
        'to create the new environment. '
        'Accepts npm-style semver ranges too, e.g. --node=22, '
        '--node=4.x or --node="^4.3.1", resolved to the highest '
        'matching release. '
        'The default is last stable version (`latest`). '
        'Use `lts` to use the latest LTS release. '
        'Use `system` to use system-wide node.')

    parser.add_argument(
        '--prefer-system', dest='prefer_system',
        action='store_true', default=Config.prefer_system,
        help='Use system-wide node if it is found in PATH, otherwise '
        'install the version given by --node.')

    parser.add_argument(
        '--mirror',
        action="store", dest='mirror', default=Config.mirror,
        help='Set mirror server of nodejs.org to download from.')

    if not is_WIN:
        parser.add_argument(
            '-j', '--jobs', dest='jobs', default=Config.jobs,
            help='Sets number of parallel commands at node.js compilation. '
            'The default is 2 jobs.')

        parser.add_argument(
            '--load-average', dest='load_average',
            help='Sets maximum load average for executing parallel commands '
            'at node.js compilation.')

        parser.add_argument(
            '--without-ssl', dest='without_ssl',
            action='store_true', default=Config.without_ssl,
            help='Build node.js without SSL support')

        parser.add_argument(
            '--debug', dest='debug',
            action='store_true', default=Config.debug,
            help='Build debug variant of the node.js')

        parser.add_argument(
            '--profile', dest='profile',
            action='store_true', default=Config.profile,
            help='Enable profiling for node.js')

        parser.add_argument(
            '--make', '-m', dest='make_path',
            metavar='MAKE_PATH',
            help='Path to make command',
            default=Config.make)

        parser.add_argument(
            '--source', dest='prebuilt',
            action='store_false', default=Config.prebuilt,
            help='Install node.js from the source')

    parser.add_argument(
        '-v', '--verbose',
        action='store_true', dest='verbose', default=False,
        help="Verbose mode")

    parser.add_argument(
        '-q', '--quiet',
        action='store_true', dest='quiet', default=False,
        help="Quiet mode")

    parser.add_argument(
        '-C', '--config-file', dest='config_file', default=None,
        help="Load a different file than '~/.nodeenvrc'. "
        "Pass an empty string for no config (use built-in defaults).")

    parser.add_argument(
        '-r', '--requirements',
        dest='requirements', default=[], action='append', metavar='FILENAME',
        help='Install all the packages listed in the given requirements file '
        'globally. May be given more than once.')

    parser.add_argument(
        '--local-requirements',
        dest='local_requirements', default=[], action='append',
        metavar='FILENAME',
        help='Install all the packages listed in the given requirements file '
        'locally, into "node_modules" of the current directory. '
        'May be given more than once.')

    parser.add_argument(
        '--prompt', dest='prompt',
        help='Provides an alternative prompt prefix for this environment')

    parser.add_argument(
        '-l', '--list', dest='list',
        action='store_true', default=False,
        help='Lists available node.js versions')

    parser.add_argument(
        '--update', dest='update',
        action='store_true', default=False,
        help='Install npm packages from file without node')

    parser.add_argument(
        '--with-npm', dest='with_npm',
        action='store_true', default=Config.with_npm,
        help='Build without installing npm into the new virtual environment. '
        'Required for node.js < 0.6.3. By default, the npm included with '
        'node.js is used. Under Windows, this defaults to true.')

    parser.add_argument(
        '--npm', dest='npm',
        metavar='NPM_VER', default=Config.npm,
        help='The npm version to use, e.g., '
        '--npm=0.3.18 will use the npm-0.3.18.tgz '
        'tarball to install. '
        'The default is last available version (`latest`).')

    parser.add_argument(
        '--no-npm-clean', dest='no_npm_clean',
        action='store_true', default=False,
        help='Skip the npm 0.x cleanup.  Cleanup is enabled by default.')

    parser.add_argument(
        '--isolate-npm', dest='isolate_npm',
        action='store_true', default=Config.isolate_npm,
        help='Keep npm cache, userconfig and init-module inside the '
        'environment instead of $HOME. Not supported on Windows.')

    parser.add_argument(
        '--python-virtualenv', '-p', dest='python_virtualenv',
        nargs='?', const=True, default=False, metavar='VENV_DIR',
        help='Use the given python virtualenv, or the current one '
        'if no directory is given')

    parser.add_argument(
        '--clean-src', '-c', dest='clean_src',
        action='store_true', default=Config.clean_src,
        help='Remove "src" directory after installation (default)')

    parser.add_argument(
        '--no-clean-src', dest='clean_src',
        action='store_false',
        help='Keep "src" directory after installation')

    parser.add_argument(
        '--parallel-extract', dest='parallel_extract',
        action='store_true', default=Config.parallel_extract,
        help='Extract the node.js zip archive on Windows, Cygwin and MSYS '
        '%d files at a time (default)' % extract_workers)

    parser.add_argument(
        '--no-parallel-extract', dest='parallel_extract',
        action='store_false',
        help='Extract the node.js zip archive one file at a time')

    parser.add_argument(
        '--force', dest='force',
        action='store_true', default=False,
        help='Force installation in a pre-existing directory')

    parser.add_argument(
        '--prebuilt', dest='prebuilt',
        action='store_true', default=Config.prebuilt,
        help='Install node.js from prebuilt package (default)')

    parser.add_argument(
        '--ignore_ssl_certs', dest='ignore_ssl_certs',
        action='store_true', default=Config.ignore_ssl_certs,
        help='Ignore certificates for package downloads. - UNSAFE -')

    parser.add_argument(
        '--with-certifi', dest='with_certifi',
        action='store_true', default=Config.with_certifi,
        help='Use the certifi certificate bundle for package downloads, '
        'if certifi is installed. Ignored with --ignore_ssl_certs.')

    parser.add_argument(
        metavar='DEST_DIR', dest='env_dir', nargs='?',
        help='Destination directory')

    return parser


def parse_args(check=True):
    """
    Parses command line arguments.

    Set `check` to False to skip validation checks.
    """
    parser = make_parser()
    args = parser.parse_args()

    if args.config_file is None:
        args.config_file = ["./tox.ini", "./setup.cfg", "~/.nodeenvrc"]
    elif not args.config_file:
        args.config_file = []
    else:
        # Make sure that explicitly provided files exist
        if not os.path.exists(args.config_file):
            parser.error("Config file '{0}' doesn't exist!".format(
                args.config_file))
        args.config_file = [args.config_file]

    if not check:
        return args

    if not args.list:
        if not args.python_virtualenv and not args.env_dir:
            parser.error('You must provide a DEST_DIR or '
                         'use current python virtualenv')

    return args


def mkdir(path):
    """
    Create directory
    """
    if not os.path.exists(path):
        logger.debug(' * Creating: %s ... ', path, extra=dict(continued=True))
        os.makedirs(path)
        logger.debug('done.')
    else:
        logger.debug(' * Directory %s already exists', path)


def make_executable(filename):
    mode_0755 = (stat.S_IRWXU | stat.S_IXGRP |
                 stat.S_IRGRP | stat.S_IROTH | stat.S_IXOTH)
    os.chmod(filename, mode_0755)


PS1_SIGNATURE = b'# SIG # Begin signature block'


# noinspection PyArgumentList
def writefile(dest, content, overwrite=True, append=False):
    """
    Create file and write content in it
    """
    if not isinstance(content, bytes):
        content = bytes(content, 'utf-8')
    if not os.path.exists(dest):
        logger.debug(' * Writing %s ... ', dest, extra=dict(continued=True))
        with open(dest, 'wb') as f:
            f.write(content)
        make_executable(dest)
        logger.debug('done.')
        return
    else:
        with open(dest, 'rb') as f:
            c = f.read()
        if content in c:
            logger.debug(' * Content %s already in place', dest)
            return

        if not overwrite:
            logger.info(' * File %s exists with different content; '
                        ' not overwriting', dest)
            return

        if append:
            logger.info(' * Appending data to %s', dest)
            # PowerShell refuses to parse code that follows the signature
            # block of a signed script, and the Activate.ps1 python ships
            # on Windows is signed, so the new part goes in front of it.
            # Editing the script voids that signature either way
            # https://github.com/ekalinin/nodeenv/issues/243
            head, signature, rest = c.partition(PS1_SIGNATURE)
            # and the appended part starts on a line of its own: a
            # "deactivate.bat" ending with `:END` and no newline would
            # swallow the first appended line into the label
            if head and not head.endswith(b'\n'):
                head += b'\n'
            if signature and not content.endswith(b'\n'):
                content += b'\n'
            with open(dest, 'wb') as f:
                f.write(head + content + signature + rest)
            return

        logger.info(' * Overwriting %s with new content', dest)
        with open(dest, 'wb') as f:
            f.write(content)


def callit(cmd, show_stdout=True, in_shell=False,
           cwd=None, extra_env=None):
    """
    Execute cmd line in sub-shell
    """
    all_output = []
    cmd_parts = []

    for part in cmd:
        if len(part) > 45:
            part = part[:20] + "..." + part[-20:]
        if ' ' in part or '\n' in part or '"' in part or "'" in part:
            part = '"%s"' % part.replace('"', '\\"')
        cmd_parts.append(part)
    cmd_desc = ' '.join(cmd_parts)
    logger.debug(" ** Running command %s" % cmd_desc)

    if in_shell:
        cmd = ' '.join(cmd)

    # output
    stdout = subprocess.PIPE

    # env
    if extra_env:
        env = os.environ.copy()
        if extra_env:
            env.update(extra_env)
    else:
        env = None

    # execute
    try:
        proc = subprocess.Popen(
            cmd, stderr=subprocess.STDOUT, stdin=None, stdout=stdout,
            cwd=cwd, env=env, shell=in_shell)
    except Exception:
        e = sys.exc_info()[1]
        logger.error("Error %s while executing command %s" % (e, cmd_desc))
        raise

    stdout = proc.stdout
    while stdout:
        line = stdout.readline()
        if not line:
            break
        try:
            if is_WIN:
                line = line.decode('mbcs').rstrip()
            else:
                line = line.decode('utf8').rstrip()
        except UnicodeDecodeError:
            line = line.decode('cp866').rstrip()
        all_output.append(line)
        if show_stdout:
            logger.info(line)
    proc.wait()

    # error handler
    if proc.returncode:
        if show_stdout:
            for s in all_output:
                logger.critical(s)
        raise OSError("Command %s failed with error code %s"
                      % (cmd_desc, proc.returncode))

    return proc.returncode, all_output


def get_root_url(version_str):
    if parse_version(version_str) > (0, 5):
        return '%s/v%s/' % (src_base_url, version_str)
    else:
        return src_base_url


def is_x86_64_musl():
    # the vendor field differs between distros: pc, unknown, alpine, ...
    host_gnu_type = sysconfig.get_config_var('HOST_GNU_TYPE') or ''
    return (host_gnu_type.startswith('x86_64-')
            and host_gnu_type.endswith('-linux-musl'))


def is_riscv64():
    return platform.machine() == 'riscv64'


def get_node_bin_url(version):
    archmap = {
        'x86':    'x86',  # Windows Vista 32
        'i686':   'x86',
        'x86_64': 'x64',  # Linux Ubuntu 64
        'amd64':  'x64',  # FreeBSD 64bits
        'amd64':  'x64',  # Windows Server 2012 R2 (x64)
        'i86pc':  'x64',  # Solaris/illumos 64
        'armv6l': 'armv6l',     # arm
        'armv7l': 'armv7l',
        'armv8l': 'armv7l',
        'aarch64': 'arm64',
        'arm64': 'arm64',
        'arm64/v8': 'arm64',
        'armv8': 'arm64',
        'armv8.4': 'arm64',
        'ppc64le': 'ppc64le',   # Power PC
        's390x': 's390x',       # IBM S390x
        'riscv64': 'riscv64',   # RISCV 64
    }
    sysinfo = {
        'system': platform.system().lower(),
        'arch': archmap[platform.machine().lower()],
    }
    if is_WIN or is_CYGWIN:
        postfix = '-win-%(arch)s.zip' % sysinfo
    elif is_x86_64_musl():
        postfix = '-linux-x64-musl.tar.gz'
    else:
        postfix = '-%(system)s-%(arch)s.tar.gz' % sysinfo
    filename = 'node-v%s%s' % (version, postfix)
    return get_root_url(version) + filename


def get_node_src_url(version):
    tar_name = 'node-v%s.tar.gz' % version
    return get_root_url(version) + tar_name


@contextlib.contextmanager
def tarfile_open(*args, **kwargs):
    """Compatibility layer because py26."""
    tf = tarfile.open(*args, **kwargs)
    try:
        yield tf
    finally:
        tf.close()


def _download_node_file(node_url, n_attempt=3, delay=3):
    """Do multiple attempts to avoid incomplete data in case
    of unstable network"""
    while n_attempt > 0:
        try:
            return io.BytesIO(urlopen(node_url).read())
        except urllib2.HTTPError:
            # The server answered, install_node_wrapped() decides what
            # the status means
            raise
        except (IncompleteRead, OSError) as e:
            # OSError: a connection reset or timed out in the middle
            # of the body https://github.com/ekalinin/nodeenv/issues/324
            logger.warning(
                'Incomplete read while reading '
                'from {} - {}'.format(node_url, e)
            )
            n_attempt -= 1
            if n_attempt == 0:
                logger.error('Error: cannot download %s: %s' % (node_url, e))
                sys.exit(1)
            time.sleep(delay)


def _extract_zip(archive, path, names):
    """
    Extract the names of a zip archive extract_workers files at a time

    Scanners that check every new file make a one by one extraction take
    hours on Windows https://github.com/ekalinin/nodeenv/issues/366
    """
    # Create the directories first, so that threads never create the same
    # one: before Python 3.13 zipfile fails if another thread did it first
    files = [name for name in names if not name.endswith('/')]
    dirs = set(name for name in names if name.endswith('/'))
    dirs.update(name.rpartition('/')[0] + '/' for name in files
                if '/' in name)
    for name in sorted(dirs):
        archive.extract(zipfile.ZipInfo(name), path)

    with concurrent.futures.ThreadPoolExecutor(extract_workers) as pool:
        try:
            pending = [pool.submit(archive.extract, name, path)
                       for name in files]
            while pending:
                # Wake up now and then: before Python 3.14 Ctrl+C does not
                # interrupt waiting on a lock on Windows
                done, pending = concurrent.futures.wait(
                    pending, timeout=0.1,
                    return_when=concurrent.futures.FIRST_EXCEPTION)
                for future in done:
                    future.result()
        except BaseException:
            # Drop the files that have not started yet
            pool.shutdown(cancel_futures=True)
            raise


def download_node_src(node_url, src_dir, args):
    """
    Download source code
    """
    logger.info('.', extra=dict(continued=True))
    dl_contents = _download_node_file(node_url)
    logger.info('.', extra=dict(continued=True))

    if is_WIN or is_CYGWIN:
        ctx = zipfile.ZipFile(dl_contents)
        members = operator.methodcaller('namelist')
        member_name = lambda s: s  # noqa: E731
    else:
        ctx = tarfile_open(fileobj=dl_contents)
        members = operator.methodcaller('getmembers')
        member_name = operator.attrgetter('name')

    with ctx as archive:
        node_ver = re.escape(args.node)
        rexp_string = r"node-v%s[^/]*/(README\.md|CHANGELOG\.md|LICENSE)"\
            % node_ver
        extract_list = [
            member
            for member in members(archive)
            if re.match(rexp_string, member_name(member)) is None
        ]
        if (is_WIN or is_CYGWIN) and args.parallel_extract:
            _extract_zip(archive, src_dir, extract_list)
        # filter= is a tarfile-only argument, zipfile has no such option
        elif sys.version_info >= (3, 12) and not (is_WIN or is_CYGWIN):
            archive.extractall(src_dir, extract_list, filter="data")
        else:
            archive.extractall(src_dir, extract_list)


def make_certifi_context():
    """
    Build an SSL context backed by the certifi bundle.

    Returns None if certifi is not installed, so that downloads keep
    using the system certificate store.
    """
    try:
        import certifi
    except ImportError:
        logger.warning(
            'certifi is not installed, --with-certifi is ignored: '
            'falling back to the system certificate store')
        return None

    return ssl.create_default_context(cafile=certifi.where())


def get_proxy_settings():
    """
    Proxy environment variables urllib acts on, as 'name=value' strings.
    """
    names = ('http_proxy', 'https_proxy', 'HTTP_PROXY', 'HTTPS_PROXY')
    return ['%s=%s' % (n, os.environ[n]) for n in names if os.environ.get(n)]


def _urlopen(req):
    if ignore_ssl_certs:
        # py27: protocol required, py3: optional
        # https://github.com/ekalinin/nodeenv/issues/296
        context = ssl.SSLContext(ssl.PROTOCOL_TLS)
        context.verify_mode = ssl.CERT_NONE
        return urllib2.urlopen(req, context=context,
                               timeout=download_timeout)

    # Use certifi certificates if they were requested and are available
    if certifi_context is not None:
        return urllib2.urlopen(req, context=certifi_context,
                               timeout=download_timeout)

    return urllib2.urlopen(req, timeout=download_timeout)


def split_url_auth(url):
    """
    Cut user:password@ off the URL, urllib takes it for a part of the host,
    and return it as a Basic Authorization header.
    https://github.com/ekalinin/nodeenv/issues/321
    """
    parts = urlsplit(url)
    if parts.username is None:
        return url, None
    credentials = '%s:%s' % (unquote(parts.username),
                             unquote(parts.password or ''))
    auth = base64.b64encode(credentials.encode('utf-8')).decode('ascii')
    netloc = parts.netloc.rpartition('@')[2]
    return urlunsplit(parts._replace(netloc=netloc)), 'Basic ' + auth


def urlopen(url):
    home_url = "https://github.com/ekalinin/nodeenv/"
    headers = {'User-Agent': 'nodeenv/%s (%s)' % (nodeenv_version, home_url)}
    req = urllib2.Request(url, None, headers)
    # the mirror's password is neither for other hosts, like the npm
    # registry, nor for the ones the mirror redirects to
    if src_auth and url.startswith(src_base_url):
        req.add_unredirected_header('Authorization', src_auth)
    try:
        return _urlopen(req)
    except urllib2.HTTPError:
        # The server answered, callers decide what the status means
        raise
    except (urllib2.URLError, socket.timeout) as e:
        # Nothing was reached at all: a broken proxy, no DNS, no route.
        # https://github.com/ekalinin/nodeenv/issues/229
        # Or nothing answered in download_timeout seconds: urllib wraps
        # a timeout into URLError only while it sends the request.
        logger.error('Error: cannot download %s: %s'
                     % (url, getattr(e, 'reason', e)))
        proxies = get_proxy_settings()
        if proxies:
            logger.error('Error: check the proxy settings: %s'
                         % ', '.join(proxies))
        sys.exit(1)

# ---------------------------------------------------------
# Virtual environment functions


def copytree(src, dst, symlinks=False, ignore=None):
    for item in os.listdir(src):
        s = os.path.join(src, item)
        d = os.path.join(dst, item)
        if os.path.isdir(s):
            try:
                shutil.copytree(s, d, symlinks, ignore)
            except OSError:
                copytree(s, d, symlinks, ignore)
        else:
            if os.path.islink(s):
                # copy link only if it not exists. #189
                if not os.path.islink(d):
                    os.symlink(os.readlink(s), d)
            else:
                shutil.copy2(s, d)


def copy_node_from_prebuilt(env_dir, src_dir, node_version):
    """
    Copy prebuilt binaries into environment
    """
    logger.info('.', extra=dict(continued=True))
    if is_WIN:
        dest = join(env_dir, 'Scripts')
        mkdir(dest)
    elif is_CYGWIN:
        dest = join(env_dir, 'bin')
        mkdir(dest)
        # write here to avoid https://bugs.python.org/issue35650
        writefile(join(env_dir, 'bin', 'node'), CYGWIN_NODE)
    else:
        dest = env_dir

    src_folder_tpl = glob.escape('node-v%s-' % node_version) + '*'
    src_folder, = glob.glob(src_folder_tpl, root_dir=src_dir)
    copytree(join(src_dir, src_folder), dest, True)

    if is_CYGWIN:
        for filename in ('npm', 'npx', 'node.exe'):
            filename = join(env_dir, 'bin', filename)
            if os.path.exists(filename):
                make_executable(filename)

    logger.info('.', extra=dict(continued=True))


def build_node_from_src(env_dir, src_dir, node_src_dir, args):
    env = {}
    make_param_names = ['load-average', 'jobs']
    make_param_values = map(
        lambda x: getattr(args, x.replace('-', '_')),
        make_param_names)
    make_opts = [
        '--{0}={1}'.format(name, value)
        if len(value) > 0 else '--{0}'.format(name)
        for name, value in zip(make_param_names, make_param_values)
        if value is not None
    ]

    node_version = parse_version(args.node)
    if node_version < (13, 0):
        # up to 12.x, ./configure only accepts Python 2.7, so fail early
        # with a clear error instead of a failed ./configure
        python2_path = shutil.which('python2')
        if not python2_path:
            raise OSError(
                'no python2 command (required for building '
                'node.js %s) was found' % args.node
            )
        if node_version < (8, 8):
            # before 8.8, gyp and make run `python` by name and node does
            # not point it at Python 2 itself, so point `python` at python2
            logger.debug(' * Temporarily pointing python to %s',
                         python2_path)
            node_tmpbin_dir = join(src_dir, 'tmpbin')
            node_tmpbin_link = join(node_tmpbin_dir, 'python')
            mkdir(node_tmpbin_dir)
            if not os.path.exists(node_tmpbin_link):
                callit(['ln', '-s', python2_path, node_tmpbin_link])
            env['PATH'] = '{}:{}'.format(node_tmpbin_dir,
                                         os.environ.get('PATH', ''))

    conf_cmd = [
        './configure',
        '--prefix=%s' % _quote(env_dir)
    ]
    if args.without_ssl:
        conf_cmd.append('--without-ssl')
    if args.debug:
        conf_cmd.append('--debug')
    if args.profile:
        conf_cmd.append('--profile')

    make_cmd = args.make_path

    callit(conf_cmd, args.verbose, True, node_src_dir, env)
    logger.info('.', extra=dict(continued=True))
    callit([make_cmd] + make_opts, args.verbose, True, node_src_dir, env)
    logger.info('.', extra=dict(continued=True))
    callit([make_cmd + ' install'], args.verbose, True, node_src_dir, env)


def install_node(env_dir, src_dir, args):
    """
    Download source code for node.js, unpack it
    and install it in virtual environment.
    """
    try:
        install_node_wrapped(env_dir, src_dir, args)
    except BaseException:
        # this restores the newline suppressed by continued=True
        logger.info('')
        raise


def report_node_download_error(errors, args):
    """
    Report an undownloadable node.js archive instead of a traceback.

    A 404 usually means the version ships no build for the host platform.
    https://github.com/ekalinin/nodeenv/issues/250
    """
    # this restores the newline suppressed by continued=True
    logger.info('')
    for node_url, error in errors:
        logger.error('Error: cannot download %s: HTTP %s %s'
                     % (node_url, error.code, error.reason))

    if errors[-1][1].code == 404:
        if args.prebuilt:
            logger.error('Error: there is no prebuilt node.js %s for %s-%s'
                         % (args.node, platform.system().lower(),
                            platform.machine().lower()))
            logger.error('Error: check "nodeenv --list" for the available '
                         'versions, or build from source with --source')
        else:
            logger.error('Error: there is no node.js %s source archive'
                         % args.node)
            logger.error('Error: check "nodeenv --list" for the available '
                         'versions')
    sys.exit(1)


def install_node_wrapped(env_dir, src_dir, args):
    env_dir = abspath(env_dir)
    node_src_dir = join(src_dir, 'node-v%s' % args.node)
    src_type = "prebuilt" if args.prebuilt else "source"

    logger.info(' * Install %s node (%s) ' % (src_type, args.node),
                extra=dict(continued=True))

    if args.prebuilt:
        node_url = get_node_bin_url(args.node)
    else:
        node_url = get_node_src_url(args.node)

    node_urls = [node_url]
    if "arm64" in node_url:
        # if arm64 not found, try x64
        node_urls.append(node_url.replace('arm64', 'x64'))

    # Only source builds can reuse the source directory.
    if args.prebuilt or not os.path.exists(node_src_dir):
        errors = []
        # retry outside the handler, so that a second failure is not
        # reported as "during handling of the above exception"
        for url in node_urls:
            try:
                download_node_src(url, src_dir, args)
                break
            except urllib2.HTTPError as e:
                errors.append((url, e))
        else:
            report_node_download_error(errors, args)

    logger.info('.', extra=dict(continued=True))

    if args.prebuilt:
        copy_node_from_prebuilt(env_dir, src_dir, args.node)
    else:
        build_node_from_src(env_dir, src_dir, node_src_dir, args)

    logger.info(' done.')


def install_npm(env_dir, _src_dir, args):
    """
    Download source code for npm, unpack it
    and install it in virtual environment.
    """
    logger.info(' * Install npm.js (%s) ... ' % args.npm,
                extra=dict(continued=True))
    env = dict(
        os.environ,
        clean='no' if args.no_npm_clean else 'yes',
        npm_install=args.npm,
    )
    proc = subprocess.Popen(
        (
            'sh', '-c',
            '. {0} && npm install -g npm@{1}'.format(
                _quote(join(env_dir, 'bin', 'activate')),
                args.npm,
            )
        ),
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    out, _ = proc.communicate()
    if args.verbose:
        logger.info(out)
    logger.info('done.')


def install_npm_win(env_dir, src_dir, args):
    """
    Download npm as published to the registry, unpack it
    and install it in virtual environment.
    """
    logger.info(' * Install npm.js (%s) ... ' % args.npm,
                extra=dict(continued=True))
    # Not the GitHub source archive: its workspace symlinks unpack as text
    # files, or the workspaces are missing from it altogether
    # https://github.com/ekalinin/nodeenv/issues/310
    npm_meta_url = 'https://registry.npmjs.org/npm/%s' % args.npm
    npm_meta = json.loads(urlopen(npm_meta_url).read().decode('UTF-8'))
    npm_url = npm_meta['dist']['tarball']
    npm_contents = io.BytesIO(urlopen(npm_url).read())

    bin_path = join(env_dir, 'Scripts')
    node_modules_path = join(bin_path, 'node_modules', 'npm')

    if os.path.exists(node_modules_path):
        shutil.rmtree(node_modules_path)

    if os.path.exists(join(bin_path, 'npm.cmd')):
        os.remove(join(bin_path, 'npm.cmd'))

    if os.path.exists(join(bin_path, 'npm-cli.js')):
        os.remove(join(bin_path, 'npm-cli.js'))

    npm_src_dir = join(src_dir, 'npm-%s' % npm_meta['version'])
    with tarfile_open(fileobj=npm_contents) as tarf:
        if sys.version_info >= (3, 12):
            tarf.extractall(npm_src_dir, filter="data")
        else:
            tarf.extractall(npm_src_dir)

    npm_ver = join('npm-%s' % npm_meta['version'], 'package')
    shutil.copytree(join(src_dir, npm_ver), node_modules_path)
    shutil.copy(join(src_dir, npm_ver, 'bin', 'npm.cmd'),
                join(bin_path, 'npm.cmd'))
    shutil.copy(join(src_dir, npm_ver, 'bin', 'npm-cli.js'),
                join(bin_path, 'npm-cli.js'))

    if is_CYGWIN:
        shutil.copy(join(bin_path, 'npm-cli.js'),
                    join(env_dir, 'bin', 'npm-cli.js'))
        shutil.copytree(join(bin_path, 'node_modules'),
                        join(env_dir, 'bin', 'node_modules'))
        shutil.copy(join(src_dir, npm_ver, 'bin', 'npm'),
                    join(env_dir, 'bin', 'npm'))


def _read_packages(filenames):
    """
    Read package names from the given requirements files
    """
    packages = []
    for filename in filenames:
        with open(filename) as f:
            packages.extend(package.strip() for package in f.readlines())
    return packages


def install_packages(env_dir, args):
    """
    Install node.js packages via npm
    """
    logger.info(' * Install node.js packages ... ',
                extra=dict(continued=True))
    activate_path = join(env_dir, 'bin', 'activate')
    real_npm_ver = args.npm if args.npm.count(".") == 2 else args.npm + ".0"
    if args.npm == "latest" or real_npm_ver >= "1.0.0":
        cmd = '. ' + _quote(activate_path) + \
              ' && npm install %(opt)s%(pack)s'
    else:
        cmd = '. ' + _quote(activate_path) + \
              ' && npm install %(pack)s' + \
              ' && npm activate %(pack)s'

    # global packages first, local ones go to the current directory
    for opt, filenames in (('-g ', args.requirements),
                           ('', args.local_requirements)):
        for package in _read_packages(filenames):
            if not package:
                continue
            callit(cmd=[cmd % {"pack": package, "opt": opt}],
                   show_stdout=args.verbose, in_shell=True)

    logger.info('done.')


# Files python's own venv/virtualenv writes: with `-p` nodeenv works
# inside such an environment and has to extend them.  Overwriting them
# throws VIRTUAL_ENV and the deactivation script away
# https://github.com/ekalinin/nodeenv/issues/243
PYTHON_VIRTUALENV_FILES = frozenset((
    'activate', 'activate.fish', 'activate.bat', 'deactivate.bat',
    'Activate.ps1',
))

# the bin/ entry `activate` and `activate.fish` put on PATH: the python
# venv's own scripts they are appended to put it there already.  Windows
# keeps it: before python 3.12 the venv's `activate` puts a `C:\...` path
# there, which git-bash splits at the colon, and the cmd and PowerShell
# scripts reset PATH to the venv's saved one first
# https://github.com/ekalinin/nodeenv/issues/332
# https://github.com/python/cpython/issues/103088
VENV_BIN_PATH = {
    'activate': '$NODE_VIRTUAL_ENV/__BIN_NAME__:',
    'activate.fish': ' "$NODE_VIRTUAL_ENV/__BIN_NAME__"',
}


def is_python_venv_script(file_path, name):
    """
    Whether `nodeenv -p` appends to a script of the python venv.  An env
    without one (conda) gets it written from scratch, starting with the
    DISABLE_PROMPT part, and a rerun appends to that file
    """
    if not os.path.exists(file_path):
        return False
    with open(file_path, 'rb') as f:
        content = f.read()
    return not content.startswith(DISABLE_PROMPT[name].encode('utf-8'))


def install_activate(env_dir, args):
    """
    Install virtual environment activation script
    """
    if is_WIN:
        # `activate` is written on Windows too, for git-bash and the other
        # posix shells available there
        # https://github.com/ekalinin/nodeenv/issues/226
        files = {
            'activate': ACTIVATE_SH,
            'activate.bat': ACTIVATE_BAT,
            "deactivate.bat": DEACTIVATE_BAT,
            "Activate.ps1": ACTIVATE_PS1
        }
        bin_dir = join(env_dir, 'Scripts')
        shim_node = join(bin_dir, "node.exe")
        shim_nodejs = join(bin_dir, "nodejs.exe")
    else:
        files = {
            'activate': ACTIVATE_SH,
            'activate.fish': ACTIVATE_FISH,
            'shim': SHIM
        }
        bin_dir = join(env_dir, 'bin')
        shim_node = join(bin_dir, "node")
        shim_nodejs = join(bin_dir, "nodejs")
    if is_CYGWIN:
        mkdir(bin_dir)

    if args.node == "system":
        files["node"] = SHIM

    # npm keeps the global modules next to node.exe on Windows,
    # under lib/ everywhere else
    mod_dir = 'Scripts/node_modules' if is_WIN else join('lib', 'node_modules')
    prompt = args.prompt or '(%s)' % os.path.basename(os.path.abspath(env_dir))

    if args.node == "system":
        shim_node = find_system_node(bin_dir)
        assert shim_node, "Did not find nodejs or node system executable"

    for name, content in files.items():
        file_path = join(bin_dir, name)
        if args.python_virtualenv and not is_WIN and \
                name in VENV_BIN_PATH and \
                is_python_venv_script(file_path, name):
            content = content.replace(VENV_BIN_PATH[name], '')
        isolate = NPM_ISOLATE.get(name, '') if args.isolate_npm else ''
        unisolate = NPM_UNISOLATE.get(name, '') if args.isolate_npm else ''
        content = content.replace('__NPM_ISOLATE__', isolate)
        content = content.replace('__NPM_UNISOLATE__', unisolate)
        content = content.replace('__NODE_VIRTUAL_PROMPT__', prompt)
        content = content.replace('__NODE_VIRTUAL_ENV__',
                                  os.path.abspath(env_dir))
        content = content.replace('__SHIM_NODE__', shim_node)
        content = content.replace('__BIN_NAME__', os.path.basename(bin_dir))
        content = content.replace('__MOD_NAME__', mod_dir)
        if is_CYGWIN:
            _, cyg_bin_dir = callit(
                ['cygpath', '-w', os.path.abspath(bin_dir)],
                show_stdout=False, in_shell=False)
            content = content.replace('__NPM_CONFIG_PREFIX__', cyg_bin_dir[0])
        elif is_WIN:
            # npm's prefix on Windows is the directory holding node.exe
            content = content.replace('__NPM_CONFIG_PREFIX__',
                                      '$NODE_VIRTUAL_ENV/Scripts')
        else:
            content = content.replace('__NPM_CONFIG_PREFIX__',
                                      '$NODE_VIRTUAL_ENV')
        # if we call in the same environment:
        #   $ nodeenv -p --prebuilt
        #   $ nodeenv -p --node=system
        # we should get `bin/node` not as binary+string.
        # `bin/activate` should be appended if we're inside
        # existing python's virtual environment
        need_append = False
        if args.python_virtualenv:
            disable_prompt = DISABLE_PROMPT.get(name, '')
            enable_prompt = ENABLE_PROMPT.get(name, '')
            content = disable_prompt + content + enable_prompt
            need_append = name in PYTHON_VIRTUALENV_FILES
        writefile(file_path, content, append=need_append)

    if not os.path.exists(shim_nodejs):
        if is_WIN:
            # a symlink needs elevation or Developer Mode, a hard link
            # needs neither
            # https://github.com/ekalinin/nodeenv/issues/303
            try:
                os.symlink('node.exe', shim_nodejs)
            except OSError:
                try:
                    os.link(join(bin_dir, 'node.exe'), shim_nodejs)
                except OSError:
                    logger.warning(
                        'Could not create the nodejs.exe link; this is '
                        'harmless unless something on your system invokes '
                        'node as "nodejs"')
        else:
            os.symlink("node", shim_nodejs)


def set_predeactivate_hook(env_dir):
    if is_WIN:
        # Windows: create predeactivate.bat for CMD and
        #           predeactivate.ps1 for PowerShell
        writefile(join(env_dir, 'Scripts', 'predeactivate.bat'),
                  PREDEACTIVATE_BAT, append=True)
        writefile(join(env_dir, 'Scripts', 'predeactivate.ps1'),
                  PREDEACTIVATE_PS1, append=True)
    else:
        # Unix: create predeactivate for bash/sh
        writefile(join(env_dir, 'bin', 'predeactivate'),
                  PREDEACTIVATE_SH, append=True)


def create_environment(env_dir, args):
    """
    Creates a new environment in ``env_dir``.
    """
    if os.path.exists(env_dir) and not args.python_virtualenv:
        logger.info(' * Environment already exists: %s', env_dir)
        if not args.force:
            sys.exit(2)
    src_dir = abspath(join(env_dir, 'src'))
    mkdir(src_dir)

    if args.node == "system":
        mkdir(join(env_dir, 'bin'))
        mkdir(join(env_dir, 'lib'))
        mkdir(join(env_dir, 'lib', 'node_modules'))
    elif not args.force and \
            get_installed_node_version(env_dir) == parse_version(args.node):
        logger.info(' * Node.js %s is already installed, skipping '
                    '(use --force to reinstall)', args.node)
    else:
        install_node(env_dir, src_dir, args)
    # activate script install must be
    # before npm install, npm use activate
    # for install
    install_activate(env_dir, args)
    if node_version_from_args(args) < (0, 6, 3) or args.with_npm:
        instfunc = install_npm_win if is_WIN or is_CYGWIN else install_npm
        instfunc(env_dir, src_dir, args)
    if args.requirements or args.local_requirements:
        install_packages(env_dir, args)
    if args.python_virtualenv:
        set_predeactivate_hook(env_dir)
    # Cleanup
    if args.clean_src:
        shutil.rmtree(src_dir)


def _get_versions_json():
    response = urlopen('%s/index.json' % src_base_url)
    return json.loads(response.read().decode('UTF-8'))


def get_node_versions():
    return [dct['version'].lstrip('v') for dct in _get_versions_json()][::-1]


def print_node_versions():
    """
    Prints into stdout all available node.js versions
    """
    versions = get_node_versions()
    chunks_of_8 = [
        versions[pos:pos + 8] for pos in range(0, len(versions), 8)
    ]
    for chunk in chunks_of_8:
        logger.info('\t'.join(chunk))


def _has_platform_build(version_entry):
    """
    Check that the version ships a prebuilt package for the host platform
    """
    if is_x86_64_musl() and "linux-x64-musl" not in version_entry['files']:
        return False
    elif is_riscv64() and "linux-riscv64" not in version_entry['files']:
        return False

    return True


def _is_prerelease(version_entry):
    """
    Check that the version is a pre-release, e.g. 27.0.0-alpha.0.0.0
    """
    return '-' in version_entry['version']


def _get_last_node_version(lts=False):
    """
    Return last node.js version matching the filter
    """
    logger.debug(
        ' * Host platform: x86_64-musl=%s, riscv64=%s, lts=%s',
        is_x86_64_musl(), is_riscv64(), lts)

    def version_filter(v):
        if lts and not v['lts']:
            return False
        if _is_prerelease(v):
            return False

        return _has_platform_build(v)

    return next((v['version'].lstrip('v')
                 for v in _get_versions_json() if version_filter(v)), None)


def get_last_stable_node_version():
    """
    Return last stable node.js version
    """
    return _get_last_node_version()


def get_last_lts_node_version():
    """
    Return the last node.js version marked as LTS
    """
    return _get_last_node_version(lts=True)


def resolve_node_version(spec):
    """
    Resolve a semver range to the highest matching node.js version

    Strings that are not a valid range are returned unchanged, so custom
    and nightly version strings keep working.
    """
    ranges = parse_node_range(spec)
    if ranges is None:
        return spec

    matched = []
    for version_entry in _get_versions_json():
        if _is_prerelease(version_entry):
            continue
        if not _has_platform_build(version_entry):
            continue
        version = _pad_version(parse_version(version_entry['version']))
        if match_node_range(version, ranges):
            matched.append(version)

    if not matched:
        logger.error("No available node.js version matches '%s'" % spec)
        sys.exit(1)

    return '.'.join(str(part) for part in max(matched))


def get_env_dir(args):
    if args.python_virtualenv:
        # whether nodeenv itself is running inside a python virtualenv
        in_virtualenv = (
            hasattr(sys, 'real_prefix') or
            (hasattr(sys, 'base_prefix') and sys.base_prefix != sys.prefix) or
            'CONDA_PREFIX' in os.environ)
        if args.python_virtualenv is not True:
            res = args.python_virtualenv
            if not os.path.isdir(res):
                logger.error("Python virtualenv '%s' doesn't exist", res)
                sys.exit(2)
        # nodeenv itself can be installed into its own virtualenv
        # (pipx, pipsi, uv tool), so the activated one wins over sys.prefix
        elif os.environ.get('VIRTUAL_ENV'):
            res = os.environ['VIRTUAL_ENV']
            if in_virtualenv and res != sys.prefix:
                logger.warning(
                    ' * Using activated virtualenv %s, not %s where nodeenv '
                    'is installed, pass a directory to -p to override',
                    res, sys.prefix)
        elif in_virtualenv:
            res = sys.prefix
        else:
            logger.error('No python virtualenv is available')
            sys.exit(2)
    else:
        res = args.env_dir
    return res


# noinspection PyProtectedMember
def main():
    """
    Entry point
    """
    # quick&dirty way to help update the README
    if "--dump-config-defaults" in sys.argv:
        Config._dump()
        return

    args = parse_args(check=False)
    # noinspection PyProtectedMember
    Config._load(args.config_file, args.verbose)

    args = parse_args()

    if args.node.lower() == 'system' and is_WIN:
        logger.error('Installing system node.js on win32 is not supported!')
        exit(1)

    global src_base_url
    global src_auth
    global ignore_ssl_certs
    global certifi_context

    ignore_ssl_certs = args.ignore_ssl_certs
    if args.with_certifi and not ignore_ssl_certs:
        certifi_context = make_certifi_context()

    src_domain = None
    if args.mirror:
        if '://' in args.mirror:
            src_base_url = args.mirror
        else:
            src_domain = args.mirror
    # use unofficial builds only if musl and no explicitly chosen mirror
    elif is_x86_64_musl() or is_riscv64():
        src_domain = 'unofficial-builds.nodejs.org'
    else:
        src_domain = 'nodejs.org'
    if src_base_url is None:
        src_base_url = 'https://%s/download/release' % src_domain
    src_base_url, src_auth = split_url_auth(src_base_url)

    # Decide on the system node before any version resolution, so that
    # a found system node never triggers a request for index.json
    if args.prefer_system and not args.list and \
            args.node.lower() != 'system':
        if is_WIN:
            logger.warning(' * --prefer-system is not supported on win32, '
                           'installing node')
        else:
            system_node = find_system_node(join(get_env_dir(args), 'bin'))
            if system_node:
                logger.info(' * Using system node: %s' % system_node)
                args.node = 'system'
            else:
                logger.info(' * System node not found, installing %s'
                            % (args.node or 'latest'))

    if args.isolate_npm and is_WIN and not args.list:
        logger.warning(' * --isolate-npm is not supported on win32, '
                       'ignored')

    if not args.node or args.node.lower() == 'latest':
        args.node = get_last_stable_node_version()
    elif args.node.lower() == 'lts':
        args.node = get_last_lts_node_version()
    elif args.node.lower() != 'system' and not _is_exact_version(args.node):
        resolved = resolve_node_version(args.node)
        if resolved != args.node:
            logger.info(" * Resolved '%s' to %s" % (args.node, resolved))
        args.node = resolved

    if args.list:
        print_node_versions()
    elif args.update:
        env_dir = get_env_dir(args)
        install_packages(env_dir, args)
    else:
        env_dir = get_env_dir(args)
        create_environment(env_dir, args)


# ---------------------------------------------------------
# Shell scripts content

DISABLE_PROMPT = {
    'activate': """
# disable nodeenv's prompt
# (prompt already changed by original virtualenv's script)
# https://github.com/ekalinin/nodeenv/issues/26
NODE_VIRTUAL_ENV_DISABLE_PROMPT=1
""",
    'activate.fish': """
# disable nodeenv's prompt
# (prompt already changed by original virtualenv's script)
# https://github.com/ekalinin/nodeenv/issues/26
set NODE_VIRTUAL_ENV_DISABLE_PROMPT 1
""",
}

ENABLE_PROMPT = {
    'activate': """
unset NODE_VIRTUAL_ENV_DISABLE_PROMPT
""",
    'activate.fish': """
set -e NODE_VIRTUAL_ENV_DISABLE_PROMPT
""",
}

# --isolate-npm: keep npm cache, userconfig and init-module inside the
# environment. Inserted at __NPM_ISOLATE__ (activation) and
# __NPM_UNISOLATE__ (deactivation), or replaced with an empty string.
# https://github.com/ekalinin/nodeenv/issues/154
NPM_ISOLATE = {
    'activate': """
_OLD_npm_config_cache="${npm_config_cache:-}"
_OLD_npm_config_userconfig="${npm_config_userconfig:-}"
_OLD_npm_config_init_module="${npm_config_init_module:-}"
npm_config_cache="$NODE_VIRTUAL_ENV/.npm"
npm_config_userconfig="$NODE_VIRTUAL_ENV/.npmrc"
npm_config_init_module="$NODE_VIRTUAL_ENV/.npm-init.js"
export npm_config_cache npm_config_userconfig npm_config_init_module
""",
    'activate.fish': """
if set -q npm_config_cache
    set -gx _OLD_npm_config_cache $npm_config_cache
end
set -gx npm_config_cache "$NODE_VIRTUAL_ENV/.npm"

if set -q npm_config_userconfig
    set -gx _OLD_npm_config_userconfig $npm_config_userconfig
end
set -gx npm_config_userconfig "$NODE_VIRTUAL_ENV/.npmrc"

if set -q npm_config_init_module
    set -gx _OLD_npm_config_init_module $npm_config_init_module
end
set -gx npm_config_init_module "$NODE_VIRTUAL_ENV/.npm-init.js"
""",
    'shim': """
export npm_config_cache='__NODE_VIRTUAL_ENV__/.npm'
export npm_config_userconfig='__NODE_VIRTUAL_ENV__/.npmrc'
export npm_config_init_module='__NODE_VIRTUAL_ENV__/.npm-init.js'
""",
}
# --node=system writes SHIM as bin/node too
NPM_ISOLATE['node'] = NPM_ISOLATE['shim']

NPM_UNISOLATE = {
    'activate': """
        npm_config_cache="${_OLD_npm_config_cache:-}"
        npm_config_userconfig="${_OLD_npm_config_userconfig:-}"
        npm_config_init_module="${_OLD_npm_config_init_module:-}"
        export npm_config_cache npm_config_userconfig npm_config_init_module
        unset _OLD_npm_config_cache
        unset _OLD_npm_config_userconfig
        unset _OLD_npm_config_init_module
""",
    'activate.fish': """
        if set -q _OLD_npm_config_cache
            set -gx npm_config_cache $_OLD_npm_config_cache
            set -e _OLD_npm_config_cache
        else
            set -e npm_config_cache
        end

        if set -q _OLD_npm_config_userconfig
            set -gx npm_config_userconfig $_OLD_npm_config_userconfig
            set -e _OLD_npm_config_userconfig
        else
            set -e npm_config_userconfig
        end

        if set -q _OLD_npm_config_init_module
            set -gx npm_config_init_module $_OLD_npm_config_init_module
            set -e _OLD_npm_config_init_module
        else
            set -e npm_config_init_module
        end
""",
}

SHIM = """#!/usr/bin/env sh
export NODE_PATH='__NODE_VIRTUAL_ENV__/lib/node_modules'
export NPM_CONFIG_PREFIX='__NODE_VIRTUAL_ENV__'
export npm_config_prefix='__NODE_VIRTUAL_ENV__'
__NPM_ISOLATE__
exec '__SHIM_NODE__' "$@"
"""

ACTIVATE_BAT = r"""
@echo off
if defined _OLD_VIRTUAL_NPM_CONFIG_PREFIX (
    set "npm_config_prefix=%_OLD_VIRTUAL_NPM_CONFIG_PREFIX%"
) else if defined NODE_VIRTUAL_ENV (
    set npm_config_prefix=
)
set "NODE_VIRTUAL_ENV=__NODE_VIRTUAL_ENV__"
if not defined PROMPT (
    set "PROMPT=$P$G"
)
if defined _OLD_VIRTUAL_PROMPT (
    set "PROMPT=%_OLD_VIRTUAL_PROMPT%"
)
if defined _OLD_VIRTUAL_NODE_PATH (
    set "NODE_PATH=%_OLD_VIRTUAL_NODE_PATH%"
)
set "_OLD_VIRTUAL_PROMPT=%PROMPT%"
set "PROMPT=__NODE_VIRTUAL_PROMPT__ %PROMPT%"
if defined NODE_PATH (
    set "_OLD_VIRTUAL_NODE_PATH=%NODE_PATH%"
    set NODE_PATH=
)
if defined _OLD_VIRTUAL_PATH (
    set "PATH=%_OLD_VIRTUAL_PATH%"
) else (
    set "_OLD_VIRTUAL_PATH=%PATH%"
)
set "PATH=%NODE_VIRTUAL_ENV%\Scripts;%PATH%"
rem npm.cmd runs the npm found under npm_config_prefix, which npx exports,
rem instead of its own: point it at the environment
rem https://github.com/ekalinin/nodeenv/issues/309
if defined npm_config_prefix (
    set "_OLD_VIRTUAL_NPM_CONFIG_PREFIX=%npm_config_prefix%"
)
set "npm_config_prefix=%NODE_VIRTUAL_ENV%\Scripts"
:END

"""

DEACTIVATE_BAT = """\
@echo off
if defined _OLD_VIRTUAL_PROMPT (
    set "PROMPT=%_OLD_VIRTUAL_PROMPT%"
)
set _OLD_VIRTUAL_PROMPT=
if defined _OLD_VIRTUAL_NODE_PATH (
    set "NODE_PATH=%_OLD_VIRTUAL_NODE_PATH%"
    set _OLD_VIRTUAL_NODE_PATH=
)
if defined _OLD_VIRTUAL_PATH (
    set "PATH=%_OLD_VIRTUAL_PATH%"
)
set _OLD_VIRTUAL_PATH=
if defined _OLD_VIRTUAL_NPM_CONFIG_PREFIX (
    set "npm_config_prefix=%_OLD_VIRTUAL_NPM_CONFIG_PREFIX%"
    set _OLD_VIRTUAL_NPM_CONFIG_PREFIX=
) else if defined NODE_VIRTUAL_ENV (
    set npm_config_prefix=
)
set NODE_VIRTUAL_ENV=
:END
"""

ACTIVATE_PS1 = r"""
# `nodeenv -p` appends this to the Activate.ps1 of the python virtualenv,
# which defines `deactivate` too: keep it and call it from ours instead
# of taking the name over for good
# https://github.com/ekalinin/nodeenv/issues/243
if ((Test-Path function:deactivate) -and
        -not (Test-Path function:_OLD_NODE_DEACTIVATE)) {
    copy-item function:deactivate function:global:_OLD_NODE_DEACTIVATE
}

function global:deactivate ([switch]$NonDestructive) {
    # Revert to original values
    if (Test-Path function:_OLD_VIRTUAL_PROMPT) {
        copy-item function:_OLD_VIRTUAL_PROMPT function:prompt
        remove-item function:_OLD_VIRTUAL_PROMPT
    }
    if (Test-Path env:_OLD_VIRTUAL_NODE_PATH) {
        copy-item env:_OLD_VIRTUAL_NODE_PATH env:NODE_PATH
        remove-item env:_OLD_VIRTUAL_NODE_PATH
    }
    if (Test-Path env:_OLD_VIRTUAL_PATH) {
        copy-item env:_OLD_VIRTUAL_PATH env:PATH
        remove-item env:_OLD_VIRTUAL_PATH
    }
    if (Test-Path env:_OLD_VIRTUAL_NPM_CONFIG_PREFIX) {
        copy-item env:_OLD_VIRTUAL_NPM_CONFIG_PREFIX env:npm_config_prefix
        remove-item env:_OLD_VIRTUAL_NPM_CONFIG_PREFIX
    } elseif (Test-Path env:NODE_VIRTUAL_ENV) {
        remove-item env:npm_config_prefix -ErrorAction SilentlyContinue
    }
    if (Test-Path env:NODE_VIRTUAL_ENV) {
        remove-item env:NODE_VIRTUAL_ENV
    }
    if (!$NonDestructive) {
        # Self destruct!
        remove-item function:deactivate
        # the python virtualenv's `deactivate` is next in line
        if (Test-Path function:_OLD_NODE_DEACTIVATE) {
            copy-item function:_OLD_NODE_DEACTIVATE function:global:deactivate
            remove-item function:_OLD_NODE_DEACTIVATE
            deactivate
        }
    }
}

deactivate -nondestructive
$env:NODE_VIRTUAL_ENV="__NODE_VIRTUAL_ENV__"

# Set the prompt to include the env name
# Make sure _OLD_VIRTUAL_PROMPT is global
function global:_OLD_VIRTUAL_PROMPT {""}
copy-item function:prompt function:_OLD_VIRTUAL_PROMPT
function global:prompt {
    Write-Host -NoNewline -ForegroundColor Green '__NODE_VIRTUAL_PROMPT__ '
    _OLD_VIRTUAL_PROMPT
}

# Clear NODE_PATH
if (Test-Path env:NODE_PATH) {
    copy-item env:NODE_PATH env:_OLD_VIRTUAL_NODE_PATH
    remove-item env:NODE_PATH
}

# Add the venv to the PATH
copy-item env:PATH env:_OLD_VIRTUAL_PATH
$env:PATH = "$env:NODE_VIRTUAL_ENV\Scripts;$env:PATH"

# npm.cmd runs the npm found under npm_config_prefix, which npx exports,
# instead of its own: point it at the environment
# https://github.com/ekalinin/nodeenv/issues/309
if (Test-Path env:npm_config_prefix) {
    copy-item env:npm_config_prefix env:_OLD_VIRTUAL_NPM_CONFIG_PREFIX
}
$env:npm_config_prefix = "$env:NODE_VIRTUAL_ENV\Scripts"
"""

ACTIVATE_SH = r"""

# This file must be used with "source bin/activate" *from sh*
# you cannot run it directly

deactivate_node () {
    # reset old environment variables
    if [ -n "${_OLD_NODE_VIRTUAL_PATH:-}" ] ; then
        PATH="${_OLD_NODE_VIRTUAL_PATH:-}"
        export PATH
        unset _OLD_NODE_VIRTUAL_PATH

        NODE_PATH="${_OLD_NODE_PATH:-}"
        export NODE_PATH
        unset _OLD_NODE_PATH

        NPM_CONFIG_PREFIX="${_OLD_NPM_CONFIG_PREFIX:-}"
        npm_config_prefix="${_OLD_npm_config_prefix:-}"
        export NPM_CONFIG_PREFIX
        export npm_config_prefix
        unset _OLD_NPM_CONFIG_PREFIX
        unset _OLD_npm_config_prefix
__NPM_UNISOLATE__
    fi

    # This should detect bash and zsh, which have a hash command that must
    # be called to get it to forget past commands.  Without forgetting
    # past commands the $PATH changes we made may not be respected
    if [ -n "${BASH:-}" -o -n "${ZSH_VERSION:-}" ] ; then
        hash -r
    fi

    if [ -n "${_OLD_NODE_VIRTUAL_PS1:-}" ] ; then
        PS1="${_OLD_NODE_VIRTUAL_PS1:-}"
        export PS1
        unset _OLD_NODE_VIRTUAL_PS1
    fi

    unset NODE_VIRTUAL_ENV
    if [ ! "${1:-}" = "nondestructive" ] ; then
    # Self destruct!
        unset -f deactivate_node
    fi
}

freeze () {
    local NPM_VER=`npm -v | cut -d '.' -f 1`
    if [ "$NPM_VER" = '0' ]; then
        NPM_LIST=`npm list installed active 2>/dev/null | \
                  cut -d ' ' -f 1 | grep -v npm`
    else
        local npmls="npm ls -g"
        if [ "${1:-}" = "-l" ]; then
            npmls="npm ls"
            shift
        fi
        # `path:name@version[:flags]`, one package per line.  The drawn
        # tree cannot be parsed: it loses the scope of `@scope/name`.
        # The root is the only line without `/node_modules/` in the path,
        # and npm and corepack come with node.js itself
        NPM_LIST=$(eval ${npmls} --depth=0 --parseable --long | \
                   sed -n 's|^.*/node_modules/[^:]*:\([^:]*\).*|\1|p' | \
                   grep -v -E '^(npm|corepack)@')
    fi

    if [ -z "$@" ]; then
        echo "$NPM_LIST"
    else
        echo "$NPM_LIST" > $@
    fi
}


# Detect calling this file as a script rather than sourcing it.
# bash: BASH_SOURCE[0] equals $0 when executed as a script.
# zsh:  ZSH_EVAL_CONTEXT contains :file when sourced; $0 alone matches
#       the activate path under FUNCTION_ARGZERO (default), so a $0 case
#       would false-positive on every `source bin/activate`.
# other (dash/sh): $0 is the script path when executed and the shell name
#       when sourced, so the */bin/activate case still works.
_NODEENV_RUN_AS_SCRIPT=0
if [ -n "${BASH_VERSION:-}" ]; then
    if [ "${BASH_SOURCE[0]}" = "$0" ]; then
        _NODEENV_RUN_AS_SCRIPT=1
    fi
elif [ -n "${ZSH_VERSION:-}" ]; then
    case ${ZSH_EVAL_CONTEXT:-} in
        *:file*) ;;
        *) _NODEENV_RUN_AS_SCRIPT=1 ;;
    esac
else
    case $0 in
        */bin/activate | */Scripts/activate )
            _NODEENV_RUN_AS_SCRIPT=1
            ;;
    esac
fi
if [ "$_NODEENV_RUN_AS_SCRIPT" -eq 1 ]; then
    echo "Do not call $0 directly.  Instead source it with \`source $0\`."
    exit 1
fi
unset _NODEENV_RUN_AS_SCRIPT


# unset irrelevant variables
deactivate_node nondestructive

# find the directory of this script
# http://stackoverflow.com/a/246128
if [ "${BASH_SOURCE:-}" ] ; then
    SOURCE="${BASH_SOURCE[0]}"

    while [ -h "$SOURCE" ] ; do SOURCE="$(readlink "$SOURCE")"; done
    DIR="$( command cd -P "$( dirname "$SOURCE" )" > /dev/null && pwd )"

    NODE_VIRTUAL_ENV="$(dirname "$DIR")"
else
    # dash not movable. fix use case:
    #   dash -c " . node-env/bin/activate && node -v"
    NODE_VIRTUAL_ENV="__NODE_VIRTUAL_ENV__"
fi

# NODE_VIRTUAL_ENV is the parent of the directory where this script is
export NODE_VIRTUAL_ENV

_OLD_NODE_VIRTUAL_PATH="$PATH"
PATH="$NODE_VIRTUAL_ENV/__MOD_NAME__/.bin:$NODE_VIRTUAL_ENV/__BIN_NAME__:$PATH"
export PATH

_OLD_NODE_PATH="${NODE_PATH:-}"
NODE_PATH="$NODE_VIRTUAL_ENV/__MOD_NAME__"
export NODE_PATH

_OLD_NPM_CONFIG_PREFIX="${NPM_CONFIG_PREFIX:-}"
_OLD_npm_config_prefix="${npm_config_prefix:-}"
NPM_CONFIG_PREFIX="__NPM_CONFIG_PREFIX__"
npm_config_prefix="__NPM_CONFIG_PREFIX__"
export NPM_CONFIG_PREFIX
export npm_config_prefix
__NPM_ISOLATE__

# Windows shells (git-bash, MSYS, Cygwin) run a native node.exe, which
# cannot read the posix paths built above: hand it the native ones.
# $PATH stays posix, that one is read by the shell itself.
case "$(uname -s 2>/dev/null)" in
    CYGWIN*|MSYS*|MINGW*)
        NODE_PATH="$(cygpath -w "$NODE_PATH")"
        NPM_CONFIG_PREFIX="$(cygpath -w "$NPM_CONFIG_PREFIX")"
        npm_config_prefix="$NPM_CONFIG_PREFIX"
        ;;
esac

if [ -z "${NODE_VIRTUAL_ENV_DISABLE_PROMPT:-}" ] ; then
    _OLD_NODE_VIRTUAL_PS1="${PS1:-}"
    if [ "x__NODE_VIRTUAL_PROMPT__" != x ] ; then
        PS1="__NODE_VIRTUAL_PROMPT__ ${PS1:-}"
    else
    if [ "`basename \"$NODE_VIRTUAL_ENV\"`" = "__" ] ; then
        # special case for Aspen magic directories
        # see http://www.zetadev.com/software/aspen/
        PS1="[`basename \`dirname \"$NODE_VIRTUAL_ENV\"\``] ${PS1:-}"
    else
        PS1="(`basename \"$NODE_VIRTUAL_ENV\"`) ${PS1:-}"
    fi
    fi
    export PS1
fi

# This should detect bash and zsh, which have a hash command that must
# be called to get it to forget past commands.  Without forgetting
# past commands the $PATH changes we made may not be respected
if [ -n "${BASH:-}" -o -n "${ZSH_VERSION:-}" ] ; then
    hash -r
fi
"""


ACTIVATE_FISH = """

# This file must be used with "source bin/activate.fish" *from fish*
# you cannot run it directly

function deactivate_node -d 'Exit nodeenv and return to normal environment.'
    # reset old environment variables
    if test -n "$_OLD_NODE_VIRTUAL_PATH"
        set -gx PATH $_OLD_NODE_VIRTUAL_PATH
        set -e _OLD_NODE_VIRTUAL_PATH

        # Activation saves PATH before changing the other variables, so this
        # also restores them if NODE_VIRTUAL_ENV was removed by the caller.
        if set -q _OLD_NODE_PATH
            set -gx NODE_PATH $_OLD_NODE_PATH
            set -e _OLD_NODE_PATH
        else
            set -e NODE_PATH
        end

        if set -q _OLD_NPM_CONFIG_PREFIX
            set -gx NPM_CONFIG_PREFIX $_OLD_NPM_CONFIG_PREFIX
            set -e _OLD_NPM_CONFIG_PREFIX
        else
            set -e NPM_CONFIG_PREFIX
        end

        if set -q _OLD_npm_config_prefix
            set -gx npm_config_prefix $_OLD_npm_config_prefix
            set -e _OLD_npm_config_prefix
        else
            set -e npm_config_prefix
        end
__NPM_UNISOLATE__
    end

    if test -n "$_OLD_NODE_FISH_PROMPT_OVERRIDE"
        # Set an empty local `$fish_function_path` to allow the removal of
        # `fish_prompt` using `functions -e`.
        set -l fish_function_path

        # Prevents error when using nested fish instances
        if functions -q _node_old_fish_prompt
            # Erase virtualenv's `fish_prompt` and restore the original.
            functions -e fish_prompt
            functions -c _node_old_fish_prompt fish_prompt
            functions -e _node_old_fish_prompt
        end
        set -e _OLD_NODE_FISH_PROMPT_OVERRIDE
    end

    set -e NODE_VIRTUAL_ENV

    if test (count $argv) = 0 -o "$argv[1]" != "nondestructive"
        # Self destruct!
        functions -e deactivate_node
    end
end

function freeze -d 'Show a list of installed packages - like `pip freeze`'
    set -l NPM_VER (npm -v | cut -d '.' -f 1)
    set -l NPM_LIST

    if test "$NPM_VER" = "0"
        set NPM_LIST (npm list installed active 2>/dev/null | \
                      cut -d ' ' -f 1 | grep -v npm)
    else
        set -l NPM_LS "npm ls -g"
        if test (count $argv) -gt 0 -a "$argv[1]" = "-l"
            set NPM_LS "npm ls"
            set -e argv[1]
        end
        # `path:name@version[:flags]`, one package per line.  The drawn
        # tree cannot be parsed: it loses the scope of `@scope/name`.
        # The root is the only line without `/node_modules/` in the path,
        # and npm and corepack come with node.js itself
        set NPM_LIST (eval $NPM_LS --depth=0 --parseable --long | \
            sed -n 's|^.*/node_modules/[^:]*:\\([^:]*\\).*|\\1|p' | \
            grep -v -E '^(npm|corepack)@')
    end

    if test (count $argv) = 0
        printf '%s\\n' $NPM_LIST
    else
        printf '%s\\n' $NPM_LIST > $argv[1]
    end
end

# unset irrelevant variables
deactivate_node nondestructive

# find the directory of this script
begin
    set -l SOURCE (status filename)
    while test -L "$SOURCE"
        set SOURCE (readlink "$SOURCE")
    end
    set -l DIR (dirname (realpath "$SOURCE"))

    # NODE_VIRTUAL_ENV is the parent of the directory where this script is
    set -gx NODE_VIRTUAL_ENV (dirname "$DIR")
end

set -gx _OLD_NODE_VIRTUAL_PATH $PATH
set -gx PATH "$NODE_VIRTUAL_ENV/__MOD_NAME__/.bin" "$NODE_VIRTUAL_ENV/__BIN_NAME__" $PATH

if set -q NODE_PATH
    set -gx _OLD_NODE_PATH $NODE_PATH
end
set -gx NODE_PATH "$NODE_VIRTUAL_ENV/__MOD_NAME__"

if set -q NPM_CONFIG_PREFIX
    set -gx _OLD_NPM_CONFIG_PREFIX $NPM_CONFIG_PREFIX
end
set -gx NPM_CONFIG_PREFIX "__NPM_CONFIG_PREFIX__"

if set -q npm_config_prefix
    set -gx _OLD_npm_config_prefix $npm_config_prefix
end
set -gx npm_config_prefix "__NPM_CONFIG_PREFIX__"
__NPM_ISOLATE__

if test -z "$NODE_VIRTUAL_ENV_DISABLE_PROMPT"
    # Copy the current `fish_prompt` function as `_node_old_fish_prompt`.
    functions -c fish_prompt _node_old_fish_prompt

    function fish_prompt
        # Save the current $status, for fish_prompts that display it.
        set -l old_status $status

        # Prompt override provided?
        # If not, just prepend the environment name.
        if test -n "__NODE_VIRTUAL_PROMPT__"
            printf '%s%s ' "__NODE_VIRTUAL_PROMPT__" (set_color normal)
        else
            printf '%s(%s) ' (set_color normal) (basename "$NODE_VIRTUAL_ENV")
        end

        # Restore the original $status
        echo "exit $old_status" | source
        _node_old_fish_prompt
    end

    set -gx _OLD_NODE_FISH_PROMPT_OVERRIDE "$NODE_VIRTUAL_ENV"
end
"""  # noqa: E501

PREDEACTIVATE_SH = """
if type -p deactivate_node > /dev/null; then deactivate_node;fi
"""

PREDEACTIVATE_BAT = """\
@echo off
REM Deactivate Node.js environment
if exist "%NODE_VIRTUAL_ENV%\\Scripts\\deactivate.bat" (
    call "%NODE_VIRTUAL_ENV%\\Scripts\\deactivate.bat"
)
"""

PREDEACTIVATE_PS1 = """\
# Deactivate Node.js environment
if (Get-Command deactivate -ErrorAction SilentlyContinue) {
    deactivate
}
"""

CYGWIN_NODE = """#!/bin/sh

if [ -r "$1" ]; then
    SCRIPT_PATH=$(cygpath -w "$1")
    shift
    set - $SCRIPT_PATH $@
    unset SCRIPT_PATH
fi

exec $(dirname "$0")/node.exe "$@"
"""

if __name__ == '__main__':
    main()
