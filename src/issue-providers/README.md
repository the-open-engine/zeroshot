# Issue Providers

Multi-platform issue support for Zeroshot. Fetch issues from GitHub, GitLab, Jira, Azure DevOps, and Linear.

## Supported Providers

| Provider         | CLI Tool               | URL Pattern                                  | Issue Key Format      |
| ---------------- | ---------------------- | -------------------------------------------- | --------------------- |
| **GitHub**       | `gh`                   | `github.com/org/repo/issues/123`             | `123`, `org/repo#123` |
| **GitLab**       | `glab`                 | `gitlab.com/org/repo/-/issues/123`           | `123`, `org/repo#123` |
| **Jira**         | `jira`                 | `*.atlassian.net/browse/KEY-123`             | `KEY-123`             |
| **Azure DevOps** | `az`                   | `dev.azure.com/org/proj/_workitems/edit/123` | `123`                 |
| **Linear**       | _(none — GraphQL API)_ | `linear.app/ws/issue/ENG-42`                 | `ENG-42`              |

## Quick Start

```bash
# GitHub
zeroshot run 123
zeroshot run https://github.com/org/repo/issues/123
zeroshot run org/repo#123

# GitLab
zeroshot run https://gitlab.com/org/repo/-/issues/123
zeroshot run 123 --gitlab

# Jira
zeroshot run PROJ-123
zeroshot run https://company.atlassian.net/browse/PROJ-123

# Azure DevOps
zeroshot run https://dev.azure.com/org/project/_workitems/edit/123
zeroshot run 123 --devops

# Linear
zeroshot run ENG-42
zeroshot run https://linear.app/workspace/issue/ENG-42/some-title
zeroshot run 42 --linear
```

## Automatic Git Remote Detection

When working in a git repository, zeroshot automatically detects the issue provider from your git remote URL:

```bash
# In a GitHub repository
git remote get-url origin  # → https://github.com/org/repo.git
zeroshot run 123            # Automatically uses GitHub

# In a GitLab repository
git remote get-url origin  # → https://gitlab.com/org/repo.git
zeroshot run 456            # Automatically uses GitLab

# In an Azure DevOps repository
git remote get-url origin  # → https://dev.azure.com/org/project/_git/repo
zeroshot run 789            # Automatically uses Azure DevOps
```

## Force Flags

Override auto-detection with explicit provider flags:

```bash
-G, --github    # Force GitHub as issue source
-L, --gitlab    # Force GitLab as issue source
-J, --jira      # Force Jira as issue source
-D, --devops    # Force Azure DevOps as issue source
-N, --linear    # Force Linear as issue source
```

**Example:**

```bash
# Force GitLab even when in a GitHub repo
zeroshot run 123 --gitlab

# Force Jira for bare number
zeroshot run 456 --jira
```

## Detection Priority

For bare issue numbers (e.g., `123`), zeroshot uses this priority:

1. **Force flag** (`--github`, `--gitlab`, etc.) - Explicit CLI override
2. **Git remote URL** - Auto-detected from your repository
3. **Settings** (`defaultIssueSource`) - User preference
4. **Legacy fallback** - GitHub (when no git context and no settings)

For URLs and issue keys (like `PROJ-123`), the provider is detected from the format.

## Settings

Configure default provider and platform-specific settings:

```bash
# Set default provider for bare numbers
zeroshot settings set defaultIssueSource gitlab

# GitLab self-hosted instance
zeroshot settings set gitlabInstance gitlab.company.com

# Jira configuration
zeroshot settings set jiraInstance jira.company.com
zeroshot settings set jiraProject MYPROJECT  # Default project for bare numbers

# Azure DevOps configuration
zeroshot settings set azureOrg mycompany
zeroshot settings set azureProject myproject  # Default project for bare numbers

# Linear configuration
zeroshot settings set linearApiKey lin_api_...  # API key (falls back to LINEAR_API_KEY env var)
zeroshot settings set linearTeam ENG  # Default team key for bare numbers (auto-derived on first use if unset)
```

### Settings Reference

| Setting              | Type   | Default | Description                                                                                         |
| -------------------- | ------ | ------- | --------------------------------------------------------------------------------------------------- |
| `defaultIssueSource` | string | github  | Provider for bare numbers (123)                                                                     |
| `gitlabInstance`     | string | null    | Self-hosted GitLab URL                                                                              |
| `jiraInstance`       | string | null    | Self-hosted Jira URL                                                                                |
| `jiraProject`        | string | null    | Default Jira project key for bare numbers                                                           |
| `azureOrg`           | string | null    | Azure DevOps organization name                                                                      |
| `azureProject`       | string | null    | Azure DevOps project name for bare numbers                                                          |
| `linearApiKey`       | string | null    | Linear personal API key (falls back to `LINEAR_API_KEY` env var)                                    |
| `linearTeam`         | string | null    | Default Linear team key for bare numbers (auto-derived from the workspace if unset and unambiguous) |

