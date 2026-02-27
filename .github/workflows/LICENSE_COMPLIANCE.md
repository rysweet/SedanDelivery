# License Compliance Scanning & Reporting System

Comprehensive automated license compliance scanning and reporting for CyberGym project dependencies across .NET, Python, and Node.js ecosystems.

## Overview

This system provides:

- **Automated License Scanning**: Continuous scanning of all dependencies for license information
- **Policy Enforcement**: Block incompatible licenses (GPL, AGPL, SSPL) in CI/CD
- **AI-Powered Analysis**: GitHub Copilot analyzes license compatibility and provides recommendations
- **Change Tracking**: Monitor license changes over time using repo-memory
- **Weekly Reports**: Comprehensive compliance reports with actionable insights
- **Release Integration**: License reports attached to every release

## Components

### 1. CI/CD License Gate (ci.yml)

**File**: `.github/workflows/ci.yml`

**Trigger**: Pull requests with dependency changes (.csproj, package.json, requirements.txt)

**What it does**:
1. Scans all three ecosystems (.NET, Python, Node.js) for licenses
2. Checks against blocked license list (GPL, AGPL, SSPL, etc.)
3. **BLOCKS PR** if incompatible licenses detected
4. Uploads license scan results as artifacts

**Tools used**:
- `dotnet-project-licenses` for .NET/NuGet packages
- `pip-licenses` for Python packages
- `license-checker` for Node.js/npm packages

**Exit codes**:
- ✅ Exit 0: All licenses approved
- ❌ Exit 1: Incompatible licenses found (PR blocked)

### 2. Weekly License Compliance Report (license-compliance-scan.md)

**File**: `.github/workflows/license-compliance-scan.md` (GitHub Agentic Workflow)

**Compiled**: `.github/workflows/license-compliance-scan.lock.yml`

**Triggers**:
- Schedule: Weekly on Monday mornings
- Release: After release publication
- Manual: workflow_dispatch

**What it does**:
1. Comprehensive scan of all dependencies (all ecosystems)
2. Compare with previous scan (from repo-memory)
3. AI-powered analysis of license compatibility
4. Generate detailed report with:
   - Executive summary with compliance status
   - License distribution statistics
   - Critical findings (blocked licenses)
   - License changes since last scan
   - AI recommendations for problematic packages
5. Create GitHub issue if critical findings or significant changes
6. Store scan snapshot in repo-memory for future comparisons

**Engine**: GitHub Copilot

**Safe Outputs**:
- `create-issue`: Max 2 per run, 7-day expiration
- Labels: `compliance`, `legal`, `automation`

**Repo-Memory**:
- Branch: `memory/licenses`
- Keys: `license-scan-{date}`, `license-last-scan`, `license-issues-created`

## License Policy

### ❌ Blocked Licenses (NOT ALLOWED)

These licenses will cause CI/CD to fail:

- **GPL Family**: `GPL-2.0`, `GPL-3.0`, `GPL-2.0-or-later`, `GPL-3.0-or-later`
- **AGPL Family**: `AGPL-1.0`, `AGPL-3.0`, `AGPL-3.0-only`, `AGPL-3.0-or-later`
- **Server Side Public License**: `SSPL-1.0`
- **Commercial/Proprietary**: Any license requiring commercial approval without documentation

**Why blocked**: These licenses are copyleft (viral) and incompatible with our project's licensing terms. They require derivative works to use the same license, which conflicts with Apache-2.0.

### ✅ Approved Licenses (Always Allowed)

These licenses are compatible and approved:

- **MIT License**: `MIT`
- **Apache License**: `Apache-2.0`
- **BSD Licenses**: `BSD-2-Clause`, `BSD-3-Clause`, `0BSD`
- **ISC License**: `ISC`
- **Public Domain**: `CC0-1.0`, `Unlicense`, `WTFPL`
- **Mozilla Public License**: `MPL-2.0`

**Why approved**: These are permissive licenses compatible with commercial use and redistribution.

### ⚠️ Review Required (Case-by-Case)

These licenses require manual review:

- **LGPL Family**: `LGPL-2.1`, `LGPL-3.0` (acceptable if dynamically linked)
- **Creative Commons**: `CC-BY-*` (depends on use case)
- **Custom/Unknown**: Any license not in SPDX database
- **Dual Licensed**: Packages offering multiple license options

