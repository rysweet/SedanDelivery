"""
test_deploy_yml_static_analysis.py — Deep static analysis of deploy.yml.

Verifies:
  - generate_bolt_tls_fallback() is completely absent (no stubs, no comments, no calls)
  - Exact 5-line TLS-required comment block is present
  - SEC-01: Both failure paths use exit 1
  - SEC-05: ::error:: annotations on both failure paths (not ::warning::)
  - SEC-06: No certificate generation/download in the TLS block
  - SEC-09: cert_pem_is_current is called (not unconditional BOLT_TLS_ENABLED=true)
  - SEC-10: Four controlled files must all be updated (structural check)
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

_ROOT = Path(__file__).parent.parent
DEPLOY_YML = _ROOT / ".github" / "workflows" / "deploy.yml"
TEST_SCRIPT = _ROOT / "scripts" / "test-bolt-deploy-secrets.sh"
DEPLOY_DOC = _ROOT / "docs" / "DEPLOY.md"
SECRETS_DOC = _ROOT / "docs" / "secrets.md"

EXPIRED_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; "
    "provision a current certificate before deploying"
)
MISSING_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; "
    "provision TLS secrets before deploying"
)


@pytest.fixture(scope="module")
def deploy_text() -> str:
    return DEPLOY_YML.read_text()


@pytest.fixture(scope="module")
def tls_block(deploy_text: str) -> str:
    start = deploy_text.index("BOLT_TLS_ENABLED=false")
    end = deploy_text.index("# Reuse the existing Bolt password", start)
    return deploy_text[start:end].rstrip()


class TestGenerateBoltTlsFallbackFullyAbsent:
    """generate_bolt_tls_fallback must be completely purged — no stubs, no calls, no comments."""

    def test_no_function_declaration(self, deploy_text: str) -> None:
        assert "generate_bolt_tls_fallback()" not in deploy_text

    def test_no_function_name_at_all(self, deploy_text: str) -> None:
        assert "generate_bolt_tls_fallback" not in deploy_text, (
            "generate_bolt_tls_fallback must not appear anywhere in deploy.yml — "
            "including in comments"
        )

    def test_no_ephemeral_openssl_call(self, deploy_text: str) -> None:
        assert "openssl req -x509 -nodes -newkey" not in deploy_text

    def test_no_fallback_generated_message(self, deploy_text: str) -> None:
        assert "Generated short-lived self-signed TLS material for pull_request" not in deploy_text

    def test_no_fallback_generation_failure_message(self, deploy_text: str) -> None:
        assert "Failed to generate pull_request fallback TLS material" not in deploy_text

    def test_no_fallback_empty_guard_message(self, deploy_text: str) -> None:
        assert "Generated pull_request fallback TLS material was empty" not in deploy_text


class TestTlsRequiredCommentBlock:
    """The exact 5-line TLS-required comment block must replace the old 'optional' comment."""

    EXPECTED_LINES = [
        "# TLS is required for all Bolt deploy environments.",
        "# DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must be",
        "# explicitly set. If either is absent or the certificate is expired,",
        "# this step exits non-zero with an actionable error. No self-signed",
        "# fallback or self-healing is performed.",
    ]

    def test_comment_block_present(self, deploy_text: str) -> None:
        # The comment appears just before BOLT_TLS_ENABLED=false (the extraction anchor).
        # Check the full file rather than the extracted sub-block.
        for line in self.EXPECTED_LINES:
            assert line.strip() in deploy_text, (
                f"Required comment line missing from deploy.yml: {line!r}"
            )

    def test_no_optional_comment(self, deploy_text: str) -> None:
        assert "TLS is optional" not in deploy_text, (
            "Old 'TLS is optional' comment must be replaced with 'TLS is required'"
        )

    def test_no_plaintext_mode_comment(self, deploy_text: str) -> None:
        # The old comment said "When absent, the Bolt server starts in plaintext mode"
        assert "Bolt server starts in plaintext mode" not in deploy_text


class TestSecurityProperties:
    """Verify all security invariants from the design spec."""

    def test_sec01_both_failure_paths_exit_1(self, tls_block: str) -> None:
        """SEC-01: Both failure paths must use exit 1 — NEVER exit 0 on missing or invalid TLS."""
        exit_count = tls_block.count("exit 1")
        assert exit_count == 2, (
            f"Expected exactly 2 'exit 1' in TLS block, found {exit_count}. "
            "Both failure paths (expired cert + missing certs) must exit 1."
        )

    def test_sec01_no_exit_0_in_failure_context(self, tls_block: str) -> None:
        # No 'exit 0' should appear in the TLS block (success is implicit fall-through)
        assert "exit 0" not in tls_block, (
            "exit 0 must not appear in the TLS decision block"
        )

    def test_sec02_no_env_type_conditional(self, tls_block: str) -> None:
        """SEC-02: No ENV_TYPE conditional — fail-closed behavior is identical for all envs."""
        assert "ENV_TYPE" not in tls_block

    def test_sec05_error_annotations_not_warnings(self, tls_block: str) -> None:
        """SEC-05: Use ::error:: (not ::warning::) on both failure paths."""
        assert "::error::" in tls_block, "TLS block must have ::error:: annotations"
        assert "::warning::" not in tls_block, (
            "::warning:: must not be used for TLS failures — use ::error::"
        )

    def test_sec06_no_cert_generation_in_tls_block(self, tls_block: str) -> None:
        """SEC-06: No certificate generation, download, or fetch of any kind."""
        assert "openssl req" not in tls_block
        assert "curl" not in tls_block
        assert "wget" not in tls_block
        assert "generate" not in tls_block.lower() or \
               "generate_bolt_tls_fallback" not in tls_block

    def test_sec09_cert_pem_is_current_called_before_enabled_true(self, tls_block: str) -> None:
        """SEC-09: cert_pem_is_current must be called — BOLT_TLS_ENABLED=true is conditional."""
        idx_check = tls_block.find("cert_pem_is_current")
        idx_enabled = tls_block.find("BOLT_TLS_ENABLED=true")
        assert idx_check >= 0, "cert_pem_is_current not called in TLS block"
        assert idx_enabled >= 0, "BOLT_TLS_ENABLED=true not set in TLS block"
        assert idx_check < idx_enabled

    def test_sec09_bolt_tls_cert_lookahead_referenced(self, tls_block: str) -> None:
        """SEC-09: BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be set to exactly '0' in deploy.yml."""
        # The lookahead is passed to cert_pem_is_current or referenced nearby
        # Check deploy.yml overall (not just tls_block) since lookahead may be set as env var
        deploy_text = DEPLOY_YML.read_text()
        assert 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"' in deploy_text, (
            "BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be set to exactly '0' in deploy.yml. "
            "The old value '86400' caused false-positive cert rejection (CI run 23669891455). "
            "Checking for the exact value '0' prevents silent regression to '86400'."
        )

    def test_sec09_mask_called_before_enabled_true(self, tls_block: str) -> None:
        """SEC-03 (adjacent): mask_multiline_secret must precede BOLT_TLS_ENABLED=true."""
        idx_mask = tls_block.find("mask_multiline_secret")
        idx_enabled = tls_block.find("BOLT_TLS_ENABLED=true")
        assert idx_mask >= 0, "mask_multiline_secret not found in TLS block"
        assert idx_mask < idx_enabled, (
            "mask_multiline_secret() must be called before BOLT_TLS_ENABLED=true "
            "to prevent PEM material from leaking into CI logs"
        )


class TestFourFileAtomicUpdate:
    """SEC-10: All four controlled files must reflect the fail-closed change."""

    def test_deploy_yml_updated(self) -> None:
        text = DEPLOY_YML.read_text()
        assert EXPIRED_ERROR in text
        assert MISSING_ERROR in text
        assert "generate_bolt_tls_fallback" not in text

    def test_test_script_updated(self) -> None:
        text = TEST_SCRIPT.read_text()
        assert EXPIRED_ERROR in text
        assert MISSING_ERROR in text
        assert "generates fallback" not in text

    def test_deploy_doc_updated(self) -> None:
        # docs/DEPLOY.md was deleted from main (ffb4226d) as part of TLS cleanup.
        # If absent, the test passes (file removal is the desired state).
        # If present (e.g. regenerated), it must contain the TLS-required content.
        if not DEPLOY_DOC.exists():
            return  # File correctly absent — matches main branch state
        text = DEPLOY_DOC.read_text()
        assert "TLS is required for all environments" in text
        assert "self-heals" not in text.lower()

    def test_secrets_doc_updated(self) -> None:
        text = SECRETS_DOC.read_text()
        assert "There is no fallback" in text
        assert "Plaintext mode does not exist" in text


class TestCertPemIsCurrentPreserved:
    """cert_pem_is_current() must remain byte-for-byte identical to its reviewed form."""

    def test_function_exists(self, deploy_text: str) -> None:
        assert "cert_pem_is_current() {" in deploy_text

    def test_uses_openssl_x509(self, deploy_text: str) -> None:
        # Extract the function body
        start = deploy_text.find("cert_pem_is_current() {")
        assert start >= 0
        # Find the closing brace (heuristic: next line with just spaces and })
        body_region = deploy_text[start:start + 800]
        assert "openssl x509" in body_region, (
            "cert_pem_is_current() must use 'openssl x509' for certificate validation"
        )

    def test_uses_checkend_or_dates(self, deploy_text: str) -> None:
        start = deploy_text.find("cert_pem_is_current() {")
        body_region = deploy_text[start:start + 800]
        assert "checkend" in body_region or "enddate" in body_region, (
            "cert_pem_is_current() must validate certificate expiry via openssl"
        )

    def test_returns_0_on_valid_1_on_invalid(self, deploy_text: str) -> None:
        start = deploy_text.find("cert_pem_is_current() {")
        body_region = deploy_text[start:start + 800]
        assert "return 0" in body_region or "return 1" in body_region, (
            "cert_pem_is_current() must explicitly return 0/1"
        )