## CLI Tool Setup

Each provider requires its corresponding CLI tool:

### GitHub

```bash
# Install
brew install gh              # macOS
apt install gh               # Linux
# Or: https://cli.github.com/

# Authenticate
gh auth login
gh auth status
```

### GitLab

```bash
# Install
brew install glab            # macOS
# Or: https://gitlab.com/gitlab-org/cli

# Authenticate
glab auth login
glab auth status
```

### Jira

```bash
# Install go-jira
brew install go-jira         # macOS
# Or: https://github.com/go-jira/jira

# Configure
jira login
jira version
```

### Azure DevOps

```bash
# Install Azure CLI
brew install azure-cli       # macOS
# Or: https://docs.microsoft.com/cli/azure/

# Configure
az login
az devops configure --defaults organization=https://dev.azure.com/yourorg
```

### Linear

No CLI install needed — Linear is accessed directly via its GraphQL API.

```bash
# Create a personal API key at https://linear.app/settings/api
zeroshot settings set linearApiKey lin_api_...

# Or, as a fallback, export it as an env var (checked if linearApiKey is unset):
export LINEAR_API_KEY=lin_api_...
```

Note: `KEY-NUMBER` input (e.g. `ENG-42`) is ambiguous between Jira and Linear.
It's routed to Linear automatically when Linear is configured
(`linearApiKey`/`linearTeam`) and Jira isn't; otherwise Jira wins. Use
`--linear`/`--jira` to force a specific provider.

## Self-Hosted Instances

### GitLab Self-Hosted

```bash
# Configure self-hosted instance
zeroshot settings set gitlabInstance gitlab.company.com

# Now these work
zeroshot run https://gitlab.company.com/org/repo/-/issues/123
zeroshot run 123 --gitlab  # Uses self-hosted instance
```

### Jira Self-Hosted (Server/Data Center)

```bash
# Configure self-hosted instance and default project
zeroshot settings set jiraInstance jira.company.com
zeroshot settings set jiraProject MYPROJ

# Now these work
zeroshot run https://jira.company.com/browse/MYPROJ-123
zeroshot run MYPROJ-123
zeroshot run 123 --jira  # Becomes MYPROJ-123
```

## Examples

### Team Using GitLab

```bash
# Set once per machine
zeroshot settings set defaultIssueSource gitlab

# Then just use bare numbers
zeroshot run 456
```

### Team Using Jira

```bash
# Set once per machine
zeroshot settings set defaultIssueSource jira
zeroshot settings set jiraProject MYTEAM

# Then just use bare numbers (becomes MYTEAM-789)
zeroshot run 789
```

### Mixed Team

```bash
# Use explicit URLs or flags when working across platforms
zeroshot run https://github.com/org/repo/issues/100
zeroshot run https://gitlab.com/org/repo/-/issues/200
zeroshot run PROJ-300
```

### CI/CD Integration

```yaml
# .github/workflows/zeroshot.yml
- name: Run Zeroshot on Issue
  run: zeroshot run ${{ github.event.issue.html_url }} --ship

# .gitlab-ci.yml
- script: zeroshot run $CI_MERGE_REQUEST_IID --gitlab --ship

# Azure Pipelines
- script: zeroshot run $(System.WorkItemId) --devops --ship
```

## Troubleshooting

### Provider Not Detected

**Problem:** Bare number not detected as expected provider

```bash
$ zeroshot run 123
# Fetches from GitHub, but I wanted GitLab
```

**Solution:** Set `defaultIssueSource` or use force flag

```bash
zeroshot settings set defaultIssueSource gitlab
# OR
zeroshot run 123 --gitlab
```

### CLI Tool Not Installed

**Problem:** Preflight check fails

```bash
PREFLIGHT CHECK FAILED
GitLab CLI (glab) not installed
```

**Solution:** Install the required CLI tool

```bash
brew install glab
glab auth login
```

### Self-Hosted Instance Not Recognized

**Problem:** Self-hosted URL not detected

```bash
$ zeroshot run https://gitlab.mycompany.com/org/repo/-/issues/123
Error: No issue provider matched input
```

**Solution:** Configure the instance setting

```bash
zeroshot settings set gitlabInstance gitlab.mycompany.com
```

### Jira Bare Numbers Don't Work

**Problem:** Bare numbers don't convert to Jira keys

```bash
$ zeroshot run 123 --jira
Error: Failed to fetch Jira issue
```

**Solution:** Configure `jiraProject` setting

```bash
zeroshot settings set jiraProject MYPROJECT
# Now: 123 becomes MYPROJECT-123
```
