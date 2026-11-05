import json
import os
import sys
import warnings
from contextlib import contextmanager

from . import arguments
from . import commands
from . import git_utils
from . import jsonpath
from . import mkdocs_utils
from .app_version import version as app_version
from .mkdocs_plugin import MikePlugin

description = """
mike is a utility to make it easy to deploy multiple versions of your
MkDocs-powered docs to a Git branch, suitable for deploying to Github via
gh-pages. It's designed to produce one version of your docs at a time. That
way, you can easily deploy a new version without touching any older versions of
your docs.
"""

deploy_desc = """
Build the current documentation and deploy it to the specified version (and
aliases, if any) on the target branch.
"""

delete_desc = """
Delete the documentation for the specified versions or aliases from the target
branch. If deleting a version, that version and all its aliases will be
removed; if deleting an alias, only that alias will be removed.
"""

alias_desc = """
Add one or more new aliases to the specified version of the documentation on
the target branch.
"""

props_desc = """
Get or set properties for the specified version.

When getting or setting a particular property, you can specify it with a
limited JSONPath-like syntax. You can use bare field names, quoted field names,
and indices/field names inside square brackets. The only operator supported is
`.`. For example, this is a valid expression:

  foo."bar"[0]["baz"]

When setting values, you can add to the head or tail of a list via the `head`
or `tail` keywords, e.g.:

  foo[head]
"""

retitle_desc = """
Change the descriptive title of the specified version of the documentation on
the target branch.
"""

list_desc = """
Display a list of the currently-deployed documentation versions on the target
branch. If VERSION is specified, search for that version or alias and display
only that result.
"""

set_default_desc = """
Set the default version of the documentation on the target branch, redirecting
users from the root of the site to that version.
"""

serve_desc = """
Start the development server, serving pages from the target branch.
"""

generate_completion_desc = """
Generate shell-completion functions for bfg9000 and write them to standard
output. This requires the Python package `shtab`.
"""


def showwarning(message, category, filename, lineno, file=None, line=None):
    sys.stderr.write('warning: {}\n'.format(message))


def add_git_arguments(parser, *, commit=True, deploy_prefix=True):
    # Add this whenever we add git arguments since we pull the remote and
    # branch from mkdocs.yml.
    parser.add_argument('-F', '--config-file', metavar='FILE', complete='file',
                        help='the MkDocs configuration file to use')

    git = parser.add_argument_group('git arguments')
    git.add_argument('-r', '--remote',
                     help='origin to push to (default: origin)')
    git.add_argument('-b', '--branch',
                     help='branch to commit to (default: gh-pages)')

    if commit:
        git.add_argument('-m', '--message', help='commit message')
        git.add_argument('-p', '--push', action='store_true',
                         help='push to {remote}/{branch} after commit')
        git.add_argument('--allow-empty', action='store_true',
                         help='allow commits with no changes')

    if deploy_prefix:
        git.add_argument('--deploy-prefix', metavar='PATH',
                         complete='directory',
                         help=('subdirectory within {branch} where ' +
                               'generated docs should be deployed to'))

    git.add_argument('--ignore-remote-status', action='store_true',
                     help="don't check status of remote branch")


def add_set_prop_arguments(parser, *, prefix=''):
    def parse_set_json(expression):
        result = jsonpath.parse_set(expression)
        return result[0], json.loads(result[1])

    prop_p = parser.add_argument_group('property manipulation arguments')
    prop_p.add_argument('--{}set'.format(prefix), metavar='PROP=JSON',
                        action='append', type=parse_set_json, dest='set_props',
                        help='set the property at PROP to a JSON value')
    prop_p.add_argument('--{}set-string'.format(prefix), metavar='PROP=STRING',
                        action='append', type=jsonpath.parse_set,
                        dest='set_props',
                        help='set the property at PROP to a STRING value')
    prop_p.add_argument('--{}set-all'.format(prefix), metavar='JSON',
                        action='append', type=lambda x: ('', json.loads(x)),
                        dest='set_props',
                        help='set all properties to a JSON value')
    prop_p.add_argument('--{}delete'.format(prefix), metavar='PROP',
                        action='append',
                        type=lambda x: (jsonpath.parse(x), jsonpath.Deleted),
                        dest='set_props', help='delete the property at PROP')
    prop_p.add_argument('--{}delete-all'.format(prefix),
                        action='append_const', const=('', jsonpath.Deleted),
                        dest='set_props', help='delete all properties')
    return prop_p


