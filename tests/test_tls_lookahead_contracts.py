"""
test_tls_lookahead_contracts.py — TDD contracts for BOLT_TLS_CERT_LOOKAHEAD_SECONDS fix.

Step 7 of TDD: FAILING tests that define the contracts for:

  CONTRACT-1  BOLT_TLS_CERT_LOOKAHEAD_SECONDS is exactly "0" in deploy.yml
              (stronger than existing test_sec09 which only checks variable name presence)

  CONTRACT-2  cert_pem_is_current() has an explicit integer validation guard
              that exits 1 (fail-closed) when the lookahead value is non-integer.
              Without this guard, a malformed env var silently passes to openssl,
              whose -checkend error is opaque and hard to diagnose.

  CONTRACT-3  docs/reference/bolt-tls-deploy.md exists and contains required content:
              - Semantics table with checkend 0 / checkend N distinction
              - "Required" external monitoring section (not optional)
              - BOLT_TLS_CERT_LOOKAHEAD_SECONDS configuration reference

  CONTRACT-4  docs/howto/rotate-bolt-tls-cert.md exists and contains:
              - Step using checkend 0 (not checkend 86400)
              - "When NOT to Rotate" section preventing unnecessary rotations

  CONTRACT-5  docs/howto/provision-tls-secrets.md reflects the "0" default
              (not the old LOOKAHEAD:-86400)

  CONTRACT-6  docs/index.md links to both new docs

Expected status on first run (before implementation):
  - CONTRACT-1: PASS (value already changed to "0")
  - CONTRACT-2: FAIL (integer guard not yet added to cert_pem_is_current)
  - CONTRACT-3: FAIL (docs/reference/bolt-tls-deploy.md does not exist)
  - CONTRACT-4: FAIL (docs/howto/rotate-bolt-tls-cert.md does not exist)
  - CONTRACT-5: FAIL (docs/howto/provision-tls-secrets.md may not reflect "0" default)
  - CONTRACT-6: FAIL (docs/index.md links may not exist yet)
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

_ROOT = Path(__file__).parent.parent
DEPLOY_YML = _ROOT / ".github" / "workflows" / "deploy.yml"

BOLT_TLS_DEPLOY_REF = _ROOT / "docs" / "reference" / "bolt-tls-deploy.md"
ROTATE_HOWTO = _ROOT / "docs" / "howto" / "rotate-bolt-tls-cert.md"
PROVISION_HOWTO = _ROOT / "docs" / "howto" / "provision-tls-secrets.md"
DOCS_INDEX = _ROOT / "docs" / "index.md"


# ── Fixtures ──────────────────────────────────────────────────────────────────

@pytest.fixture(scope="module")
def deploy_text() -> str:
    return DEPLOY_YML.read_text()


@pytest.fixture(scope="module")
def cert_fn_body(deploy_text: str) -> str:
    """Extract the cert_pem_is_current() function body from deploy.yml."""
    start = deploy_text.find("cert_pem_is_current() {")
    assert start >= 0, "cert_pem_is_current() not found in deploy.yml"
    # Find the closing brace — look for a line that is just spaces + '}'
    region = deploy_text[start : start + 1200]
    # The function ends at the first unindented/same-indent closing brace
    # Heuristic: find the } that closes the function (appears as standalone '          }')
    end_match = re.search(r'\n\s{10}\}', region)
    if end_match:
        return region[: end_match.end()]
    return region


# ── CONTRACT-1: Exact value assertion ─────────────────────────────────────────

class TestLookaheadValueIsZero:
    """CONTRACT-1: BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be exactly '0', not '86400'."""

    def test_lookahead_value_is_exactly_zero(self, deploy_text: str) -> None:
        """The env var must be set to the string '0', not 86400 or any other value.

        This is stronger than the existing test_sec09 which only checks that the
        variable name is referenced. A future commit could silently reintroduce '86400'
        and the old test would not catch it.
        """
        assert 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"' in deploy_text, (
            "BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be exactly '0'. "
            "The old value '86400' caused false-positive cert rejection for "
            "certs valid now but expiring within 24h."
        )

    def test_lookahead_value_is_not_86400(self, deploy_text: str) -> None:
        """The old 86400 value must not appear as the env var value."""
        assert 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "86400"' not in deploy_text, (
            "BOLT_TLS_CERT_LOOKAHEAD_SECONDS: '86400' is the regression we fixed. "
            "It must not be reintroduced."
        )

    def test_comment_explains_zero_semantics(self, deploy_text: str) -> None:
        """The comment near BOLT_TLS_CERT_LOOKAHEAD_SECONDS must explain what '0' means.

        Operators reading the YAML should understand that '0' means 'reject only
        truly expired certs', not 'no validation'.
        """
        # Find the context around the env var declaration
        idx = deploy_text.find('BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"')
        assert idx >= 0, "BOLT_TLS_CERT_LOOKAHEAD_SECONDS: '0' not found"
        # Check within 300 chars before this line for the explanatory comment
        context = deploy_text[max(0, idx - 300) : idx + 50]
        assert "Default 0 means only truly expired certs are rejected" in context, (
            "The comment near BOLT_TLS_CERT_LOOKAHEAD_SECONDS must explain that "
            "'0' means only truly expired certs are rejected (not no validation). "
            "This prevents operators from misreading '0' as 'disabled'."
        )


# ── CONTRACT-2: Integer validation guard ─────────────────────────────────────

class TestIntegerValidationGuard:
    """CONTRACT-2: cert_pem_is_current() must validate lookahead is a non-negative integer.

    Without this guard, a malformed BOLT_TLS_CERT_LOOKAHEAD_SECONDS (e.g. 'abc',
    '3600s', or '') passes through to openssl -checkend. openssl rejects the value
    with a cryptic error, which openssl's return code obscures — the function may
    return 1 (invalid cert) when the cert is actually valid, causing a false-positive
    deploy failure with no actionable error message.

    The guard converts this into an auditable fail-closed path with a clear message.
    """

    def test_integer_guard_present_in_function(self, cert_fn_body: str) -> None:
        """cert_pem_is_current() must check that lookahead_seconds is a non-negative integer."""
        # The guard should use a regex or arithmetic test to validate the value
        has_regex_guard = (
            re.search(r'\[\[.*lookahead_seconds.*=~.*\^\[0-9\]', cert_fn_body) is not None
            or re.search(r'\[\[.*lookahead_seconds.*=~.*\^\+\?[0-9\]]', cert_fn_body) is not None
            or "[[ ${lookahead_seconds}" in cert_fn_body
            or "=~ ^[0-9]" in cert_fn_body
        )
        assert has_regex_guard, (
            "cert_pem_is_current() must validate that lookahead_seconds is a "
            "non-negative integer (e.g. [[ ${lookahead_seconds} =~ ^[0-9]+$ ]]) "
            "before passing it to openssl -checkend. Without this, a malformed "
            "BOLT_TLS_CERT_LOOKAHEAD_SECONDS causes an opaque openssl failure."
        )

    def test_integer_guard_has_error_message(self, cert_fn_body: str) -> None:
        """The integer guard must emit an actionable ::error:: message, not silently exit 1."""
        # If the guard is there, it should tell the operator what went wrong
        assert "::error::" in cert_fn_body or "invalid" in cert_fn_body.lower(), (
            "cert_pem_is_current() integer guard must emit an error message "
            "explaining that BOLT_TLS_CERT_LOOKAHEAD_SECONDS is not a valid integer. "
            "Silent exit 1 is indistinguishable from an expired cert."
        )

    def test_integer_guard_exits_one_on_bad_input(self, cert_fn_body: str) -> None:
        """The integer guard must exit/return 1 (fail-closed) on malformed input."""
        # The function should return 1 on invalid lookahead (fail-closed invariant)
        lines = cert_fn_body.splitlines()
        guard_line_idx = next(
            (i for i, l in enumerate(lines) if "lookahead" in l and ("=~" in l or "guard" in l.lower())),
            None
        )
        assert guard_line_idx is not None, (
            "Integer validation guard line not found in cert_pem_is_current()"
        )
        # Check that return 1 or exit 1 appears within a few lines of the guard
        guard_context = "\n".join(lines[guard_line_idx : guard_line_idx + 6])
        assert "return 1" in guard_context or "exit 1" in guard_context, (
            "Integer guard must return 1 (fail-closed) when lookahead_seconds "
            "is not a valid non-negative integer."
        )


# ── CONTRACT-3: docs/reference/bolt-tls-deploy.md ────────────────────────────

class TestBoltTlsDeployReferenceDoc:
    """CONTRACT-3: docs/reference/bolt-tls-deploy.md must exist with required content."""

    def test_reference_doc_exists(self) -> None:
        assert BOLT_TLS_DEPLOY_REF.exists(), (
            f"{BOLT_TLS_DEPLOY_REF} does not exist. "
            "This reference doc was specified in the architect refinements for issue #1021. "
            "It must explain BOLT_TLS_CERT_LOOKAHEAD_SECONDS semantics and cert validation."
        )

    def test_reference_doc_explains_checkend_zero(self) -> None:
        text = BOLT_TLS_DEPLOY_REF.read_text()
        assert "checkend 0" in text or "checkend ${" in text, (
            "docs/reference/bolt-tls-deploy.md must explain what 'checkend 0' means: "
            "only truly expired certs are rejected. Operators need this to understand "
            "why a cert expiring in 1h will still pass the deploy gate."
        )

    def test_reference_doc_monitoring_is_required_not_optional(self) -> None:
        """External cert monitoring must be framed as Required, not Optional.

        With lookahead=0, a cert can expire between deploys with no CI warning.
        The monitoring section must make clear this is a compensating control
        required by the removal of the 24h lookahead window.
        """
        text = BOLT_TLS_DEPLOY_REF.read_text()
        # Must NOT say "Optional" or "recommended" for monitoring
        assert "Required" in text or "required" in text, (
            "Monitoring section in bolt-tls-deploy.md must be framed as 'Required', "
            "not optional. With lookahead=0, external monitoring is the only mechanism "
            "that warns operators about upcoming cert expiry."
        )
        # Monitoring must not be framed as merely optional
        lines = text.splitlines()
        monitoring_section_lines = [
            l for l in lines
            if "monitor" in l.lower() or "expir" in l.lower()
        ]
        optional_framing = any(
            "optional" in l.lower() or "recommended" in l.lower()
            for l in monitoring_section_lines
        )
        assert not optional_framing, (
            "Monitoring must not be described as 'optional' or 'recommended'. "
            "It is a required compensating control when BOLT_TLS_CERT_LOOKAHEAD_SECONDS=0."
        )

    def test_reference_doc_explains_86400_false_positive(self) -> None:
        """The doc must explain WHY 86400 caused false positives."""
        text = BOLT_TLS_DEPLOY_REF.read_text()
        assert "86400" in text or "24h" in text or "24 hour" in text, (
            "docs/reference/bolt-tls-deploy.md must explain that the old '86400' value "
            "caused false-positive CI failures for certs expiring within 24h but valid now. "
            "Operators need this context to understand the fix and avoid re-introducing it."
        )

    def test_reference_doc_has_lookahead_config_reference(self) -> None:
        text = BOLT_TLS_DEPLOY_REF.read_text()
        assert "BOLT_TLS_CERT_LOOKAHEAD_SECONDS" in text, (
            "docs/reference/bolt-tls-deploy.md must document the BOLT_TLS_CERT_LOOKAHEAD_SECONDS "
            "env var — its default value, semantics, and when to change it."
        )


# ── CONTRACT-4: docs/howto/rotate-bolt-tls-cert.md ──────────────────────────

class TestRotateCertHowtoDoc:
    """CONTRACT-4: docs/howto/rotate-bolt-tls-cert.md must exist with correct checkend usage."""

    def test_rotate_howto_exists(self) -> None:
        assert ROTATE_HOWTO.exists(), (
            f"{ROTATE_HOWTO} does not exist. "
            "Architects specified this how-to in refinement C. "
            "Without it, operators may test cert validity with 'checkend 86400' "
            "and incorrectly rotate a cert the gate would actually accept."
        )

    def test_rotate_howto_uses_checkend_zero(self) -> None:
        """Step 1 must use 'checkend 0' to match the deploy gate's zero-lookahead default."""
        text = ROTATE_HOWTO.read_text()
        assert "checkend 0" in text, (
            "docs/howto/rotate-bolt-tls-cert.md Step 1 must use 'openssl x509 -checkend 0' "
            "to match the gate's zero-lookahead. Using 'checkend 86400' would report failure "
            "for a cert the deploy gate would actually accept, causing unnecessary rotations."
        )

    def test_rotate_howto_warns_against_checkend_86400(self) -> None:
        """The doc must explicitly warn that checkend 86400 differs from the gate."""
        text = ROTATE_HOWTO.read_text()
        has_warning = (
            "checkend 86400" in text and
            ("may report failure" in text or "do not rotate" in text.lower() or "warning" in text.lower())
        )
        assert has_warning, (
            "docs/howto/rotate-bolt-tls-cert.md must warn that 'checkend 86400' may report "
            "failure even when the deploy gate would accept the cert. Operators who use the "
            "wrong checkend value will rotate unnecessarily."
        )

    def test_rotate_howto_has_when_not_to_rotate_section(self) -> None:
        """'When NOT to Rotate' section prevents unnecessary rotations."""
        text = ROTATE_HOWTO.read_text()
        assert "When NOT to Rotate" in text or "when not to rotate" in text.lower(), (
            "docs/howto/rotate-bolt-tls-cert.md must have a 'When NOT to Rotate' section "
            "(architect refinement D). This section prevents operators from rotating a cert "
            "that would actually pass the deploy gate, saving unnecessary downtime risk."
        )


