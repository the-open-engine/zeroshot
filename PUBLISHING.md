# NPM Publishing Setup for @the-open-engine/zeroshot

This document explains how to set up automated NPM publishing using semantic-release and GitHub Actions.

## Prerequisites

Before you can publish the package, you need to:

1. **Create the @the-open-engine npm organization** (if it doesn't exist yet)
2. **Configure npm trusted publishing** for this GitHub Actions workflow
3. **Keep an NPM automation token fallback** until trusted publishing has been verified

## Step 1: Create the @the-open-engine npm Organization

The package name `@the-open-engine/zeroshot` uses the `@the-open-engine` scope, which requires an npm organization.

### Check if the organization exists:

```bash
npm org ls @the-open-engine
```

If you get an error or "organization not found", you need to create it:

### Create the organization:

1. Log in to npm:

   ```bash
   npm login
   ```

2. Visit https://www.npmjs.com/org/create

3. Create an organization named `the-open-engine`

4. Choose the organization type:
   - **Free** (for public packages only)
   - **Paid** (if you need private packages)

5. Verify the organization exists:
   ```bash
   npm org ls @the-open-engine
   ```

## Step 2: Configure Trusted Publishing

The release workflow is configured for npm trusted publishing via GitHub Actions OIDC. Configure the package on npm with:

- **GitHub organization/user:** `the-open-engine`
- **Repository:** `zeroshot`
- **Workflow filename:** `release.yml`
- **Allowed action:** `npm publish`

The package's `repository.url` in `package.json` must continue to match `git+https://github.com/the-open-engine/zeroshot.git`.

If `@the-open-engine/zeroshot` does not exist yet, use the `NPM_TOKEN` fallback in Step 3 for the first publish, then add the trusted publisher in npm package settings and keep the token only as an emergency fallback.

## Step 3: Add NPM_TOKEN Fallback

The semantic-release workflow attempts trusted publishing first and falls back to `NPM_TOKEN` when npm OIDC cannot publish, such as the first publish of a new package or a missing trusted-publisher configuration.

### Generate the token:

1. Log in to npmjs.com
2. Open **Access Tokens**
3. Generate an **Automation** token named `GitHub Actions - zeroshot`
4. Copy the token immediately

### Add the secret:

1. Go to https://github.com/the-open-engine/zeroshot
2. Navigate to **Settings -> Secrets and variables -> Actions**
3. Click **New repository secret**
4. Set:
   - **Name:** `NPM_TOKEN`
   - **Value:** the automation token
5. Save the secret

## Step 4: Verify Package Configuration

The package.json is already configured correctly:

```json
{
  "name": "@the-open-engine/zeroshot",
  "version": "5.4.0",
  "publishConfig": {
    "access": "public",
    "registry": "https://registry.npmjs.org/"
  }
}
```

### Key settings:

- **`"access": "public"`** - Required for scoped packages to be public
- **`"registry"`** - Explicit npm registry URL
- **`"name"`** - Scoped package name with @the-open-engine org

## Step 5: Test Publishing Locally (Optional)

Before relying on CI/CD, test packaging manually:

### Dry run:

```bash
npm publish --dry-run
```

This shows what would be published without actually publishing.

### Manual publish fallback:

```bash
npm login
npm publish --access public
```

Use manual publish only for emergency recovery. Normal releases should go through GitHub Actions.

## Step 6: How Automated Publishing Works

Once trusted publishing or the `NPM_TOKEN` fallback is configured, publishing happens automatically from `main` after CI passes.

### Trigger a release:

1. **Make changes** to the codebase

2. **Commit with conventional commit messages**:

   ```bash
   git commit -m "feat: add new feature"      # Minor version bump (0.1.0 → 0.2.0)
   git commit -m "fix: fix bug"               # Patch version bump (0.1.0 → 0.1.1)
   git commit -m "feat!: breaking change"     # Major version bump (0.1.0 → 1.0.0)
   ```

3. **Merge through the protected flow**:

   ```bash
   # PR into dev, then release PR from dev to main
   gh pr create --base dev
   gh pr create --base main --head dev --title "Release"
   ```

4. **GitHub Actions runs** the release workflow:
   - Analyzes commit messages
   - Determines version bump
   - Updates CHANGELOG.md
   - Creates a GitHub release
   - Publishes to npm

### Check the release:

- **GitHub**: https://github.com/the-open-engine/zeroshot/releases
- **npm**: https://www.npmjs.com/package/@the-open-engine/zeroshot

## Conventional Commit Format

semantic-release uses conventional commits to determine version bumps:

| Commit Type                       | Version Bump          | Example                       |
| --------------------------------- | --------------------- | ----------------------------- |
| `fix:`                            | Patch (0.1.0 → 0.1.1) | `fix: resolve memory leak`    |
| `feat:`                           | Minor (0.1.0 → 0.2.0) | `feat: add cluster resume`    |
| `feat!:` or `BREAKING CHANGE:`    | Major (0.1.0 → 1.0.0) | `feat!: change API signature` |
| `docs:`, `chore:`, `style:`, etc. | No release            | `docs: update README`         |

### Breaking changes:

Use `!` after the type or include `BREAKING CHANGE:` in the commit body:

```bash
git commit -m "feat!: remove deprecated API"

# OR

git commit -m "feat: new API" -m "BREAKING CHANGE: removes old API"
```

## Troubleshooting

### Error: "npm ERR! 404 Not Found - PUT https://registry.npmjs.org/@the-open-engine%2fzeroshot"

**Cause:** The @the-open-engine organization or package is missing, trusted publishing is not configured for `the-open-engine/zeroshot` + `release.yml`, or the token fallback lacks publish access.

**Fix:** Create the organization, verify `package.json#repository.url`, configure trusted publishing, or update `NPM_TOKEN`.

### Error: "npm ERR! 403 Forbidden"

**Cause:** The token fallback does not have permission to publish to @the-open-engine.

**Fix:**

1. Verify you're a member of the @the-open-engine npm organization
2. Regenerate the automation token
3. Update the GitHub secret

### Error: "npm ERR! need auth This command requires you to be logged in"

**Cause:** Trusted publishing is not configured and the `NPM_TOKEN` fallback is missing or invalid.

**Fix:** Configure trusted publishing or verify the secret exists in GitHub Settings -> Secrets -> Actions.

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
npm publish --access public

# Open PRs through the protected dev -> main flow,
# then let the release workflow own normal publication again.
```

## Security Best Practices

1. Prefer trusted publishing over long-lived tokens.
2. Keep `NPM_TOKEN` only as a first-publish or emergency fallback.
3. Store tokens only in GitHub Secrets.
4. Enable 2FA on npm maintainer accounts.
5. Revoke or rotate tokens immediately if compromised.

## Next Steps

1. ✅ Create @the-open-engine npm organization (if needed)
2. ✅ Configure trusted publishing for `the-open-engine/zeroshot` + `release.yml`
3. ✅ Add or refresh `NPM_TOKEN` as fallback
4. ✅ Make a commit with `feat:` or `fix:`
5. ✅ Merge dev to main through the protected PR flow
6. ✅ Watch GitHub Actions run the release
7. ✅ Verify package published to npm

## Resources

- [npm Organizations](https://docs.npmjs.com/organizations)
- [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers/)
- [npm Tokens](https://docs.npmjs.com/about-access-tokens)
- [Conventional Commits](https://www.conventionalcommits.org/)
- [semantic-release](https://semantic-release.gitbook.io/)
- [GitHub Secrets](https://docs.github.com/en/actions/security-guides/encrypted-secrets)