**Review process**: Security/legal team evaluates compatibility before approval.

## Workflow Sequence

### Pull Request Lifecycle

```
Developer updates dependencies (package.json, .csproj, requirements.txt)
    ↓
Creates Pull Request
    ↓
ci.yml triggers (license-scan job)
    ├─ Scan .NET packages (dotnet-project-licenses)
    ├─ Scan Python packages (pip-licenses)
    └─ Scan Node.js packages (license-checker)
    ↓
Check against blocked list
    ├─ ✅ All approved → CI passes, PR can merge
    └─ ❌ Blocked license found → CI FAILS, PR blocked
         ↓
         Developer must:
         - Remove incompatible dependency
         - Find MIT/Apache alternative
         - OR seek legal approval + document exception
```

### Weekly Compliance Report

```
Monday 9:00 AM (schedule trigger)
    ↓
license-compliance-scan.lock.yml runs (GitHub Copilot agent)
    ├─ Scan all three ecosystems
    ├─ Retrieve previous scan from repo-memory
    ├─ Compare current vs. previous (detect changes)
    ├─ Classify licenses (blocked/approved/review)
    └─ AI analysis (Copilot evaluates risks, suggests alternatives)
    ↓
Generate comprehensive report
    ├─ Executive summary
    ├─ License distribution statistics
    ├─ Critical findings (if any)
    ├─ License changes (new, updated, removed dependencies)
    └─ AI recommendations
    ↓
Decision: Create issue?
    ├─ YES if: Blocked licenses OR >5 packages need review OR significant changes
    └─ NO if: All clean, no changes (silent pass)
    ↓
Update repo-memory
    └─ Store scan snapshot: license-scan-{date}
```

### Release Integration

```
Release published on main/integration branch
    ↓
license-compliance-scan.lock.yml triggered (release event)
    ↓
Run comprehensive scan
    ↓
Generate license report
    ↓
Attach to release notes (via issue comment linking to release)
    ↓
Store release snapshot in repo-memory
```

## Usage Guide

### For Developers

#### Adding Dependencies

**Before adding a new dependency**:

1. Check the dependency's license on its repository or npm/NuGet/PyPI page
2. Verify it's an approved license (MIT, Apache-2.0, BSD, ISC)
3. Avoid GPL, AGPL, SSPL licenses
4. Add dependency to appropriate manifest (package.json, .csproj, requirements.txt)
5. Create PR - CI will automatically scan licenses
6. If CI fails, find an alternative with compatible license

#### Handling CI Failures

If license scan fails your PR:

```bash
# CI output will show:
❌ Found incompatible license: GPL-3.0
Package: problematic-lib@1.2.3 (Python)

# Find alternatives:
npm search alternative-to-problematic-lib
# Filter by license: MIT or Apache-2.0

# Replace dependency:
# package.json: "problematic-lib" → "approved-alternative"
npm install approved-alternative
npm uninstall problematic-lib

# Commit and push - CI will re-scan
```

#### Requesting License Exception

If no alternative exists and dependency is critical:

1. File issue titled: "License Exception Request: [package-name]"
2. Include:
   - Why dependency is needed (no alternatives available)
   - How it's used (dynamic linking? Separate process?)
   - License type and text
   - Risk assessment
3. Tag security team and legal team
4. Await approval before proceeding

### For Security/Compliance Teams

#### Viewing Weekly Reports

1. Go to **Issues** tab with label `compliance`
2. Look for weekly report: "🔐 License Compliance Report - YYYY-MM-DD"
3. Review:
   - Executive summary (compliance status)
   - Critical findings (blocked licenses)
   - License changes (new dependencies added)
   - AI recommendations

#### Manual Scan

Trigger on-demand scan:

```bash
# Via GitHub CLI
gh workflow run license-compliance-scan.lock.yml

# Or via GitHub UI
Actions → license-compliance-scan → Run workflow
```

Manual runs **always** create an issue for review.

#### Reviewing Exceptions

When license exception requested:

1. Review issue with details
2. Evaluate:
   - Is dependency truly necessary? (No alternatives?)
   - How is it used? (Dynamically linked? Separate process?)
   - What are the license terms? (Copyleft implications?)
   - Risk to project licensing?