# ── CONTRACT-5: docs/howto/provision-tls-secrets.md ─────────────────────────

class TestProvisionTlsSecretsHowto:
    """CONTRACT-5: docs/howto/provision-tls-secrets.md must use checkend 0, not checkend 86400."""

    def test_provision_howto_exists(self) -> None:
        assert PROVISION_HOWTO.exists(), (
            f"{PROVISION_HOWTO} does not exist. "
            "This how-to was updated as a bonus accuracy fix for issue #1021."
        )

    def test_provision_howto_uses_checkend_zero(self) -> None:
        """Step 2 must use checkend 0 to match the gate's actual behavior."""
        text = PROVISION_HOWTO.read_text()
        assert "checkend 0" in text, (
            "docs/howto/provision-tls-secrets.md must use 'checkend 0' (not 'checkend 86400' "
            "or 'LOOKAHEAD:-86400') to match the deploy gate's zero-lookahead default. "
            "The old value causes operators to provision unnecessarily."
        )

    def test_provision_howto_not_using_old_lookahead_default(self) -> None:
        """The old LOOKAHEAD:-86400 default must be replaced."""
        text = PROVISION_HOWTO.read_text()
        assert "LOOKAHEAD:-86400" not in text and "checkend 86400" not in text, (
            "docs/howto/provision-tls-secrets.md must not reference the old '86400' default. "
            "The correct default is now 0."
        )


