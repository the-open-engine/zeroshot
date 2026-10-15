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
    git_fixture_count=$GIT_CONFIG_COUNT
    git_fixture_index=0
    while [ "$git_fixture_index" -lt "$git_fixture_count" ]; do
        eval "git_fixture_key=\${GIT_CONFIG_KEY_$git_fixture_index-}"
        case "$git_fixture_key" in
            http.*.extraheader)
                export "GIT_CONFIG_KEY_$git_fixture_index=http.$git_fixture_origin/.extraheader"
                ;;
        esac
        git_fixture_index=$((git_fixture_index + 1))
    done
    export "GIT_CONFIG_KEY_$git_fixture_count=url.$git_fixture_origin.insteadOf"
    export "GIT_CONFIG_VALUE_$git_fixture_count=https://github.com/acme/project.git"
    GIT_CONFIG_COUNT=$((git_fixture_count + 1))
    export GIT_CONFIG_COUNT
fi

exec /usr/bin/git "$@"
