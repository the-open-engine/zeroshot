#!/bin/sh

case "$2" in
repos/acme/project/pulls)
    cat "$HOME/reviews.json"
    ;;
repos/acme/project/pulls/17)
    cat "$HOME/review.json"
    ;;
repos/acme/project/git/ref/heads/zeroshot/v2-test)
    if test -f "$HOME/reference.json"; then
        cat "$HOME/reference.json"
    else
        printf '%s\n' 'gh: Not Found (HTTP 404)' >&2
        exit 1
    fi
    ;;
*)
    exit 19
    ;;
esac