# ── CONTRACT-6: docs/index.md links ──────────────────────────────────────────

class TestDocsIndexLinks:
    """CONTRACT-6: docs/index.md must link to both new docs."""

    def test_index_exists(self) -> None:
        assert DOCS_INDEX.exists(), (
            f"{DOCS_INDEX} does not exist. "
            "The index was rebuilt as part of the issue #1021 documentation pass."
        )

    def test_index_links_to_bolt_tls_reference(self) -> None:
        text = DOCS_INDEX.read_text()
        assert "bolt-tls-deploy" in text, (
            "docs/index.md must link to docs/reference/bolt-tls-deploy.md. "
            "Without this link, operators cannot discover the TLS cert gate documentation."
        )

    def test_index_links_to_rotate_howto(self) -> None:
        text = DOCS_INDEX.read_text()
        assert "rotate-bolt-tls-cert" in text, (
            "docs/index.md must link to docs/howto/rotate-bolt-tls-cert.md. "
            "Without this link, operators cannot find cert rotation instructions."
        )

    def test_index_links_to_provision_howto(self) -> None:
        text = DOCS_INDEX.read_text()
        assert "provision-tls-secrets" in text, (
            "docs/index.md must link to docs/howto/provision-tls-secrets.md. "
            "Without this link, operators cannot find the provisioning guide."
        )


