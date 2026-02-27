# SBOM Generation for CyberGym

This document describes the automated Software Bill of Materials (SBOM) generation and analysis system for the CyberGym project.

## Overview

The SBOM generation system automatically creates comprehensive software bill of materials for every release and provides automated analysis of dependency changes between releases.

## Components

### 1. SBOM Generation (auto-release.yml)

**Workflow**: `.github/workflows/auto-release.yml`

**Trigger**: Push to `main` branch (automatic release creation)

**What it does**:
1. Creates a version-tagged release (format: `vYYYY.MM.DD.BUILD`)
2. Generates SBOM in two industry-standard formats:
   - **SPDX JSON**: Standard format for compliance and vulnerability scanning
   - **CycloneDX JSON**: Industry standard for security and supply chain analysis
3. Attaches both SBOM files as release artifacts

**Technology**:
- Uses [Anchore Syft](https://github.com/anchore/syft) via [anchore/sbom-action](https://github.com/anchore/sbom-action)
- Scans entire repository workspace
- Generates comprehensive dependency trees including transitive dependencies

**Generated Files**:
- `sbom-spdx.json`: SPDX 2.3 format SBOM
- `sbom-cyclonedx.json`: CycloneDX 1.4+ format SBOM

### 2. SBOM Diff Analysis (sbom-diff-analysis.md)

**Workflow**: `.github/workflows/sbom-diff-analysis.md` (GitHub Agentic Workflow)

**Compiled**: `.github/workflows/sbom-diff-analysis.lock.yml`

**Trigger**: Release created (after auto-release.yml completes)

**What it does**:
1. Downloads SBOM files from current and previous releases
2. Analyzes differences in dependencies:
   - New dependencies added
   - Dependencies removed
   - Version changes (major, minor, patch)
   - License changes
3. Generates security assessment
4. Creates a GitHub Discussion with comprehensive analysis

**Engine**: GitHub Copilot (AI-powered analysis)

**Safe Outputs**:
- `create-discussion`: Creates one discussion per release in "Releases" category
- Maximum: 1 discussion per workflow run

**Analysis Includes**:
- Dependency statistics comparison
- New, removed, and updated dependencies
- Security assessment (vulnerabilities detected in SBOM metadata)
- License distribution changes
- Links to full SBOM artifacts

## Workflow Sequence

```
Push to main
    ↓
auto-release.yml runs
    ├─ Generate version tag (vYYYY.MM.DD.BUILD)
    ├─ Generate SBOM (SPDX format)
    ├─ Generate SBOM (CycloneDX format)
    └─ Create GitHub release with SBOM artifacts
         ↓
         [Release Created Event]
         ↓
sbom-diff-analysis.lock.yml runs
    ├─ Fetch current release SBOMs
    ├─ Fetch previous release SBOMs (if available)
    ├─ Analyze differences (Copilot AI)
    ├─ Generate security assessment
    └─ Create GitHub Discussion with findings
```

## Usage

### For Developers

**Automatic Operation**: SBOM generation happens automatically on every merge to `main`. No manual action required.

**Viewing SBOMs**:
1. Go to [Releases](https://github.com/cloud-ecosystem-security/SedanDelivery/releases)
2. Select a release
3. Download SBOM files from release assets:
   - `sbom-spdx.json`
   - `sbom-cyclonedx.json`

**Reviewing Analysis**:
1. Go to [Discussions → Releases](https://github.com/cloud-ecosystem-security/SedanDelivery/discussions/categories/releases)
2. Find discussion titled "SBOM Analysis: Release vYYYY.MM.DD.BUILD"
3. Review dependency changes and security assessment

### For Security/Compliance Teams

**Compliance Scanning**:
```bash
# Download SBOM for a specific release
gh release download v2026.02.14.1 -p "sbom-*.json"

# Scan with vulnerability scanner (example: Grype)
grype sbom:./sbom-spdx.json

# Or with Syft
syft sbom-cyclonedx.json
```

**License Auditing**:
```bash
# Extract license information from SPDX
jq '.packages[] | {name: .name, version: .versionInfo, license: .licenseConcluded}' sbom-spdx.json

# From CycloneDX
jq '.components[] | {name: .name, version: .version, license: .licenses}' sbom-cyclonedx.json
```

**Dependency Analysis**:
```bash
# Count total dependencies
jq '.packages | length' sbom-spdx.json

# List direct dependencies (varies by language/package manager)
jq '.relationships[] | select(.relationshipType == "DEPENDENCY_OF")' sbom-spdx.json
```

### For Operations/SRE

**Integration with Vulnerability Scanning**:

The SBOMs can be integrated with vulnerability scanning tools:

```yaml
# Example: Add vulnerability scanning after SBOM generation
- name: Scan SBOM for vulnerabilities
  uses: anchore/scan-action@v3
  with:
    sbom: sbom-spdx.json
    fail-build: false  # Set to true to fail on vulnerabilities
```

**Monitoring Dependency Changes**:

Check the GitHub Discussions automatically created after each release for:
- Unexpected dependency additions
- Security vulnerability introductions
- License compliance issues
- Major version bumps requiring attention

## SBOM Formats

### SPDX (Software Package Data Exchange)

**Best for**:
- Compliance requirements (ISO/IEC 5962:2021)
- Government contracts (NTIA minimum elements)
- License compliance auditing
- Supply chain transparency

**Structure**:
- Package metadata (name, version, supplier)
- License information (SPDX license identifiers)
- File-level details
- Relationship graphs (dependencies)
- Checksums and verification data

### CycloneDX

**Best for**:
- Security analysis and vulnerability management
- DevSecOps integration
- Real-time dependency tracking
- Component pedigree and provenance

**Structure**:
- Component inventory (libraries, frameworks)
- Service definitions (APIs, microservices)
- Vulnerability references (CVE, CWE)
- License information
- Dependency graph
- Pedigree and external references

## Configuration

### Modifying SBOM Generation

Edit `.github/workflows/auto-release.yml`:

```yaml
# Add additional SBOM formats
- name: Generate SBOM (SPDX Tag-Value)
  uses: anchore/sbom-action@v0.18.0
  with:
    path: .
    format: spdx  # Tag-value format
    output-file: sbom-spdx.txt

# Customize scan scope
- name: Generate SBOM (Application Only)
  uses: anchore/sbom-action@v0.18.0
  with:
    path: ./CyberGym.Catalog  # Specific directory
    format: spdx-json
    output-file: sbom-catalog.json
```

### Modifying SBOM Analysis

Edit `.github/workflows/sbom-diff-analysis.md` and recompile:

```bash
# Make changes to sbom-diff-analysis.md
vim .github/workflows/sbom-diff-analysis.md

# Recompile the workflow
gh aw compile .github/workflows/sbom-diff-analysis.md
```

**Important**: Always edit the `.md` file, never the `.lock.yml` file. The lock file is auto-generated.

### Adjusting Safe Outputs

Modify the frontmatter in `sbom-diff-analysis.md`:

```yaml
safe-outputs:
  create-discussion:
    max: 1  # Limit discussions per workflow run
```

## Troubleshooting

### SBOM Generation Failures

**Symptom**: Release created but SBOM files missing

**Diagnosis**:
1. Check workflow logs for auto-release.yml run
2. Look for Anchore Syft errors

**Common causes**:
- Large repository size (timeout)
- Unsupported package manager
- Network issues downloading dependencies

**Solution**:
```yaml
# Add timeout and retry logic
- name: Generate SBOM (SPDX)
  uses: anchore/sbom-action@v0.18.0
  timeout-minutes: 15
  with:
    path: .
    format: spdx-json
    output-file: sbom-spdx.json
```

### SBOM Analysis Not Running

**Symptom**: Release created but no discussion posted

**Diagnosis**:
1. Check workflow runs for sbom-diff-analysis.lock.yml
2. Review agent logs for errors

**Common causes**:
- SBOM files not available in release assets
- GitHub Copilot API issues
- Safe output quota exceeded

**Solution**:
1. Verify SBOM files exist in release
2. Check GitHub Actions logs for detailed error
3. Retry workflow manually if needed:
```bash
gh workflow run sbom-diff-analysis.lock.yml
```

### Discussion Not Created

**Symptom**: Workflow succeeds but no discussion appears

**Possible causes**:
- Safe output limit reached (max: 1 per run)
- Insufficient permissions
- Discussion category doesn't exist

**Solution**:
1. Check workflow logs for "create-discussion" output
2. Ensure "Releases" category exists in Discussions
3. Verify workflow has correct permissions

## Security Considerations

### SBOM Contents

SBOMs contain:
- ✅ **Public information**: Package names, versions, licenses
- ✅ **Safe to share**: No secrets, credentials, or internal code
- ✅ **Compliance data**: Required for transparency and auditing

### Access Control

- **SBOMs**: Attached to public releases (if repository is public)
- **Analysis**: Posted to GitHub Discussions (respects repository visibility)
- **Workflow**: Uses minimal permissions (contents: read)

### Best Practices

1. **Review before release**: Check SBOM contents before making repository public
2. **Vulnerability scanning**: Integrate SBOM scanning into CI/CD
3. **Regular audits**: Review dependency changes in discussions
4. **License compliance**: Monitor license distribution changes

## Integration Examples

### Dependency Review

```bash
# Compare dependencies between releases
diff <(jq -r '.packages[].name' sbom-v1.json | sort) \
     <(jq -r '.packages[].name' sbom-v2.json | sort)
```

### Vulnerability Scanning Pipeline

```yaml
# Add to auto-release.yml or separate workflow
- name: Download SBOM
  run: gh release download ${{ steps.tag.outputs.tag }} -p "sbom-spdx.json"

- name: Scan for vulnerabilities
  uses: aquasecurity/trivy-action@master
  with:
    scan-type: 'sbom'
    input: 'sbom-spdx.json'
    format: 'sarif'
    output: 'trivy-results.sarif'

- name: Upload to Security tab
  uses: github/codeql-action/upload-sarif@v3
  with:
    sarif_file: 'trivy-results.sarif'
```

### Compliance Reporting

```bash
# Generate compliance report from SBOM
sbom-tool compliance -i sbom-spdx.json -o compliance-report.html
```

## Maintenance

### Regular Tasks

- **Monthly**: Review SBOM analysis discussions for trends
- **Quarterly**: Audit dependency licenses for compliance
- **Per release**: Review security assessment in discussion
- **As needed**: Update Anchore Syft version in workflows

### Updating Components

```bash
# Update gh-aw (for analysis workflow)
gh extension upgrade gh-aw

# Recompile workflows
gh aw compile .github/workflows/sbom-diff-analysis.md

# Update action versions (in auto-release.yml)
# Check: https://github.com/anchore/sbom-action/releases
```

## References

- [Anchore SBOM Action](https://github.com/anchore/sbom-action)
- [Syft Documentation](https://github.com/anchore/syft)
- [SPDX Specification](https://spdx.dev/specifications/)
- [CycloneDX Specification](https://cyclonedx.org/specification/overview/)
- [GitHub Agentic Workflows](https://github.com/github/gh-aw)
- [NTIA SBOM Minimum Elements](https://www.ntia.gov/files/ntia/publications/sbom_minimum_elements_report.pdf)

## Support

For issues with SBOM generation:
1. Check workflow logs in GitHub Actions
2. Review this documentation
3. File an issue with:
   - Release tag
   - Workflow run URL
   - Error message or unexpected behavior

For security concerns related to SBOM contents:
1. Contact the security team
2. Review security policy: [SECURITY.md](../../SECURITY.md)
3. Report vulnerabilities via private security advisory

---

**Last Updated**: 2026-02-14
**Version**: 1.0.0
**Maintainer**: DevOps & Security Team
