"""
test_tls_block_extraction.py — Structural tests for the TLS decision block.

Verifies the block that run_tls_case() extracts from deploy.yml:
  - Both anchor strings exist and are correctly ordered
  - The extracted block has the expected structure
  - The block content satisfies fail-closed semantics
  - No fallback or self-healing constructs appear in the block
"""

from __future__ import annotations

from pathlib import Path

import pytest

_ROOT = Path(__file__).parent.parent
DEPLOY_YML = _ROOT / ".github" / "workflows" / "deploy.yml"

EXPIRED_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; "
    "provision a current certificate before deploying"
)
MISSING_ERROR = (
    "DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; "
    "provision TLS secrets before deploying"
)

START_ANCHOR = "BOLT_TLS_ENABLED=false"
END_ANCHOR = "# Reuse the existing Bolt password"


@pytest.fixture(scope="module")
def deploy_text() -> str:
    return DEPLOY_YML.read_text()


@pytest.fixture(scope="module")
def tls_block(deploy_text: str) -> str:
    """Replicate the extraction logic used by run_tls_case() harness."""
    start = deploy_text.index(START_ANCHOR)
    end = deploy_text.index(END_ANCHOR, start)
    return deploy_text[start:end].rstrip()


class TestAnchorIntegrity:
    """Both extraction anchors must exist and be correctly ordered."""

    def test_start_anchor_present(self, deploy_text: str) -> None:
        assert START_ANCHOR in deploy_text, (
            f"Start anchor {START_ANCHOR!r} not found in deploy.yml"
        )

    def test_end_anchor_present(self, deploy_text: str) -> None:
        assert END_ANCHOR in deploy_text, (
            f"End anchor {END_ANCHOR!r} not found in deploy.yml"
        )

    def test_start_before_end(self, deploy_text: str) -> None:
        start = deploy_text.find(START_ANCHOR)
        end = deploy_text.find(END_ANCHOR, start)
        assert start >= 0
        assert end > start, (
            f"End anchor must appear after start anchor in deploy.yml"
        )

    def test_exactly_one_start_anchor(self, deploy_text: str) -> None:
        count = deploy_text.count(START_ANCHOR)
        assert count == 1, (
            f"Expected exactly 1 occurrence of {START_ANCHOR!r}, found {count}"
        )

    def test_block_is_non_empty(self, tls_block: str) -> None:
        assert len(tls_block) > 50, (
            "TLS decision block is unexpectedly short — extraction may be broken"
        )


class TestBlockContent:
    """The extracted TLS block must contain all fail-closed logic."""

    def test_bolt_tls_enabled_initialised_false(self, tls_block: str) -> None:
        assert tls_block.startswith("BOLT_TLS_ENABLED=false"), (
            "TLS block must begin with BOLT_TLS_ENABLED=false"
        )

    def test_bolt_tls_enabled_set_true(self, tls_block: str) -> None:
        assert "BOLT_TLS_ENABLED=true" in tls_block

    def test_expired_error_string_present(self, tls_block: str) -> None:
        assert EXPIRED_ERROR in tls_block

    def test_missing_error_string_present(self, tls_block: str) -> None:
        assert MISSING_ERROR in tls_block

    def test_error_annotation_for_expired(self, tls_block: str) -> None:
        assert f"::error::{EXPIRED_ERROR}" in tls_block

    def test_error_annotation_for_missing(self, tls_block: str) -> None:
        assert f"::error::{MISSING_ERROR}" in tls_block

    def test_cert_pem_is_current_called(self, tls_block: str) -> None:
        assert "cert_pem_is_current" in tls_block

    def test_mask_multiline_secret_called(self, tls_block: str) -> None:
        assert "mask_multiline_secret" in tls_block

    def test_required_comment_block(self, deploy_text: str) -> None:
        # The comment appears just before BOLT_TLS_ENABLED=false (the extraction anchor),
        # so we check the full file rather than the extracted sub-block.
        assert "TLS is required for all Bolt deploy environments" in deploy_text

    def test_tls_required_comment_no_fallback(self, deploy_text: str) -> None:
        # Verify the comment in deploy.yml explicitly states no fallback/self-healing.
        assert "self-healing is performed" in deploy_text or \
               "No self-signed" in deploy_text or \
               "no self-signed" in deploy_text


