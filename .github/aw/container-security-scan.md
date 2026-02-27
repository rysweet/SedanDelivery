---
on:
  schedule: weekly
  workflow_dispatch:

permissions:
  contents: read
  security-events: read
  issues: read

engine: copilot

tools:
  github:
    toolsets: [repos, issues, code_security]
    mode: remote
    read-only: true
  repo-memory:
    branch-name: memory/vulnerabilities

safe-outputs:
  create-issue:
    title-prefix: "[Security] "
    labels: [security, automation]
    max: 3
    expires: 2
    close-older-issues: true
---

# Container Vulnerability Trend Analysis

## Mission

Analyze container vulnerability trends from Trivy scans and generate actionable security reports for the CyberGym team.

## Instructions

### 1. Gather Current Vulnerability Data

Use the GitHub code_security toolset to retrieve the latest container vulnerability scan results from the Security tab:

- Query code scanning alerts with category matching "container-*"
- Filter for HIGH and CRITICAL severity vulnerabilities
- Group vulnerabilities by container image:
  - gameboard-tenant (CyberGym.GameboardTenant)
  - catalog (CyberGym.Catalog)
  - tenant-scanning (CyberGym.TenantScanning)
- Note: vulnerability ID (CVE), severity, affected package, and fix availability

### 2. Load Historical Data

Use repo-memory to retrieve vulnerability history from previous weeks:

- Load data from branch: `memory/vulnerabilities`
- Compare current vulnerabilities with historical data
- Identify:
  - **New vulnerabilities** (appeared this week)
  - **Resolved vulnerabilities** (disappeared this week)
  - **Persistent vulnerabilities** (present for multiple weeks)

### 3. Analyze Trends

Calculate and analyze:

- Total vulnerability count by severity (CRITICAL, HIGH)
- Vulnerability count per container image
- Week-over-week change (increase/decrease)
- Average time to remediation for resolved vulnerabilities
- Most frequently appearing CVEs
- Containers with most security issues

### 4. Generate Weekly Report Issue

Create a comprehensive weekly report as a GitHub issue with this structure:

**Title**: `[Weekly] Container Vulnerability Trend Report - Week of YYYY-MM-DD`

**Body**:

```markdown
## Summary

- **Total Vulnerabilities**: X (Y CRITICAL, Z HIGH)
- **Week-over-Week Change**: +/- N vulnerabilities
- **Containers Scanned**: gameboard-tenant, catalog, tenant-scanning

## Vulnerabilities by Container

### GameboardTenant
- CRITICAL: X
- HIGH: Y
- Top CVEs: CVE-YYYY-NNNNN, CVE-YYYY-NNNNN

### Catalog
- CRITICAL: X
- HIGH: Y
- Top CVEs: CVE-YYYY-NNNNN, CVE-YYYY-NNNNN

### TenantScanning
- CRITICAL: X
- HIGH: Y
- Top CVEs: CVE-YYYY-NNNNN, CVE-YYYY-NNNNN

## New Vulnerabilities This Week

List new vulnerabilities discovered in the past week:
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Severity: CRITICAL
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Severity: HIGH

## Resolved Vulnerabilities

Vulnerabilities fixed since last week:
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Resolved by: base image update
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Resolved by: package upgrade

## Persistent Vulnerabilities

Vulnerabilities present for 2+ weeks:
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Weeks present: N
- [CVE-YYYY-NNNNN] Container: X, Package: Y, Weeks present: N

## Trend Analysis

- **Velocity**: Average X days to fix CRITICAL vulnerabilities
- **Pattern**: Most vulnerabilities from [base images / application dependencies]
- **Recommendation**: [Update base images / Upgrade vulnerable packages / Review dependencies]

## Action Items

1. **Immediate** (CRITICAL): Review and fix CVE-YYYY-NNNNN in container X
2. **This Sprint** (HIGH): Address persistent vulnerabilities in container Y
3. **Future**: Consider alternative base images to reduce vulnerability surface

## Resources

- [View all container scan results](../../security/code-scanning)
- [Trivy Documentation](https://aquasecurity.github.io/trivy/)
- [Container Scanning Docs](../../docs/security/container-scanning.md)
```

### 5. Create Issues for New CRITICAL Vulnerabilities

For each new CRITICAL vulnerability discovered this week:

- Create a separate GitHub issue (max 3 per run, due to safe-outputs limit)
- Title: `[CRITICAL] Container Vulnerability: CVE-YYYY-NNNNN in {container-name}`
- Body: Include CVE details, affected container, package, fix recommendation, and remediation timeline
- Labels: `security`, `critical`, `automation`
- Assign to security team if configured

### 6. Update Vulnerability Memory

Save current vulnerability data to repo-memory for next week's comparison:

- Store snapshot as JSON with:
  - Timestamp
  - Vulnerability list by container
  - Total counts by severity
  - CVE details
- Branch: `memory/vulnerabilities`
- File: `vulnerabilities-YYYY-MM-DD.json`

## Success Criteria

- Weekly report issue created with accurate data
- New CRITICAL vulnerabilities have individual issues
- Historical trends calculated correctly
- Vulnerability memory updated for next run
- Actionable recommendations provided to team

## Tone and Style

- Professional and data-driven
- Clear and actionable recommendations
- Prioritize CRITICAL vulnerabilities prominently
- Use bullet points and tables for readability
- Include links to relevant resources

## Edge Cases

- If no vulnerabilities found: Celebrate in report, but still create issue
- If repo-memory is empty (first run): Note baseline established, no trends yet
- If unable to access Security tab: Report error and suggest checking permissions
- If too many new CRITICAL vulnerabilities (>3): Prioritize by exploitability and exposure
