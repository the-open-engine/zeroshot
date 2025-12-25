# NPM Publishing Setup for @covibes/zeroshot

This document explains how to set up automated NPM publishing using semantic-release and GitHub Actions.

## Prerequisites

Before you can publish the package, you need to:

1. **Create the @covibes npm organization** (if it doesn't exist yet)
2. **Generate an NPM automation token**
3. **Add the token to GitHub Secrets**

## Step 1: Create the @covibes npm Organization

The package name `@covibes/zeroshot` uses the `@covibes` scope, which requires an npm organization.

### Check if the organization exists:

```bash
npm org ls @covibes
```

If you get an error or "organization not found", you need to create it:

### Create the organization:

1. Log in to npm:
   ```bash
   npm login
   ```

2. Visit https://www.npmjs.com/org/create

3. Create an organization named `covibes`

4. Choose the organization type:
   - **Free** (for public packages only)
   - **Paid** (if you need private packages)

5. Verify the organization exists:
   ```bash
   npm org ls @covibes
   ```

## Step 2: Generate an NPM Automation Token

For CI/CD publishing, you need an **automation token** (not a classic token).

### Generate the token:

1. Log in to npmjs.com

2. Navigate to **Access Tokens** page:
   - Click your profile icon → "Access Tokens"
   - OR visit: https://www.npmjs.com/settings/[your-username]/tokens

3. Click **"Generate New Token"** → Select **"Automation"**

4. Name the token: `GitHub Actions - zeroshot`

5. Set the token type to **"Automation"**

6. **Copy the token immediately** (you won't be able to see it again)

### Token Permissions:

Automation tokens have these capabilities:
- ✅ Publish packages
- ✅ Update package metadata
- ✅ Read private packages (if your org has any)
- ❌ Cannot bypass 2FA prompts
- ❌ Cannot be used for `npm login`

## Step 3: Add NPM_TOKEN to GitHub Secrets

The semantic-release workflow expects an `NPM_TOKEN` secret.

### Add the secret:

1. Go to your GitHub repository: https://github.com/covibes/zeroshot

2. Navigate to: **Settings → Secrets and variables → Actions**

3. Click **"New repository secret"**

4. Set:
   - **Name:** `NPM_TOKEN`
   - **Value:** [paste the automation token from Step 2]

5. Click **"Add secret"**

### Verify the secret:

The secret should now appear in the list as `NPM_TOKEN` (value hidden).

## Step 4: Verify Package Configuration

The package.json is already configured correctly:

```json
{
  "name": "@covibes/zeroshot",
  "version": "0.1.0",
  "publishConfig": {
    "access": "public",
    "registry": "https://registry.npmjs.org/"
  }
}
```

### Key settings:

- **`"access": "public"`** - Required for scoped packages to be public
- **`"registry"`** - Explicit npm registry URL
- **`"name"`** - Scoped package name with @covibes org

## Step 5: Test Publishing Locally (Optional)

Before relying on CI/CD, test publishing manually:

### Dry run:

```bash
npm publish --dry-run
```

This shows what would be published without actually publishing.

### Manual publish (first time):

```bash
npm login
npm publish
```

For subsequent releases, semantic-release will handle this automatically.

## Step 6: How Automated Publishing Works

Once the `NPM_TOKEN` secret is configured, publishing happens automatically:

### Trigger a release:

1. **Make changes** to the codebase

2. **Commit with conventional commit messages**:
   ```bash
   git commit -m "feat: add new feature"      # Minor version bump (0.1.0 → 0.2.0)
   git commit -m "fix: fix bug"               # Patch version bump (0.1.0 → 0.1.1)
   git commit -m "feat!: breaking change"     # Major version bump (0.1.0 → 1.0.0)
   ```

3. **Push to main branch**:
   ```bash
   git push origin main
   ```

4. **GitHub Actions runs** the release workflow:
   - Analyzes commit messages
   - Determines version bump
   - Updates CHANGELOG.md
   - Creates a GitHub release
   - Publishes to npm

### Check the release:

- **GitHub**: https://github.com/covibes/zeroshot/releases
- **npm**: https://www.npmjs.com/package/@covibes/zeroshot

## Conventional Commit Format

semantic-release uses conventional commits to determine version bumps:

| Commit Type | Version Bump | Example |
|-------------|-------------|---------|
| `fix:` | Patch (0.1.0 → 0.1.1) | `fix: resolve memory leak` |
| `feat:` | Minor (0.1.0 → 0.2.0) | `feat: add cluster resume` |
| `feat!:` or `BREAKING CHANGE:` | Major (0.1.0 → 1.0.0) | `feat!: change API signature` |
| `docs:`, `chore:`, `style:`, etc. | No release | `docs: update README` |

### Breaking changes:

Use `!` after the type or include `BREAKING CHANGE:` in the commit body:

```bash
git commit -m "feat!: remove deprecated API"

# OR

git commit -m "feat: new API" -m "BREAKING CHANGE: removes old API"
```

## Troubleshooting

### Error: "npm ERR! 404 Not Found - PUT https://registry.npmjs.org/@covibes%2fzeroshot"

**Cause:** The @covibes organization doesn't exist.

**Fix:** Create the organization (see Step 1).

### Error: "npm ERR! 403 Forbidden"

**Cause:** The NPM_TOKEN doesn't have permission to publish to @covibes.

**Fix:**
1. Verify you're a member of the @covibes npm organization
2. Regenerate the automation token
3. Update the GitHub secret

### Error: "npm ERR! need auth This command requires you to be logged in"

**Cause:** The NPM_TOKEN secret is missing or invalid.

**Fix:** Verify the secret exists in GitHub Settings → Secrets → Actions.

### No release created

**Cause:** Commits don't follow conventional commit format.

**Fix:** Use `feat:`, `fix:`, or other conventional commit types.

## Manual Publishing (Emergency)

If GitHub Actions fails and you need to publish manually:

```bash
# Login to npm
npm login

# Update version (semantic-release normally does this)
npm version patch   # or 'minor' or 'major'

# Publish
npm publish

# Push the version commit and tag
git push origin main --follow-tags
```

## Security Best Practices

1. ✅ **Use automation tokens** (not classic tokens)
2. ✅ **Store tokens in GitHub Secrets** (never commit them)
3. ✅ **Enable 2FA** on your npm account
4. ✅ **Rotate tokens** periodically (e.g., every 6 months)
5. ✅ **Revoke tokens** immediately if compromised

## Next Steps

1. ✅ Create @covibes npm organization (if needed)
2. ✅ Generate NPM automation token
3. ✅ Add NPM_TOKEN to GitHub Secrets
4. ✅ Make a commit with `feat:` or `fix:`
5. ✅ Push to main branch
6. ✅ Watch GitHub Actions run the release
7. ✅ Verify package published to npm

## Resources

- [npm Organizations](https://docs.npmjs.com/organizations)
- [npm Tokens](https://docs.npmjs.com/about-access-tokens)
- [Conventional Commits](https://www.conventionalcommits.org/)
- [semantic-release](https://semantic-release.gitbook.io/)
- [GitHub Secrets](https://docs.github.com/en/actions/security-guides/encrypted-secrets)
