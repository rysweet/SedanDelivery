"""
test_deploy_workflow_validator.py — pytest suite for issue #1021 TLS fail-closed.

Verifies:
  - deploy.yml has no fallback/self-heal patterns (TestDeployYmlForbiddenPatterns)
  - deploy.yml has all required fail-closed patterns (TestDeployYmlRequiredPatterns)
  - test-bolt-deploy-secrets.sh has forbidden_snippets guards (TestValidatorForbiddenSnippets)
  - test-bolt-deploy-secrets.sh has updated required_snippets (TestValidatorRequiredSnippets)
  - run_tls_case() invocations reflect fail-closed semantics (TestRunTlsCaseInvocations)
  - docs/DEPLOY.md has TLS-required language (TestDeployDocCompliance)
  - docs/secrets.md has provisioning requirement (TestSecretsDocCompliance)
  - Error strings are consistent across controlled files (TestCrossFileConsistency)
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest

# ── File paths ────────────────────────────────────────────────────────────────
_ROOT = Path(__file__).parent.parent
DEPLOY_YML = _ROOT / ".github" / "workflows" / "deploy.yml"
TEST_SCRIPT = _ROOT / "scripts" / "test-bolt-deploy-secrets.sh"
DEPLOY_DOC = _ROOT / "docs" / "DEPLOY.md"
SECRETS_DOC = _ROOT / "docs" / "secrets.md"

# ── Exact error strings (single source of truth for all test classes) ─────────
EXPIRED_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; "
    "provision a current certificate before deploying"
)
MISSING_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; "
    "provision TLS secrets before deploying"
)


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="module")
def deploy_yml_text() -> str:
    return DEPLOY_YML.read_text()


@pytest.fixture(scope="module")
def test_script_text() -> str:
    return TEST_SCRIPT.read_text()


@pytest.fixture(scope="module")
def deploy_doc_text() -> str:
    return DEPLOY_DOC.read_text()


@pytest.fixture(scope="module")
def secrets_doc_text() -> str:
    return SECRETS_DOC.read_text()


@pytest.fixture(scope="module")
def tls_block(deploy_yml_text: str) -> str:
    """Extract the TLS decision block from deploy.yml."""
    start = deploy_yml_text.index("BOLT_TLS_ENABLED=false")
    end = deploy_yml_text.index("# Reuse the existing Bolt password", start)
    return deploy_yml_text[start:end].rstrip()


# ── §1+4: Forbidden patterns ──────────────────────────────────────────────────

class TestDeployYmlForbiddenPatterns:
    """deploy.yml must not contain any fallback/self-heal logic."""

    def test_no_ephemeral_cert_generation(self, deploy_yml_text: str) -> None:
        assert "openssl req -x509 -nodes -newkey" not in deploy_yml_text, (
            "Ephemeral cert generation command must not exist in deploy.yml"
        )

    def test_no_fallback_function_declaration(self, deploy_yml_text: str) -> None:
        assert "generate_bolt_tls_fallback" not in deploy_yml_text, (
            "generate_bolt_tls_fallback() must be fully deleted from deploy.yml"
        )

    def test_no_self_healing_prose(self, deploy_yml_text: str) -> None:
        assert "branch-environment fallback TLS" not in deploy_yml_text, (
            "Self-healing prose must not appear in deploy.yml"
        )

    def test_no_pr_fallback_message(self, deploy_yml_text: str) -> None:
        assert "self-signed TLS material for pull_request" not in deploy_yml_text, (
            "PR fallback message must not appear in deploy.yml"
        )

    def test_no_env_type_tls_gate(self, tls_block: str) -> None:
        assert "ENV_TYPE" not in tls_block, (
            "TLS decision block must not reference ENV_TYPE — "
            "fail-closed behavior must be identical for all environments"
        )

    def test_no_tls_warning_without_failure(self, deploy_yml_text: str) -> None:
        """::warning:: must not be used for TLS validation failures — only ::error::."""
        for line in deploy_yml_text.splitlines():
            if "::warning::" in line and (
                "TLS" in line
                or "cert" in line.lower()
                or "DEPLOY_BOLT_TLS" in line
                or "BOLT_TLS_ENABLED" in line
            ):
                pytest.fail(
                    f"::warning:: used for TLS condition (must be ::error::): {line.strip()!r}"
                )


# ── §2: Required patterns ─────────────────────────────────────────────────────

class TestDeployYmlRequiredPatterns:
    """deploy.yml must contain all fail-closed pattern anchors and error strings."""

    def test_cert_freshness_helper_exists(self, deploy_yml_text: str) -> None:
        assert "cert_pem_is_current() {" in deploy_yml_text

    def test_mask_multiline_secret_exists(self, deploy_yml_text: str) -> None:
        assert "mask_multiline_secret" in deploy_yml_text

    def test_bolt_tls_enabled_false(self, deploy_yml_text: str) -> None:
        assert "BOLT_TLS_ENABLED=false" in deploy_yml_text

    def test_bolt_tls_enabled_true(self, deploy_yml_text: str) -> None:
        assert "BOLT_TLS_ENABLED=true" in deploy_yml_text

    def test_expired_cert_error_message(self, deploy_yml_text: str) -> None:
        assert EXPIRED_ERROR in deploy_yml_text

    def test_missing_certs_error_message(self, deploy_yml_text: str) -> None:
        assert MISSING_ERROR in deploy_yml_text

    def test_error_annotation_expired_cert(self, deploy_yml_text: str) -> None:
        assert f"::error::{EXPIRED_ERROR}" in deploy_yml_text

    def test_error_annotation_missing_certs(self, deploy_yml_text: str) -> None:
        assert f"::error::{MISSING_ERROR}" in deploy_yml_text

    def test_exit_1_after_expired_error(self, deploy_yml_text: str) -> None:
        idx_error = deploy_yml_text.find(f"::error::{EXPIRED_ERROR}")
        assert idx_error >= 0, "Expired cert ::error:: annotation not found"
        idx_exit = deploy_yml_text.find("exit 1", idx_error)
        assert 0 <= idx_exit - idx_error <= 200, (
            "exit 1 must immediately follow the expired cert ::error:: annotation"
        )

    def test_exit_1_after_missing_certs_error(self, deploy_yml_text: str) -> None:
        idx_error = deploy_yml_text.find(f"::error::{MISSING_ERROR}")
        assert idx_error >= 0, "Missing certs ::error:: annotation not found"
        idx_exit = deploy_yml_text.find("exit 1", idx_error)
        assert 0 <= idx_exit - idx_error <= 200, (
            "exit 1 must immediately follow the missing certs ::error:: annotation"
        )

    def test_bolt_tls_enabled_true_only_after_cert_validation(self, deploy_yml_text: str) -> None:
        idx_check = deploy_yml_text.find("cert_pem_is_current")
        idx_enabled = deploy_yml_text.find("BOLT_TLS_ENABLED=true")
        assert idx_check >= 0, "cert_pem_is_current not found"
        assert idx_enabled >= 0, "BOLT_TLS_ENABLED=true not found"
        assert idx_check < idx_enabled, (
            "cert_pem_is_current() must appear before BOLT_TLS_ENABLED=true"
        )

    def test_mask_called_before_bolt_tls_enabled_true(self, tls_block: str) -> None:
        idx_mask = tls_block.find("mask_multiline_secret")
        idx_enabled = tls_block.find("BOLT_TLS_ENABLED=true")
        assert idx_mask >= 0, "mask_multiline_secret not found in TLS block"
        assert idx_enabled >= 0, "BOLT_TLS_ENABLED=true not found in TLS block"
        assert idx_mask < idx_enabled, (
            "mask_multiline_secret() must be called before BOLT_TLS_ENABLED=true"
        )

    def test_tls_required_comment_block(self, deploy_yml_text: str) -> None:
        # The comment precedes BOLT_TLS_ENABLED=false (extraction start anchor).
        # It describes the block, so we check the full deploy.yml, not the sub-block.
        assert "TLS is required for all Bolt deploy environments" in deploy_yml_text, (
            "TLS-required comment block must replace the old 'TLS is optional' comment"
        )

    def test_no_self_healing_in_tls_block(self, tls_block: str) -> None:
        assert "self-healing" not in tls_block.lower()
        assert "fallback" not in tls_block.lower()


# ── §5: Validator forbidden_snippets ─────────────────────────────────────────

class TestValidatorForbiddenSnippets:
    """test-bolt-deploy-secrets.sh must contain regression guards for all fallback patterns."""

    def test_forbidden_snippets_section_exists(self, test_script_text: str) -> None:
        assert "forbidden" in test_script_text.lower(), (
            "Test script must contain a forbidden_snippets section"
        )

    def test_no_ephemeral_cert_guard(self, test_script_text: str) -> None:
        assert "openssl req -x509 -nodes -newkey" in test_script_text, (
            "Ephemeral cert pattern must be in forbidden_snippets as a regression guard"
        )

    def test_no_fallback_function_guard(self, test_script_text: str) -> None:
        assert "generate_bolt_tls_fallback" in test_script_text, (
            "Fallback function name must be in forbidden_snippets as a regression guard"
        )

    def test_no_self_healing_guard(self, test_script_text: str) -> None:
        assert "branch-environment fallback TLS" in test_script_text, (
            "Self-healing prose must be in forbidden_snippets as a regression guard"
        )

    def test_no_pr_fallback_guard(self, test_script_text: str) -> None:
        assert "self-signed TLS material for pull_request" in test_script_text, (
            "PR fallback message must be in forbidden_snippets as a regression guard"
        )


# ── §6: Validator required_snippets ──────────────────────────────────────────

class TestValidatorRequiredSnippets:
    """test-bolt-deploy-secrets.sh required_snippets must use the new fail-closed error strings."""

    def test_required_snippets_section_exists(self, test_script_text: str) -> None:
        assert "required_snippets" in test_script_text

    def test_fail_closed_expired_cert_snippet(self, test_script_text: str) -> None:
        assert EXPIRED_ERROR in test_script_text, (
            f"Expired cert error string must be in required_snippets: {EXPIRED_ERROR!r}"
        )

    def test_fail_closed_missing_certs_snippet(self, test_script_text: str) -> None:
        assert MISSING_ERROR in test_script_text, (
            f"Missing certs error string must be in required_snippets: {MISSING_ERROR!r}"
        )

    def test_no_stale_branch_tls_expiry_fallback_warning(self, test_script_text: str) -> None:
        assert "branch tls expiry fallback warning" not in test_script_text, (
            "Stale 'branch tls expiry fallback warning' key must be removed from required_snippets"
        )

    def test_no_stale_pr_tls_fallback_generation(self, test_script_text: str) -> None:
        assert "pr tls fallback generation" not in test_script_text, (
            "Stale 'pr tls fallback generation' key must be removed from required_snippets"
        )

    def test_no_stale_branch_tls_missing_fallback_warning(self, test_script_text: str) -> None:
        assert "branch tls missing fallback warning" not in test_script_text, (
            "Stale 'branch tls missing fallback warning' key must be removed from required_snippets"
        )

    def test_no_stale_ephemeral_cert_generation_in_required(self, test_script_text: str) -> None:
        # 'ephemeral cert generation' must now be in forbidden_snippets, not required_snippets
        assert "ephemeral cert generation" not in test_script_text or (
            "no ephemeral cert generation" in test_script_text
        ), (
            "'ephemeral cert generation' must move from required_snippets to forbidden_snippets"
        )


# ── §7: run_tls_case() invocations ───────────────────────────────────────────

class TestRunTlsCaseInvocations:
    """run_tls_case() test cases must reflect fail-closed semantics, not fallback."""

    def test_run_tls_case_function_exists(self, test_script_text: str) -> None:
        assert "run_tls_case" in test_script_text

    def test_branch_missing_certs_fails_closed(self, test_script_text: str) -> None:
        assert "branch env missing certs fails closed" in test_script_text, (
            "Test case 'branch env missing certs fails closed' must exist"
        )

    def test_branch_expired_cert_fails_closed(self, test_script_text: str) -> None:
        assert "branch env expired cert fails closed" in test_script_text, (
            "Test case 'branch env expired cert fails closed' must exist"
        )

    def test_valid_cert_happy_path(self, test_script_text: str) -> None:
        assert "valid cert happy path" in test_script_text, (
            "Regression test 'valid cert happy path' must exist (expect_exit=0, enabled=true)"
        )

    def test_no_fallback_expect_invocations(self, test_script_text: str) -> None:
        assert "generates fallback" not in test_script_text, (
            "Old fallback-expect invocations must be replaced with fail-closed cases"
        )

    def test_no_generate_bolt_tls_fallback_stub_in_harness(self, test_script_text: str) -> None:
        # The harness must not define generate_bolt_tls_fallback() as a mock
        # (It no longer exists in deploy.yml, so no stub needed)
        # We check that the harness f-string does not contain the function body pattern
        assert "generate_bolt_tls_fallback() {" not in test_script_text or (
            "no fallback function" in test_script_text
        ), (
            "generate_bolt_tls_fallback() stub must not exist in run_tls_case() harness; "
            "it may only appear in forbidden_snippets check"
        )


# ── §8: docs/DEPLOY.md compliance ────────────────────────────────────────────

class TestDeployDocCompliance:
    """docs/DEPLOY.md must reflect fail-closed TLS requirements."""

    def test_no_self_heal_language(self, deploy_doc_text: str) -> None:
        assert "self-heal" not in deploy_doc_text.lower(), (
            "Self-healing language must be removed from DEPLOY.md"
        )

    def test_tls_required_language_present(self, deploy_doc_text: str) -> None:
        assert "TLS is required for all environments" in deploy_doc_text, (
            "DEPLOY.md must state that TLS is required for all environments"
        )

    def test_no_fallback_paragraph(self, deploy_doc_text: str) -> None:
        assert "short-lived self-signed certificate" not in deploy_doc_text, (
            "Fallback self-signed certificate paragraph must be removed from DEPLOY.md"
        )

    def test_fail_closed_language_present(self, deploy_doc_text: str) -> None:
        assert "There is no fallback" in deploy_doc_text, (
            "DEPLOY.md must explicitly state 'There is no fallback'"
        )

    def test_provision_instruction_present(self, deploy_doc_text: str) -> None:
        assert "Provision current, valid TLS secrets before triggering any deploy" in deploy_doc_text

    def test_no_plaintext_mode_section(self, deploy_doc_text: str) -> None:
        assert "Bolt container starts in\nplaintext mode" not in deploy_doc_text and \
               "plaintext mode without `BOLT_REQUIRE_TLS`" not in deploy_doc_text, (
            "Plaintext-mode fallback section must be removed from DEPLOY.md"
        )


# ── §9: docs/secrets.md compliance ───────────────────────────────────────────

class TestSecretsDocCompliance:
    """docs/secrets.md must document DEPLOY_BOLT_TLS_* and fail-closed requirement."""

    def test_deploy_bolt_tls_cert_pem_present(self, secrets_doc_text: str) -> None:
        assert "DEPLOY_BOLT_TLS_CERT_PEM" in secrets_doc_text

    def test_deploy_bolt_tls_key_pem_present(self, secrets_doc_text: str) -> None:
        assert "DEPLOY_BOLT_TLS_KEY_PEM" in secrets_doc_text

    def test_fail_closed_statement_present(self, secrets_doc_text: str) -> None:
        assert "There is no fallback" in secrets_doc_text, (
            "secrets.md must explicitly state 'There is no fallback'"
        )

    def test_no_plaintext_exists_statement(self, secrets_doc_text: str) -> None:
        assert "Plaintext mode does not exist" in secrets_doc_text, (
            "secrets.md must state 'Plaintext mode does not exist'"
        )

    def test_provisioning_requirement_present(self, secrets_doc_text: str) -> None:
        lower = secrets_doc_text.lower()
        assert "must" in lower and "provid" in lower or "must" in lower and "configured" in lower, (
            "secrets.md must contain an explicit provisioning requirement"
        )

    def test_no_pr_fallback_description(self, secrets_doc_text: str) -> None:
        assert "fall back to `pr-" not in secrets_doc_text, (
            "PR fallback description must be removed from secrets.md"
        )


# ── §10: Cross-file consistency ───────────────────────────────────────────────

class TestCrossFileConsistency:
    """Error strings must be identical in all four controlled files."""

    def test_expired_error_in_deploy_yml(self, deploy_yml_text: str) -> None:
        assert EXPIRED_ERROR in deploy_yml_text

    def test_expired_error_in_test_script(self, test_script_text: str) -> None:
        assert EXPIRED_ERROR in test_script_text

    def test_missing_error_in_deploy_yml(self, deploy_yml_text: str) -> None:
        assert MISSING_ERROR in deploy_yml_text

    def test_missing_error_in_test_script(self, test_script_text: str) -> None:
        assert MISSING_ERROR in test_script_text

    def test_tls_block_anchors_exist(self, deploy_yml_text: str) -> None:
        """Both extraction anchors used by run_tls_case() harness must be present."""
        assert "BOLT_TLS_ENABLED=false" in deploy_yml_text, (
            "BOLT_TLS_ENABLED=false anchor missing from deploy.yml"
        )
        assert "# Reuse the existing Bolt password" in deploy_yml_text, (
            "'# Reuse the existing Bolt password' anchor missing from deploy.yml"
        )

    def test_tls_block_anchor_ordering(self, deploy_yml_text: str) -> None:
        """BOLT_TLS_ENABLED=false must precede '# Reuse the existing Bolt password'."""
        idx_start = deploy_yml_text.find("BOLT_TLS_ENABLED=false")
        idx_end = deploy_yml_text.find("# Reuse the existing Bolt password", idx_start)
        assert idx_start >= 0
        assert idx_end > idx_start, (
            "BOLT_TLS_ENABLED=false must precede '# Reuse the existing Bolt password'"
        )