3. Decision:
   - ✅ **Approved**: Add to `.license-exceptions.yml`, close issue
   - ❌ **Denied**: Require alternative, close issue with explanation

**Exception file format** (`.license-exceptions.yml`):

```yaml
exceptions:
  - package: problematic-lib
    version: "1.2.3"
    license: LGPL-2.1
    reason: "Dynamically linked library, no static linking"
    approved_by: "security-team"
    approved_date: "2026-02-14"
    review_date: "2026-08-14"
```

### For Operations/SRE

#### Monitoring Compliance Trends

Track compliance over time:

```bash
# View all compliance issues
gh issue list --label compliance --state all

# Check license distribution trends
# Compare license-scan-{date} entries in repo-memory

# Example: Retrieve historical scan
gh api repos/:owner/:repo/git/trees/memory/licenses --recursive | jq '.tree[] | select(.path | startswith("license-scan-"))'
```

#### Integration with Release Process

License reports are automatically generated on release:

1. Release published → workflow triggered
2. Scan runs → report generated
3. Issue created (if findings) → linked to release
4. Compliance status documented in release notes

#### Vulnerability Scanning Integration

Combine license scanning with vulnerability scanning:

```yaml
# Example: Add to deploy.yml or separate security workflow
- name: Download license scan
  uses: actions/download-artifact@v4
  with:
    name: license-scan-results

- name: Scan for vulnerabilities in licenses
  run: |
    # Use license scan output to check CVEs
    for package in $(jq -r '.[] | .name + "@" + .version' license-scan/*.json); do
      npm audit $package
    done
```

## Technical Details

### License Scanning Tools

#### dotnet-project-licenses (.NET)

```bash
# Install
dotnet tool install --global dotnet-project-licenses

# Scan solution
dotnet-project-licenses --input CyberGym.sln \
  --output-directory /tmp/license-scan \
  --export-license-texts \
  --format json \
  --unique

# Output: JSON with structure
{
  "packages": [
    {
      "id": "Newtonsoft.Json",
      "version": "13.0.1",
      "license": "MIT",
      "licenseUrl": "https://licenses.nuget.org/MIT",
      "projectUrl": "https://www.newtonsoft.com/json"
    }
  ]
}
```

#### pip-licenses (Python)

```bash
# Install
pip install pip-licenses

# Scan installed packages
pip-licenses --format json --output-file python-licenses.json

# Output: JSON with structure
[
  {
    "Name": "requests",
    "Version": "2.31.0",
    "License": "Apache 2.0",
    "LicenseFile": "/path/to/LICENSE"
  }
]
```

#### license-checker (Node.js)

```bash
# Install
npm install -g license-checker

# Scan project
license-checker --json --out node-licenses.json

# Output: JSON with structure
{
  "package-name@1.0.0": {
    "licenses": "MIT",
    "repository": "https://github.com/user/repo",
    "licenseFile": "/path/to/LICENSE"
  }
}
```

### Repo-Memory Structure

Repo-memory stores license history in branch `memory/licenses`:

```
memory/licenses
├── license-scan-2026-02-14.json      # Weekly scan snapshot
├── license-scan-2026-02-07.json      # Previous week
├── license-scan-2026-01-31.json      # Older scans
├── license-last-scan.txt              # Timestamp: 2026-02-14T09:00:00Z
└── license-issues-created.json        # [123, 145, 178] (issue numbers)
```

**Snapshot structure**:

```json
{
  "scan_date": "2026-02-14",
  "scan_trigger": "schedule",
  "total_dependencies": 144,
  "ecosystems": {
    "dotnet": {
      "count": 80,
      "licenses": {
        "MIT": 45,
        "Apache-2.0": 23,
        "BSD-3-Clause": 8,
        "Others": 4
      },
      "packages": [
        {"name": "Newtonsoft.Json", "version": "13.0.1", "license": "MIT"}
      ]
    },
    "python": {
      "count": 19,
      "licenses": {"MIT": 12, "Apache-2.0": 5, "BSD-3-Clause": 2}
    },
    "nodejs": {
      "count": 45,
      "licenses": {"MIT": 34, "ISC": 8, "Apache-2.0": 3}
    }
  },
  "blocked_count": 0,
  "review_required_count": 2,
  "review_required": [
    {"name": "some-lib", "version": "2.1.0", "license": "LGPL-2.1", "ecosystem": "dotnet"}
  ]
}
```