def load_mkdocs_config(args, strict=False):
    def maybe_set(args, cfg, field, cfg_field=None):
        if getattr(args, field, object()) is None:
            setattr(args, field, cfg[cfg_field or field])

    try:
        cfg = mkdocs_utils.load_config(args.config_file)
        plugin = cfg['plugins'].get('mike') or MikePlugin.default()

        maybe_set(args, cfg, 'branch', 'remote_branch')
        maybe_set(args, cfg, 'remote', 'remote_name')
        maybe_set(args, plugin.config, 'alias_type')
        maybe_set(args, plugin.config, 'template', 'redirect_template')
        maybe_set(args, plugin.config, 'deploy_prefix')
        return cfg
    except FileNotFoundError as e:
        if strict:
            raise

        plugin = MikePlugin.default()
        maybe_set(args, plugin.config, 'alias_type')
        maybe_set(args, plugin.config, 'template', 'redirect_template')
        maybe_set(args, plugin.config, 'deploy_prefix')
        if args.branch is None or args.remote is None:
            raise FileNotFoundError(
                '{}; pass --config-file or set --remote/--branch explicitly'
                .format(str(e))
            )


def check_remote_status(args, strict=False):
    if args.ignore_remote_status:
        return

    try:
        git_utils.update_from_upstream(args.remote, args.branch)
    except (git_utils.GitBranchDiverged, git_utils.GitRevUnrelated) as e:
        if strict:
            raise ValueError(str(e) + "\n  If you're sure this is intended, " +
                             'retry with --ignore-remote-status')
        else:
            warnings.warn(str(e))


@contextmanager
def handle_empty_commit():
    try:
        yield
    except git_utils.GitEmptyCommit as e:
        warnings.warn(str(e) + '\n  To create a commit anyway, retry with ' +
                      '--allow-empty')


def deploy(parser, args):
    cfg = load_mkdocs_config(args, strict=True)
    check_remote_status(args, strict=True)
    with handle_empty_commit():
        alias_type = commands.AliasType[args.alias_type]
        with commands.deploy(cfg, args.version, args.title, args.aliases,
                             args.update_aliases, alias_type, args.template,
                             branch=args.branch, message=args.message,
                             allow_empty=args.allow_empty,
                             deploy_prefix=args.deploy_prefix,
                             set_props=args.set_props or []), \
             mkdocs_utils.inject_plugin(args.config_file) as config_file:
            mkdocs_utils.build(config_file, args.version, quiet=args.quiet)
        if args.push:
            git_utils.push_branch(args.remote, args.branch)


def delete(parser, args):
    load_mkdocs_config(args)
    check_remote_status(args, strict=True)
    commands.delete(args.identifiers, args.all, branch=args.branch,
                    message=args.message, allow_empty=args.allow_empty,
                    deploy_prefix=args.deploy_prefix)
    if args.push:
        git_utils.push_branch(args.remote, args.branch)


def alias(parser, args):
    cfg = load_mkdocs_config(args)
    check_remote_status(args, strict=True)
    with handle_empty_commit():
        alias_type = commands.AliasType[args.alias_type]
        commands.alias(cfg, args.identifier, args.aliases, args.update_aliases,
                       alias_type, args.template, branch=args.branch,
                       message=args.message, allow_empty=args.allow_empty,
                       deploy_prefix=args.deploy_prefix)
        if args.push:
            git_utils.push_branch(args.remote, args.branch)


def props(parser, args):
    load_mkdocs_config(args)
    check_remote_status(args, strict=args.set_props)

    if args.get_prop and args.set_props:
        raise ValueError('cannot get and set properties at the same time')
    elif args.set_props:
        with handle_empty_commit():
            commands.set_properties(args.identifier, args.set_props,
                                    branch=args.branch, message=args.message,
                                    allow_empty=args.allow_empty,
                                    deploy_prefix=args.deploy_prefix)
            if args.push:
                git_utils.push_branch(args.remote, args.branch)
    else:
        print(json.dumps(
            commands.get_property(args.identifier, args.get_prop,
                                  branch=args.branch,
                                  deploy_prefix=args.deploy_prefix)
        ))


