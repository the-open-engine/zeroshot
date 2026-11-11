#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

assert os.environ.get('GH_TOKEN') == 'test-token'
path = Path(sys.argv[0] + '.state')
state = json.loads(path.read_text())
args = sys.argv[1:]
endpoint = args[1]
review = json.loads(Path(sys.argv[0] + '.review').read_text())
repo = review['base']['repo']['full_name']
prefix = f'repos/{repo}'
state.setdefault('calls', []).append(args)
query = next((arg[6:] for arg in args if arg.startswith('query=')), '')
threads = state['threads']
if state.get('move_positions'):
    for thread in threads:
        for comment in thread['comments']:
            comment['commit_id'] = review['head']['sha']
            comment['line'] = len(state['calls'])

def save():
    path.write_text(json.dumps(state))

def emit(value):
    save()
    print(json.dumps(value))
    sys.exit(0)

def unavailable():
    save()
    print('HTTP 503: resolution confirmation unavailable', file=sys.stderr)
    sys.exit(1)

if endpoint == f'{prefix}/pulls/17':
    if state.get('resolve_started') and state.get('fail_confirmation_reads', 0) > 0:
        state['fail_confirmation_reads'] -= 1
        unavailable()
    emit(review)
if endpoint == f'{prefix}/pulls/17/comments?per_page=100':
    emit([[comment for thread in threads for comment in thread['comments']]])
if endpoint in (f'{prefix}/issues/17/comments?per_page=100', f'{prefix}/pulls/17/reviews?per_page=100'):
    emit([[]])
if endpoint == f'{prefix}/rules/branches/main?per_page=100':
    emit([[]])
if endpoint.startswith(f'{prefix}/collaborators/'):
    login = endpoint.split('/')[-2]
    author = next((comment['user'] for thread in threads for comment in thread['comments'] if comment['user']['login'] == login), None)
    if author is None:
        author = {'id': 7, 'login': login, 'type': 'User'}
    emit({'permission': 'write' if author['id'] == 900 else state.get('permission', 'read'), 'user': author})
if endpoint != 'graphql':
    raise RuntimeError(endpoint)
if 'ReviewConversations' in query:
    nodes = [{'id': thread['id'], 'isResolved': thread['resolved'], 'comments': {'totalCount': len(thread['comments']), 'nodes': [{'fullDatabaseId': str(thread['comments'][0]['id'])}]}} for thread in threads]
    emit([{'data': {'repository': {'nameWithOwner': repo, 'pullRequest': {
        'number': 17, 'baseRefName': review['base']['ref'], 'headRefName': review['head']['ref'], 'headRefOid': review['head']['sha'],
        'reviewThreads': {'totalCount': len(nodes), 'pageInfo': {'hasNextPage': False, 'endCursor': None}, 'nodes': nodes}
    }}}}])
thread_id = next((arg[7:] for arg in args if arg.startswith('thread=')), None)
thread = next((thread for thread in threads if thread['id'] == thread_id), None)
if 'addPullRequestReviewThreadReply' in query:
    body = next(arg[5:] for arg in args if arg.startswith('body='))
    reply = dict(thread['comments'][0])
    reply.update(id=max(900, max(comment['id'] for entry in threads for comment in entry['comments'])) + 1, user={'id': 900, 'login': 'delivery', 'type': 'User'}, body=body, in_reply_to_id=thread['comments'][0]['id'])
    thread['comments'].append(reply)
    if state.get('race') == 'edit_after_reply':
        thread['comments'][0]['body'] += ' Additional finding.'
    if state.get('race') == 'reply_after_reply':
        extra = dict(thread['comments'][0])
        extra.update(id=800, in_reply_to_id=thread['comments'][0]['id'], body='Another substantive finding.')
        thread['comments'].append(extra)
    emit({'data': {'addPullRequestReviewThreadReply': {'comment': {'fullDatabaseId': str(reply['id'])}}}})
if 'resolveReviewThread' in query:
    unresolve = 'unresolveReviewThread' in query
    thread['resolved'] = not unresolve
    if not unresolve:
        state['resolve_started'] = True
    if not unresolve and state.get('race') == 'edit_after_resolve':
        thread['comments'][0]['body'] += ' Edited during resolution.'
        state['race'] = None
    if not unresolve and state.get('race') == 'unauthorized_reply_after_resolve':
        extra = dict(thread['comments'][0])
        extra.update(id=950, in_reply_to_id=thread['comments'][0]['id'], body='Unauthorized request.', user={'id': 9, 'login': 'reader', 'type': 'User'})
        thread['comments'].append(extra)
        state['race'] = None
    if state.pop('lose_reopen_response' if unresolve else 'lose_resolve_response', False):
        unavailable()
    emit({'data': {'unresolveReviewThread' if unresolve else 'resolveReviewThread': {'thread': {'id': thread_id, 'isResolved': not unresolve}}}})
if 'enqueuePullRequest' in query:
    state['threads'][0]['resolved'] = False
    save()
    print(json.dumps({'data': {'enqueuePullRequest': None}, 'errors': [{'type': 'UNPROCESSABLE', 'path': ['enqueuePullRequest'], 'message': 'A conversation must be resolved before this pull request can be merged.'}]}))
    sys.exit(1)
if 'dequeuePullRequest' in query or 'disablePullRequestAutoMerge' in query:
    emit({'data': {'dequeuePullRequest': {'clientMutationId': None}, 'disablePullRequestAutoMerge': {'clientMutationId': None}}})
if 'policy' in state:
    emit([state['policy']])
raise RuntimeError(query)
