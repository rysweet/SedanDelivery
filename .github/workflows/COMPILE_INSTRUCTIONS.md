# Compilation Instructions for Test Coverage Enforcement Workflow

## Status

⚠️ **The `test-coverage-enforcement.md` workflow has been created but needs compilation.**

The compiled `.lock.yml` file must be generated before this workflow can run.

## Why Compilation is Required

GitHub Agentic Workflows (gh-aw) use a two-file system:

1. **Source File** (`.md`): Human-readable natural language workflow definition
2. **Compiled File** (`.lock.yml`): Machine-executable GitHub Actions workflow

The compilation process:
- Validates YAML frontmatter against schemas
- Pins all GitHub Actions to specific SHA hashes (supply chain security)
- Generates SafeOutput jobs with proper permission isolation
- Runs security scanners (actionlint, zizmor, poutine)
- Creates production-ready workflow with defense-in-depth security

## How to Compile

### Prerequisites

1. Install GitHub CLI (if not already installed):
   ```bash
   # macOS
   brew install gh

   # Windows
   winget install GitHub.cli

   # Linux (Debian/Ubuntu)
   sudo apt install gh
   ```

2. Install gh-aw extension:
   ```bash
   gh extension install github/gh-aw
   ```

### Compilation Steps

```bash
# Navigate to workflows directory
cd .github/workflows

# Compile the workflow
gh aw compile test-coverage-enforcement.md

# This will generate test-coverage-enforcement.lock.yml
```

### Verify Compilation

Check that the `.lock.yml` file was created:

```bash
ls -lh test-coverage-enforcement.lock.yml
```

The file should be significantly larger than the source `.md` file (typically 30-50KB).

### Commit Both Files

Both files must be committed to the repository:

```bash
git add test-coverage-enforcement.md test-coverage-enforcement.lock.yml
git commit -m "feat: Add test coverage enforcement workflow"
```

## Alternative: Manual Compilation Environment

If you don't have local access to gh-aw, you can compile in a GitHub Codespace or CI:

### Option 1: GitHub Codespace

```bash
# In a GitHub Codespace
gh extension install github/gh-aw
cd .github/workflows
gh aw compile test-coverage-enforcement.md
```

### Option 2: CI Compilation Job

Create a temporary workflow to compile:

```yaml
name: Compile gh-aw Workflow
on: workflow_dispatch

jobs:
  compile:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install gh-aw
        run: gh extension install github/gh-aw
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Compile workflow
        run: |
          cd .github/workflows
          gh aw compile test-coverage-enforcement.md

      - name: Upload compiled workflow
        uses: actions/upload-artifact@v4
        with:
          name: compiled-workflow
          path: .github/workflows/test-coverage-enforcement.lock.yml
```

## Troubleshooting

### Error: "gh-aw extension not found"

**Solution**: Install the extension first
```bash
gh extension install github/gh-aw
```

### Error: "Invalid frontmatter"

**Solution**: Check YAML syntax in the frontmatter section of the `.md` file
```bash
# Validate YAML
yq eval '.github/workflows/test-coverage-enforcement.md' > /dev/null
```

### Error: "Action not found in lock file"

**Solution**: Update actions-lock.json
```bash
# Add missing actions to .github/aw/actions-lock.json
gh aw update-locks
```

### Error: "Security scan failed"

**Solution**: Review security scanner output
```bash
# Run security scans manually
gh aw compile test-coverage-enforcement.md --verbose
```

## What Gets Generated

The `.lock.yml` file will contain:

1. **Pre-activation Job**: Role checks, deadline validation
2. **Activation Job**: Context preparation, sanitization
3. **Agent Job**: Read-only execution with GitHub Copilot
4. **Threat Detection Job**: AI-powered security analysis
5. **SafeOutput Jobs**: Permission-isolated issue creation
6. **Conclusion Job**: Result aggregation, summary generation

## Security Features in Compiled Workflow

The compilation process adds:

- ✅ SHA-pinned actions (prevents supply chain attacks)
- ✅ Permission isolation (agent runs read-only)
- ✅ Content sanitization (prevents prompt injection)
- ✅ Secret redaction (automatic masking)
- ✅ Network firewall (domain allowlisting)
- ✅ Threat detection (AI-powered analysis)

## Next Steps After Compilation

1. ✅ Verify `.lock.yml` file exists
2. ✅ Commit both `.md` and `.lock.yml` files
3. ✅ Push to repository
4. ✅ Trigger workflow manually to test: `gh workflow run test-coverage-enforcement.lock.yml`
5. ✅ Check workflow run: `gh run list --workflow=test-coverage-enforcement.lock.yml`
6. ✅ Review generated issues in repository

## References

- [gh-aw Documentation](https://github.com/github/gh-aw)
- [gh-aw Compilation Guide](https://github.github.com/gh-aw/reference/compilation/)
- [GitHub Actions Security Hardening](https://docs.github.com/en/actions/security-guides/security-hardening-for-github-actions)