### GitHub Copilot AI Analysis

The agentic workflow uses Copilot to provide intelligent analysis:

**Capabilities**:
- **Risk assessment**: Analyze overall license risk profile
- **Compatibility check**: Detect license conflicts between dependencies
- **Alternative suggestions**: Recommend MIT/Apache alternatives for problematic packages
- **Trend analysis**: Identify licensing trends (more/less restrictive over time)
- **Best practices**: Recommend improvements to dependency management

**Example AI output**:

```markdown
## AI-Powered Analysis

**Risk Profile**: Low ✅
- 98.6% of dependencies use approved permissive licenses
- No copyleft contamination detected
- Only 2 packages require review, both low risk

**Compatibility Analysis**: Pass ✅
- No conflicting license requirements
- All dependencies compatible with project's Apache-2.0 license

**Recommendations**:
1. **Replace `some-lib` (LGPL-2.1)**: Consider `alternative-lib` (MIT) for similar functionality
2. **Monitor `new-package`**: Recently added, track for stability
3. **Consolidate dependencies**: 3 packages provide similar JSON parsing - consider reducing to 1

**Trends**:
- Dependencies grew by 5% since last scan (137 → 144)
- All new packages use MIT or Apache-2.0 (positive trend)
- License diversity remains low (reduces compliance overhead)
```

## Configuration

### Modifying Blocked Licenses

Edit `ci.yml` and `license-compliance-scan.md`:

```yaml
# ci.yml - Update BLOCKED_LICENSES
BLOCKED_LICENSES="GPL-2.0 GPL-3.0 AGPL-3.0 AGPL-1.0 SSPL-1.0"
```

```markdown
# license-compliance-scan.md - Update policy section
### Blocked Licenses (Incompatible)
- `GPL-2.0`, `GPL-3.0`
- `AGPL-1.0`, `AGPL-3.0`
- `SSPL-1.0`
- `NEW-BLOCKED-LICENSE`  # Add here
```

After editing `.md` file, recompile:

```bash
gh aw compile .github/workflows/license-compliance-scan.md
```

### Adjusting Scan Frequency

Edit `license-compliance-scan.md` frontmatter:

```yaml
on:
  schedule: weekly on monday  # Change to: daily, weekly on friday, etc.
  workflow_dispatch:
  release:
    types: [published]
```

### Customizing Safe Outputs

Increase issue creation limits:

```yaml
safe-outputs:
  create-issue:
    max: 2          # Increase to 5 for more issues
    expires: 7d     # Change to 30d for longer expiration
    labels:
      - compliance
      - legal
      - automation
      - security    # Add additional labels
```

## Troubleshooting

### CI License Scan Fails but Should Pass

**Symptom**: PR blocked even though licenses appear compatible

**Diagnosis**:
1. Check CI logs for exact package and license flagged
2. Verify license name format (might be non-SPDX)

**Solution**:

```bash
# Download artifact to inspect
gh run download [run-id] -n license-scan-results

# Check actual license in scan output
jq '.packages[] | select(.name == "problem-package")' license-scan/*.json

# If license is actually compatible (e.g., "MIT License" vs "MIT"):
# Update blocked license check to normalize names
```

### Weekly Report Not Generated

**Symptom**: Monday morning, no compliance issue created

**Possible causes**:
- Workflow didn't run (check Actions tab)
- No significant findings (silent pass - expected)
- Safe output limit reached (max 2 issues per run)

**Diagnosis**:

```bash
# Check workflow runs
gh run list --workflow=license-compliance-scan.lock.yml

# View most recent run logs
gh run view [run-id] --log
```

**Solution**:
- If workflow didn't run: Check schedule trigger syntax
- If no findings: Expected behavior (no noise)
- If limit reached: Wait for next run or increase max in safe-outputs

### Repo-Memory Not Updating

**Symptom**: Reports don't show "changes since last scan"

**Diagnosis**:

```bash
# Check if memory branch exists
git ls-remote origin memory/licenses

# View memory contents
gh api repos/:owner/:repo/git/trees/memory/licenses --recursive
```

**Solution**:

```bash
# Create memory branch if missing
git checkout --orphan memory/licenses
git rm -rf .
git commit --allow-empty -m "Initialize license memory"
git push origin memory/licenses
```