class TestBlockStructure:
    """The TLS block must have the correct if/else structure."""

    def test_outer_if_checks_both_secrets(self, tls_block: str) -> None:
        assert "DEPLOY_BOLT_TLS_CERT_PEM" in tls_block
        assert "DEPLOY_BOLT_TLS_KEY_PEM" in tls_block

    def test_inner_if_calls_cert_pem_is_current(self, tls_block: str) -> None:
        # cert_pem_is_current must be called inside the if -n CERT && -n KEY block
        idx_outer_if = tls_block.find("DEPLOY_BOLT_TLS_CERT_PEM")
        idx_cert_check = tls_block.find("cert_pem_is_current", idx_outer_if)
        assert idx_cert_check > idx_outer_if

    def test_exit_1_present_twice(self, tls_block: str) -> None:
        count = tls_block.count("exit 1")
        assert count == 2, (
            f"Expected exactly 2 'exit 1' statements in TLS block (one per failure path), found {count}"
        )

    def test_no_env_type_reference(self, tls_block: str) -> None:
        assert "ENV_TYPE" not in tls_block, (
            "TLS block must not reference ENV_TYPE — behavior must be environment-agnostic"
        )

    def test_no_generate_fallback_call(self, tls_block: str) -> None:
        assert "generate_bolt_tls_fallback" not in tls_block

    def test_no_fallback_called_variable(self, tls_block: str) -> None:
        assert "fallback_called" not in tls_block

    def test_no_branch_conditional(self, tls_block: str) -> None:
        assert '"branch"' not in tls_block

    def test_mask_before_enabled_true(self, tls_block: str) -> None:
        idx_mask = tls_block.find("mask_multiline_secret")
        idx_enabled = tls_block.find("BOLT_TLS_ENABLED=true")
        assert idx_mask < idx_enabled, (
            "mask_multiline_secret() must be called before BOLT_TLS_ENABLED=true"
        )

    def test_no_warning_annotations(self, tls_block: str) -> None:
        assert "::warning::" not in tls_block, (
            "::warning:: must not appear in TLS block — use ::error:: and exit 1"
        )


class TestBlockFunctional:
    """Run the extracted TLS block with mock stubs and verify exit codes."""

    def _run_case(
        self,
        tls_block: str,
        *,
        cert_present: bool,
        cert_current: bool,
    ) -> tuple[int, str]:
        """Execute the TLS block in a bash subshell with simulated inputs."""
        import subprocess

        cert_val = "configured-cert" if cert_present else ""
        key_val = "configured-key" if cert_present else ""
        harness = f"""set -euo pipefail
mask_multiline_secret() {{ :; }}
cert_pem_is_current() {{
  [[ "${{SIM_CERT_CURRENT}}" == "true" ]]
}}
DEPLOY_BOLT_TLS_CERT_PEM="{cert_val}"
DEPLOY_BOLT_TLS_KEY_PEM="{key_val}"
SIM_CERT_CURRENT={"true" if cert_current else "false"}
{tls_block}
printf 'enabled=%s\\n' "${{BOLT_TLS_ENABLED}}"
"""
        result = subprocess.run(
            ["bash", "-c", harness],
            capture_output=True,
            text=True,
        )
        return result.returncode, result.stdout

    def test_missing_certs_exits_1(self, tls_block: str) -> None:
        rc, _ = self._run_case(tls_block, cert_present=False, cert_current=False)
        assert rc == 1, "Missing certs must exit 1"

    def test_expired_cert_exits_1(self, tls_block: str) -> None:
        rc, _ = self._run_case(tls_block, cert_present=True, cert_current=False)
        assert rc == 1, "Expired cert must exit 1"

    def test_valid_cert_exits_0(self, tls_block: str) -> None:
        rc, stdout = self._run_case(tls_block, cert_present=True, cert_current=True)
        assert rc == 0, f"Valid cert must exit 0, got {rc}"
        assert "enabled=true" in stdout, f"Expected enabled=true in stdout: {stdout!r}"

    def test_missing_key_only_exits_1(self, tls_block: str) -> None:
        """Providing cert but not key must exit 1 (missing certs path)."""
        import subprocess

        harness = f"""set -euo pipefail
mask_multiline_secret() {{ :; }}
cert_pem_is_current() {{ return 0; }}
DEPLOY_BOLT_TLS_CERT_PEM="some-cert"
DEPLOY_BOLT_TLS_KEY_PEM=""
SIM_CERT_CURRENT=true
{tls_block}
printf 'enabled=%s\\n' "${{BOLT_TLS_ENABLED}}"
"""
        result = subprocess.run(["bash", "-c", harness], capture_output=True, text=True)
        assert result.returncode == 1, (
            "Missing DEPLOY_BOLT_TLS_KEY_PEM must exit 1 even if cert is present"
        )
