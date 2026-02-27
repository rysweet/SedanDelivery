# Workflow Compilation Instructions

## Overview

The `performance-testing.md` file needs to be compiled into a `.lock.yml` file using the `gh aw compile` command. This compilation process generates the actual GitHub Actions workflow that will execute.

## Compilation Status

🚧 **PENDING** - Workflow has been created but not yet compiled.

## Why Compilation is Needed

GitHub Agentic Workflows (gh-aw) uses a two-file system:

1. **Source file** (`.md`): Human-readable workflow definition with natural language instructions
2. **Lock file** (`.lock.yml`): Compiled GitHub Actions workflow with security hardening

The compilation process:
- Validates frontmatter schema
- Checks expression safety (prevents secret leakage)
- Pins GitHub Actions to SHA hashes (supply chain security)
- Runs security scanners (actionlint, zizmor, poutine)
- Generates hardened workflow configuration

## Compilation Command

```bash
# From repository root or worktree
cd /home/azureuser/src/SedanDelivery

# Compile the workflow
gh aw compile .github/aw/performance-testing.md

# Output: .github/aw/performance-testing.lock.yml
```

## Expected Output

After successful compilation, you should see:

```
✓ Validating frontmatter schema
✓ Checking expression safety
✓ Pinning actions to SHAs
✓ Running security scanners
✓ Generating lock file

Compiled: .github/aw/performance-testing.lock.yml
```

## Post-Compilation Steps

Once compiled:

1. **Review the lock file**:
   ```bash
   cat .github/aw/performance-testing.lock.yml | head -50
   ```

2. **Verify compilation hash** (in lock file header):
   ```yaml
   # frontmatter-hash: <sha256-hash>
   ```

3. **Commit both files**:
   ```bash
   git add .github/aw/performance-testing.md
   git add .github/aw/performance-testing.lock.yml
   git commit -m "feat: Add performance testing workflow"
   ```

4. **Push to remote**:
   ```bash
   git push origin feat/issue-175-performance-testing
   ```

## Troubleshooting Compilation

### Error: "Invalid frontmatter schema"

**Cause**: YAML frontmatter syntax error

**Solution**:
```bash
# Validate YAML
yamllint .github/aw/performance-testing.md

# Common issues:
# - Incorrect indentation
# - Missing colons
# - Invalid field names
```

### Error: "Unsafe expression detected"

**Cause**: Workflow uses expressions that could leak secrets

**Solution**:
- Review expressions in frontmatter
- Ensure no `${{ secrets.* }}` in unsafe contexts
- Use environment variables for sensitive data

### Error: "Action not found"

**Cause**: Referenced GitHub Action doesn't exist or is inaccessible

**Solution**:
- Verify action names: `owner/repo@version`
- Check network connectivity
- Ensure actions are public or accessible

### Error: "Security scanner failed"

**Cause**: actionlint or other security scanners detected issues

**Solution**:
```bash
# Run scanners manually
actionlint .github/aw/performance-testing.lock.yml

# Check zizmor (privilege escalation)
zizmor .github/aw/performance-testing.lock.yml

# Fix reported issues
```

## Compilation Requirements

**Prerequisites**:
- `gh` CLI installed and authenticated
- `gh aw` extension installed (`gh extension install github/gh-aw`)
- Network access to GitHub API
- Valid GitHub token with required permissions

**Permissions needed**:
- `repo` scope (read repository)
- `workflow` scope (read workflow definitions)

## Manual Compilation (If gh aw Not Available)

If `gh aw compile` is unavailable, you can:

1. **Use GitHub Actions workflow** to compile:
   - Create `.github/workflows/compile-aw.yml`
   - Use `github/gh-aw` action to compile
   - Commit result automatically

2. **Use pre-compiled template**:
   - Copy structure from existing `.lock.yml` files
   - Manually update job definitions
   - **NOT RECOMMENDED** - security features may be missed

## Verification After Compilation

**Check that lock file**:
1. Contains frontmatter hash matching source
2. Has SHA-pinned action versions
3. Includes all jobs from source workflow
4. Has security hardening applied

**Test workflow**:
```bash
# Trigger manually
gh workflow run performance-testing.lock.yml \
  --ref feat/issue-175-performance-testing \
  --field branch=integration \
  --field test_duration=5

# Monitor execution
gh run watch
```

## Re-Compilation

**When to recompile**:
- After editing `.md` source file
- When gh-aw version is updated
- After security advisories
- To update action SHA pins

**Re-compilation is automatic if**:
- GitHub Actions detects stale lock file
- Timestamp mismatch between source and lock

## Additional Resources

- [gh-aw Compilation Docs](https://github.github.com/gh-aw/reference/compilation/)
- [Workflow Validation](https://github.github.com/gh-aw/reference/validation/)
- [Security Scanning](https://github.github.com/gh-aw/reference/security/)

---

**Status**: 🚧 Awaiting compilation
**Created**: 2026-02-14
**Source**: `.github/aw/performance-testing.md`
**Target**: `.github/aw/performance-testing.lock.yml` (not yet generated)