# ── CONTRACT-7: Regression guard — test_sec09 value assertion ─────────────────

class TestSec09ValueAssertion:
    """CONTRACT-7: test_deploy_yml_static_analysis.py must assert exact value '0', not just name.

    The existing test_sec09_bolt_tls_cert_lookahead_referenced only checks that
    'BOLT_TLS_CERT_LOOKAHEAD_SECONDS' appears in deploy.yml. A future commit
    could change the value back to '86400' and the test would still pass.

    This test verifies that the static analysis test has been strengthened to
    assert the exact value.
    """

    _STATIC_TEST = _ROOT / "tests" / "test_deploy_yml_static_analysis.py"

    def test_static_analysis_test_asserts_value_not_just_name(self) -> None:
        text = self._STATIC_TEST.read_text()
        # The strengthened test must assert the exact value '0', not just variable name
        has_value_assertion = (
            '"0"' in text and "BOLT_TLS_CERT_LOOKAHEAD_SECONDS" in text
            and (
                'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: \\"0\\"' in text
                or "BOLT_TLS_CERT_LOOKAHEAD_SECONDS: \"0\"" in text
                or "'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: \"0\"'" in text
            )
        )
        assert has_value_assertion, (
            "test_deploy_yml_static_analysis.py test_sec09 must assert the exact value "
            "'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: \"0\"' rather than just checking the "
            "variable name is referenced. Without this, a regression to '86400' would "
            "not be caught by the existing test."
        )