def retitle(parser, args):
    load_mkdocs_config(args)
    check_remote_status(args, strict=True)
    with handle_empty_commit():
        commands.retitle(args.identifier, args.title, branch=args.branch,
                         message=args.message, allow_empty=args.allow_empty,
                         deploy_prefix=args.deploy_prefix)
        if args.push:
            git_utils.push_branch(args.remote, args.branch)


def list_versions(parser, args):
    def print_version(info):
        version = str(info.version)
        aliases = (' [{}]'.format(', '.join(sorted(info.aliases)))
                   if info.aliases else '')
        if info.title != version:
            print('"{title}" ({version}){aliases}'.format(
                title=info.title, version=version, aliases=aliases
            ))
        else:
            print('{version}{aliases}'.format(
                version=version, aliases=aliases
            ))

    load_mkdocs_config(args)
    check_remote_status(args)
    all_versions = commands.list_versions(args.branch, args.deploy_prefix)

    if args.identifier:
        try:
            key = all_versions.find(args.identifier, strict=True)
            info = all_versions[key[0]]
            if args.json:
                print(info.dumps())
            else:
                print_version(info)
        except KeyError as e:
            raise ValueError('identifier {} does not exist'.format(e))
    elif args.json:
        print(all_versions.dumps())
    else:
        for i in all_versions:
            print_version(i)


def set_default(parser, args):
    load_mkdocs_config(args)
    check_remote_status(args, strict=True)
    with handle_empty_commit():
        commands.set_default(args.identifier, args.template,
                             args.allow_undefined, branch=args.branch,
                             message=args.message,
                             allow_empty=args.allow_empty,
                             deploy_prefix=args.deploy_prefix)
        if args.push:
            git_utils.push_branch(args.remote, args.branch)


def serve(parser, args):
    load_mkdocs_config(args)
    check_remote_status(args)
    commands.serve(args.dev_addr, branch=args.branch)


def help(parser, args):
    parser.parse_args(args.subcommand + ['--help'])


def generate_completion(parser, args):
    try:
        import shtab
        print(shtab.complete(parser, shell=args.shell))
    except ImportError:  # pragma: no cover
        print('shtab not found; install via `pip install shtab`')
        return 1


