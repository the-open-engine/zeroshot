---
description: 'Set up GitHub and GitLab delivery with native CLI logins or token connections.'
---

# GitHub and GitLab delivery

GitHub.com, GitHub Enterprise, and GitLab use the same delivery nodes for PR/MR creation, merging,
and issue or review comments.

| Service                                     | Minimum version | API credential |
| ------------------------------------------- | --------------- | -------------- |
| GitHub.com / Enterprise Cloud (`*.ghe.com`) | Hosted service  | `GH_TOKEN`     |
| GitHub Enterprise Server                    | 3.22.2          | `GH_TOKEN`     |
| GitLab.com / self-managed GitLab            | 19.5.1          | `GITLAB_TOKEN` |

Instances must be publicly reachable over HTTPS with trusted TLS. Older or unidentifiable servers
fail version checks. Use a web base without a trailing slash, including any GitLab installation
prefix (such as `https://gitlab.example/install`). GitHub uses a root web URL.

## Local setup

GitHub delivery requires `gh`. GitLab needs `glab` only to reuse its native login. Sign in to the host you use:

```console
gh auth login --hostname github.com
gh auth login --hostname ghe.example
glab auth login --hostname gitlab.com
```

Local delivery reuses the selected host's login when its connection is empty. Explicit or saved
values take precedence; partial connections fail. Ambient tokens are bound to `GH_HOST` or
`GITLAB_HOST`; without a selector they apply only to GitHub.com or GitLab.com. Other hosts, including
`*.ghe.com`, require a matching selector, a per-host CLI login, or declared connection values.
Use `repo` and `read:org` for a classic GitHub PAT,
or `api` and `write_repository` for a GitLab PAT. Git fetch and push must also work.

GitHub.com and GitLab.com remotes identify themselves automatically. Other hosts need a configured
`gh`/`glab` login or explicit selection; a host configured in both CLIs needs explicit selection:

```console
zeroshot run --title "Example change" --input task.json \
  --template software-change --uniform-runtime-config runtime.json --pr \
  --source-provider source.gitlab --source-base-url https://gitlab.example/install
```

Use `source.github` for GitHub. The paired flags select the current remote's forge; they do not change
the remote. Supply them for GitLab installation prefixes.

## Docker and Platform

Contained runs need an exact Git source. Set `remote.forge` to the provider, repository and HTTPS
web base, separately from the Git locator:

```json
{
  "repository": "team/project",
  "branch": "main",
  "revision": "0123456789012345678901234567890123456789",
  "remote": {
    "url": "https://gitlab.example/install/team/project.git",
    "connection": "company-forge",
    "forge": {
      "provider": "source.gitlab",
      "repository": "team/project",
      "baseUrl": "https://gitlab.example/install"
    }
  }
}
```

The source connection supplies `GIT_USERNAME` / `GIT_PASSWORD` for HTTPS, or
`GIT_SSH_PRIVATE_KEY` / `GIT_SSH_KNOWN_HOSTS` for SSH. Declare the API token on the delivery node:

```json
{
  "kind": "git_delivery",
  "connections": { "company-forge": ["GITLAB_TOKEN"] }
}
```

Default API connections are `github.GH_TOKEN` and `gitlab.GITLAB_TOKEN`; explicit bindings keep their names.
Named targets read the declared field names for both submission and resume. Native CLI aliases such
as `GH_ENTERPRISE_TOKEN` and `GITLAB_ACCESS_TOKEN` apply only to local runs.
One static connection can hold both transport and API fields; inline run values work too. GitHub uses
`GH_TOKEN`. An API field alone does not authorize a private clone or push. Agents receive only the
connection fields their own bindings declare.

## Scope and limits

GitHub.com-only features include `--task-from`, Platform repository browsing and App/OAuth setup,
inbound commands, and hosted merge plans. For GHE or GitLab comments, supply the graph input directly.

Delivery preserves review identity across retries and confirms the integrated revision before
reporting a merge. Comment retries reuse the original publication; captured MR scopes refuse stale
base/head revisions. Required GitLab merge trains and synthetic merged-results pipelines remain
pending. Escape standalone GitLab slash commands in comments and MR descriptions.
