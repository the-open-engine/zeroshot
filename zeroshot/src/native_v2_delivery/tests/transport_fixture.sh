#!/bin/sh
# Test-only immutable launcher: real Git HTTP transport plus bare-repository API metadata.
# Per-test state lives in the workspace config and HOME, never in this executable.
if [ "$1" = "api" ]; then
    for git_fixture_argument in "$@"; do
        case "$git_fixture_argument" in
            repos/acme/project/git/ref/heads/*)
                git_fixture_branch=$(printf '%s' "$git_fixture_argument" |
                    /usr/bin/sed 's|^repos/acme/project/git/ref/heads/||')
                git_fixture_revision=$(/usr/bin/git --git-dir="$HOME/remote.git" \
                    rev-parse --verify "refs/heads/$git_fixture_branch" 2>/dev/null) || {
                    echo 'HTTP 404: reference absent' >&2
                    exit 1
                }
                printf '{"ref":"refs/heads/%s","object":{"type":"commit","sha":"%s"}}' \
                    "$git_fixture_branch" "$git_fixture_revision"
                exit 0
                ;;
        esac
    done
    echo 'Unexpected metadata operation' >&2
    exit 1
fi

if [ -n "$GIT_CONFIG_COUNT" ]; then
    git_fixture_previous=''
    git_fixture_workspace=''
    for git_fixture_argument in "$@"; do
        if [ "$git_fixture_previous" = "-C" ]; then
            git_fixture_workspace=$git_fixture_argument
            break
        fi
        git_fixture_previous=$git_fixture_argument
    done
    test -n "$git_fixture_workspace" || exit 2
    git_fixture_origin=$(/usr/bin/git -C "$git_fixture_workspace" \
        config --local --get zeroshotTest.httpOrigin) || exit 2
    GIT_CONFIG_KEY_1="http.$git_fixture_origin/.extraheader"
    GIT_CONFIG_COUNT=3
    GIT_CONFIG_KEY_2="url.$git_fixture_origin.insteadOf"
    GIT_CONFIG_VALUE_2='https://github.com/acme/project.git'
    export GIT_CONFIG_KEY_1 GIT_CONFIG_COUNT GIT_CONFIG_KEY_2 GIT_CONFIG_VALUE_2
fi

exec /usr/bin/git "$@"