def main():
    warnings.showwarning = showwarning

    parser = arguments.ArgumentParser(prog='mike', description=description)
    subparsers = parser.add_subparsers(metavar='COMMAND')
    subparsers.required = True

    parser.add_argument('--version', action='version',
                        version='%(prog)s ' + app_version)
    parser.add_argument('-q', '--quiet', action='store_true',
                        help='silence warnings')
    parser.add_argument('--debug', action='store_true',
                        help='report extra information for debugging mike')

    deploy_p = subparsers.add_parser(
        'deploy', description=deploy_desc,
        help='build docs and deploy them to a branch'
    )
    deploy_p.set_defaults(func=deploy)
    deploy_p.add_argument('-t', '--title',
                          help='short descriptive title for this version')
    deploy_p.add_argument('-u', '--update-aliases', action='store_true',
                          help='update aliases pointing to other versions')
    deploy_p.add_argument('--alias-type', metavar='TYPE',
                          choices=[i.name for i in commands.AliasType],
                          help=('method for creating aliases (one of: ' +
                                '%(choices)s; default: symlink)'))
    deploy_p.add_argument('-T', '--template', complete='file',
                          help='template file to use for redirects')
    add_git_arguments(deploy_p)
    add_set_prop_arguments(deploy_p, prefix='prop-')
    deploy_p.add_argument('version', metavar='VERSION',
                          help='version to deploy this build to')
    deploy_p.add_argument('aliases', nargs='*', metavar='ALIAS',
                          help='additional alias for this build')

    delete_p = subparsers.add_parser(
        'delete', description=delete_desc, help='delete docs from a branch'
    )
    delete_p.set_defaults(func=delete)
    delete_p.add_argument('--all', action='store_true',
                          help='delete everything')
    add_git_arguments(delete_p)
    delete_p.add_argument('identifiers', nargs='*', metavar='IDENTIFIER',
                          help='version or alias to delete')

    alias_p = subparsers.add_parser(
        'alias', description=alias_desc, help='alias docs on a branch'
    )
    alias_p.set_defaults(func=alias)
    alias_p.add_argument('-u', '--update-aliases', action='store_true',
                         help='update aliases pointing to other versions')
    alias_p.add_argument('--alias-type', metavar='TYPE',
                         choices=[i.name for i in commands.AliasType],
                         help=('method for creating aliases (one of: ' +
                               '%(choices)s; default: symlink)'))
    alias_p.add_argument('-T', '--template', complete='file',
                         help='template file to use for redirects')
    add_git_arguments(alias_p)
    alias_p.add_argument('identifier', metavar='IDENTIFIER',
                         help='existing version or alias')
    alias_p.add_argument('aliases', nargs='*', metavar='ALIAS',
                         help='new alias to add')

    props_p = subparsers.add_parser(
        'props', description=props_desc, help='get/set version properties',
        formatter_class=arguments.ParagraphDescriptionHelpFormatter
    )
    props_p.set_defaults(func=props)
    add_git_arguments(props_p)
    add_set_prop_arguments(props_p)
    props_p.add_argument('identifier', metavar='IDENTIFIER',
                         help='existing version or alias')
    props_p.add_argument('get_prop', nargs='?', metavar='PROP', default='',
                         help='property to get')

    retitle_p = subparsers.add_parser(
        'retitle', description=retitle_desc,
        help='change the title of a version'
    )
    retitle_p.set_defaults(func=retitle)
    add_git_arguments(retitle_p)
    retitle_p.add_argument('identifier', metavar='IDENTIFIER',
                           help='version or alias to retitle')
    retitle_p.add_argument('title', metavar='TITLE', help='new title to use')

    list_p = subparsers.add_parser(
        'list', description=list_desc, help='list deployed docs on a branch'
    )
    list_p.set_defaults(func=list_versions)
    list_p.add_argument('-j', '--json', action='store_true',
                        help='display the result as JSON')
    add_git_arguments(list_p, commit=False)
    list_p.add_argument('identifier', metavar='IDENTIFIER', nargs='?',
                        help='optional version or alias to search for')

    set_default_p = subparsers.add_parser(
        'set-default', description=set_default_desc,
        help='set the default version for your docs'
    )
    set_default_p.set_defaults(func=set_default)
    set_default_p.add_argument('-T', '--template', complete='file',
                               help='template file to use')
    set_default_p.add_argument('--allow-undefined', action='store_true',
                               help=('allow setting undefined versions as ' +
                                     'default'))
    add_git_arguments(set_default_p)
    set_default_p.add_argument('identifier', metavar='IDENTIFIER',
                               help='version or alias to set as default')

    serve_p = subparsers.add_parser(
        'serve', description=serve_desc, help='serve docs locally for testing'
    )
    serve_p.set_defaults(func=serve)
    add_git_arguments(serve_p, commit=False, deploy_prefix=False)
    serve_p.add_argument('-a', '--dev-addr', default='localhost:8000',
                         metavar='HOST[:PORT]',
                         help=('Host address and port to serve from ' +
                               '(default: %(default)s)'))

    help_p = subparsers.add_parser(
        'help', help='show this help message and exit', add_help=False
    )
    help_p.set_defaults(func=help)
    help_p.add_argument('subcommand', metavar='CMD', nargs=arguments.REMAINDER,
                        help='subcommand to request help for')

    completion_p = subparsers.add_parser(
        'generate-completion', description=generate_completion_desc,
        help='print shell completion script'
    )
    completion_p.set_defaults(func=generate_completion)
    shell = (os.path.basename(os.environ['SHELL'])
             if 'SHELL' in os.environ else None)
    completion_p.add_argument('-s', '--shell', metavar='SHELL', default=shell,
                              help='shell type (default: %(default)s)')

    args = parser.parse_args()
    if args.quiet:
        warnings.filterwarnings('ignore')

    try:
        return args.func(parser, args)
    except Exception as e:
        if args.debug:  # pragma: no cover
            raise
        parser.exit(1, 'error: {}\n'.format(e))