### False Positive: Approved License Flagged

**Symptom**: MIT license flagged as incompatible

**Cause**: License name variation (e.g., "MIT License" vs "MIT")

**Solution**: Normalize license names in check:

```bash
# In ci.yml, update check to be case-insensitive and normalize
for license in $BLOCKED_LICENSES; do
  # Use grep -i for case-insensitive
  if grep -qi "$license" /tmp/license-scan/*.json; then
    # Additional check: exclude variations
    if ! grep -qi "mit" /tmp/license-scan/*.json; then
      echo "❌ Found incompatible license: $license"
    fi
  fi
done
```

## Security Considerations

### Data Privacy

License scan data contains:
- ✅ Package names and versions (public information)
- ✅ License identifiers (public information)
- ✅ Repository URLs (public information)
- ❌ No secrets, credentials, or proprietary code

**Safe to share**: License reports can be public if repository is public.

### Access Control

- **CI scans**: Run with `contents: read`, `pull-requests: write`
- **Weekly reports**: Run with `contents: read` (no write access)
- **Safe outputs**: Limit issue creation (max 2 per run)
- **Repo-memory**: Isolated branch, no code access

### Threat Detection

Workflow includes threat detection for:
- Attempts to approve unauthorized licenses
- Malicious package names that could confuse scanners
- Attempts to suppress license warnings inappropriately

## Best Practices

### For Development Teams

1. **Check licenses before adding dependencies**
2. **Prefer MIT and Apache-2.0 licenses**
3. **Avoid GPL and AGPL at all costs**
4. **Review weekly compliance reports**
5. **Document any approved exceptions**

### For Security Teams

1. **Monitor compliance issues weekly**
2. **Audit license exceptions quarterly**
3. **Track license distribution trends**
4. **Integrate with vulnerability scanning**
5. **Update blocked license list as policy evolves**

### For Legal Teams

1. **Review license policy annually**
2. **Approve or deny exception requests**
3. **Document approval decisions**
4. **Provide guidance on edge cases (LGPL, dual licenses)**
5. **Ensure compliance with customer contracts**

## Integration Examples

### CI/CD Pipeline

```yaml
# .github/workflows/ci.yml (already included)
jobs:
  license-scan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Scan licenses
        run: dotnet-project-licenses ...
      - name: Check blocked licenses
        run: |
          if grep -qi "GPL" license-scan/*.json; then
            exit 1
          fi
```

### Pre-commit Hook

```bash
# .git/hooks/pre-commit
#!/bin/bash
# Check licenses before committing dependency changes

if git diff --cached --name-only | grep -E '(package.json|.csproj|requirements.txt)'; then
  echo "Dependency change detected, checking licenses..."

  # Run quick license scan
  dotnet-project-licenses --input . --format json | grep -qi "GPL"

  if [ $? -eq 0 ]; then
    echo "❌ Blocked license detected! Aborting commit."
    exit 1
  fi
fi
```

### Slack Notifications

```yaml
# Add to license-compliance-scan.md workflow
- name: Notify Slack on blocked license
  if: blocked_licenses_found
  run: |
    curl -X POST $SLACK_WEBHOOK_URL \
      -H 'Content-Type: application/json' \
      -d "{\"text\": \"🚨 License Compliance Alert: Blocked license detected in ${{ github.repository }}\"}"
```

## References

- [SPDX License List](https://spdx.org/licenses/)
- [dotnet-project-licenses](https://github.com/tomchavakis/nuget-license)
- [pip-licenses](https://github.com/raimon49/pip-licenses)
- [license-checker (npm)](https://github.com/davglass/license-checker)
- [GitHub Agentic Workflows](https://github.com/github/gh-aw)
- [Open Source Licensing Guide](https://opensource.guide/legal/)

## Support

For issues with license compliance:

1. **CI failures**: Check workflow logs, review flagged package
2. **False positives**: File issue with details, tag security team
3. **License exceptions**: Follow exception request process
4. **Policy questions**: Contact legal team

**Issue template**: "License Compliance: [Brief Description]"

Labels: `compliance`, `legal`, `question`

---

**Last Updated**: 2026-02-14
**Version**: 1.0.0
**Maintainer**: Security & Compliance Team
