#!/usr/bin/env bash
# test-tls-fail-closed.sh — TDD test suite for issue #1021 TLS fail-closed requirements.
#
# Verifies that:
#  §1  generate_bolt_tls_fallback() is absent from deploy.yml
#  §2  Fail-closed TLS block exists with exact error strings
#  §3  cert_pem_is_current() is present and preserved
#  §4  No ENV_TYPE-gated TLS behavior
#  §5  forbidden_snippets regression guards are in the validator
#  §6  required_snippets sync with deploy.yml error strings
#  §7  run_tls_case() invocations reflect fail-closed semantics
#  §8  docs/DEPLOY.md has fail-closed language
#  §9  docs/secrets.md has provisioning requirement
#  §10 Cross-file error string consistency
#
# Usage:
#   bash scripts/test-tls-fail-closed.sh
#
# Exit: 0 if all assertions pass, 1 if any fail.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

DEPLOY_YML="$PROJECT_DIR/.github/workflows/deploy.yml"
TEST_SCRIPT="$PROJECT_DIR/scripts/test-bolt-deploy-secrets.sh"
DEPLOY_DOC="$PROJECT_DIR/docs/DEPLOY.md"
SECRETS_DOC="$PROJECT_DIR/docs/secrets.md"

PASS=0
FAIL=0

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; ((PASS += 1)); }
fail() { echo -e "${RED}FAIL${NC} $1"; ((FAIL += 1)); }

assert_absent() {
    local file="$1" pattern="$2" label="$3"
    if grep -qF "$pattern" "$file" 2>/dev/null; then
        fail "§ $label: '$pattern' found in $file (must be absent)"
    else
        pass "§ $label: '$pattern' absent from $(basename "$file")"
    fi
}

assert_present() {
    local file="$1" pattern="$2" label="$3"
    if grep -qF "$pattern" "$file" 2>/dev/null; then
        pass "§ $label: '$pattern' present in $(basename "$file")"
    else
        fail "§ $label: '$pattern' NOT FOUND in $file"
    fi
}

# ── §1 generate_bolt_tls_fallback() must not exist ─────────────────────────
echo ""
echo "=== §1: generate_bolt_tls_fallback() must not exist in deploy.yml ==="

assert_absent "$DEPLOY_YML" "generate_bolt_tls_fallback()" "1.1 function declaration absent"
assert_absent "$DEPLOY_YML" "generate_bolt_tls_fallback" "1.2 no call sites"
assert_absent "$DEPLOY_YML" "openssl req -x509 -nodes -newkey" "1.3 no ephemeral cert generation"
assert_absent "$DEPLOY_YML" "self-signed TLS material for pull_request" "1.4 no PR fallback message"

# ── §2 Fail-closed TLS block with exact error strings ──────────────────────
echo ""
echo "=== §2: Fail-closed TLS block with exact error strings ==="

EXPIRED_ERROR="DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying"
MISSING_ERROR="DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying"

assert_present "$DEPLOY_YML" "BOLT_TLS_ENABLED=false" "2.1 BOLT_TLS_ENABLED initialised false"
assert_present "$DEPLOY_YML" "BOLT_TLS_ENABLED=true" "2.2 BOLT_TLS_ENABLED set true on valid cert"
assert_present "$DEPLOY_YML" "::error::${EXPIRED_ERROR}" "2.3 ::error:: annotation for expired cert"
assert_present "$DEPLOY_YML" "::error::${MISSING_ERROR}" "2.4 ::error:: annotation for missing certs"

# Verify exit 1 follows each ::error:: annotation
if python3 - "$DEPLOY_YML" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
expired_err = "::error::DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying"
missing_err = "::error::DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying"

for label, err in (("expired cert", expired_err), ("missing certs", missing_err)):
    idx = text.find(err)
    if idx < 0:
        raise SystemExit(f"ERROR string not found: {label}")
    next_exit = text.find("exit 1", idx)
    if next_exit < 0 or next_exit - idx > 200:
        raise SystemExit(f"exit 1 not found within 200 chars of ::error:: for {label}")
PY
then
    pass "2.5 exit 1 follows each ::error:: annotation"
else
    fail "2.5 exit 1 missing after one or more ::error:: annotations"
fi

assert_absent "$DEPLOY_YML" "::warning::.*TLS" "2.6 no ::warning:: for TLS failures (must be ::error::)"

# ── §3 cert_pem_is_current() must be preserved ─────────────────────────────
echo ""
echo "=== §3: cert_pem_is_current() must be preserved ==="

assert_present "$DEPLOY_YML" "cert_pem_is_current() {" "3.1 function declaration present"
assert_present "$DEPLOY_YML" "openssl x509 -in" "3.2 openssl x509 check inside cert_pem_is_current"

# Verify cert_pem_is_current appears BEFORE BOLT_TLS_ENABLED=true
if python3 - "$DEPLOY_YML" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
idx_fn = text.find("cert_pem_is_current() {")
idx_true = text.find("BOLT_TLS_ENABLED=true")
if idx_fn < 0:
    raise SystemExit("cert_pem_is_current() not found")
if idx_true < 0:
    raise SystemExit("BOLT_TLS_ENABLED=true not found")
if not (idx_fn < idx_true):
    raise SystemExit(f"cert_pem_is_current (pos {idx_fn}) must precede BOLT_TLS_ENABLED=true (pos {idx_true})")
PY
then
    pass "3.3 cert_pem_is_current() precedes BOLT_TLS_ENABLED=true"
else
    fail "3.3 cert_pem_is_current() must appear before BOLT_TLS_ENABLED=true"
fi

# ── §4 No ENV_TYPE-gated TLS behavior ──────────────────────────────────────
echo ""
echo "=== §4: No ENV_TYPE-gated TLS behavior ==="

assert_absent "$DEPLOY_YML" 'elif [[ "${ENV_TYPE}" == "branch" ]]' "4.1 no ENV_TYPE branch gate in TLS block"
assert_absent "$DEPLOY_YML" "branch-environment fallback TLS" "4.2 no self-healing prose"

# Verify TLS block has no ENV_TYPE conditional
if python3 - "$DEPLOY_YML" <<'PY'
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
tls_start = text.index("BOLT_TLS_ENABLED=false")
tls_end = text.index("# Reuse the existing Bolt password", tls_start)
tls_block = text[tls_start:tls_end]
if "ENV_TYPE" in tls_block:
    raise SystemExit(f"ENV_TYPE found inside TLS decision block — must not be gated by environment type")
PY
then
    pass "4.3 TLS decision block contains no ENV_TYPE reference"
else
    fail "4.3 TLS decision block must not reference ENV_TYPE"
fi

# ── §5 forbidden_snippets regression guards in validator ───────────────────
echo ""
echo "=== §5: forbidden_snippets regression guards in validator ==="

assert_present "$TEST_SCRIPT" "openssl req -x509 -nodes -newkey" "5.1 ephemeral cert guard in forbidden_snippets"
assert_present "$TEST_SCRIPT" "generate_bolt_tls_fallback" "5.2 fallback function guard in forbidden_snippets"
assert_present "$TEST_SCRIPT" "branch-environment fallback TLS" "5.3 self-healing prose guard in forbidden_snippets"
assert_present "$TEST_SCRIPT" "self-signed TLS material for pull_request" "5.4 PR fallback guard in forbidden_snippets"

# ── §6 required_snippets sync with deploy.yml error strings ────────────────
echo ""
echo "=== §6: required_snippets sync with deploy.yml error strings ==="

assert_present "$TEST_SCRIPT" "$EXPIRED_ERROR" "6.1 expired cert error string in required_snippets"
assert_present "$TEST_SCRIPT" "$MISSING_ERROR" "6.2 missing certs error string in required_snippets"
assert_absent "$TEST_SCRIPT" "branch tls expiry fallback warning" "6.3 stale fallback warning key removed"
assert_absent "$TEST_SCRIPT" "pr tls fallback generation" "6.4 stale PR fallback key removed"
assert_absent "$TEST_SCRIPT" "branch tls missing fallback warning" "6.5 stale missing warning key removed"

# ── §7 run_tls_case() invocations reflect fail-closed semantics ─────────────
echo ""
echo "=== §7: run_tls_case() invocations reflect fail-closed semantics ==="

assert_present "$TEST_SCRIPT" "branch env missing certs fails closed" "7.1 branch missing certs → fails closed"
assert_present "$TEST_SCRIPT" "branch env expired cert fails closed" "7.2 branch expired cert → fails closed"
assert_present "$TEST_SCRIPT" "valid cert happy path" "7.3 valid cert happy path present"
assert_absent "$TEST_SCRIPT" "generates fallback" "7.4 no fallback-expect invocations remain"
assert_absent "$TEST_SCRIPT" "generate_bolt_tls_fallback() {" "7.5 fallback stub removed from harness"

# ── §8 docs/DEPLOY.md has fail-closed language ─────────────────────────────
echo ""
echo "=== §8: docs/DEPLOY.md has fail-closed language ==="

assert_present "$DEPLOY_DOC" "TLS is required for all environments" "8.1 TLS-required paragraph present"
assert_present "$DEPLOY_DOC" "Provision current, valid TLS secrets before triggering any deploy" "8.2 explicit provisioning instruction"
assert_present "$DEPLOY_DOC" "There is no fallback" "8.3 explicit no-fallback statement"
assert_absent "$DEPLOY_DOC" "self-heals" "8.4 no self-healing language"
assert_absent "$DEPLOY_DOC" "plaintext mode without" "8.5 no plaintext mode paragraph"

# ── §9 docs/secrets.md has provisioning requirement ────────────────────────
echo ""
echo "=== §9: docs/secrets.md has provisioning requirement ==="

assert_present "$SECRETS_DOC" "DEPLOY_BOLT_TLS_CERT_PEM" "9.1 DEPLOY_BOLT_TLS_CERT_PEM documented"
assert_present "$SECRETS_DOC" "DEPLOY_BOLT_TLS_KEY_PEM" "9.2 DEPLOY_BOLT_TLS_KEY_PEM documented"
assert_present "$SECRETS_DOC" "There is no fallback" "9.3 fail-closed statement present"
assert_present "$SECRETS_DOC" "Plaintext mode does not exist" "9.4 no-plaintext statement present"
assert_absent "$SECRETS_DOC" "fall back to \`pr-" "9.5 PR fallback description removed"

# ── §10 Cross-file error string consistency ─────────────────────────────────
echo ""
echo "=== §10: Cross-file error string consistency ==="

if python3 - "$DEPLOY_YML" "$TEST_SCRIPT" <<'PY'
import sys
from pathlib import Path

deploy_text = Path(sys.argv[1]).read_text()
test_text = Path(sys.argv[2]).read_text()

EXPIRED_ERROR = "DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying"
MISSING_ERROR = "DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying"

errors = []
for label, string in (("expired cert", EXPIRED_ERROR), ("missing certs", MISSING_ERROR)):
    if string not in deploy_text:
        errors.append(f"{label}: not in deploy.yml")
    if string not in test_text:
        errors.append(f"{label}: not in test-bolt-deploy-secrets.sh")
if errors:
    raise SystemExit("Cross-file consistency failures: " + "; ".join(errors))
PY
then
    pass "10.1 Error strings identical in deploy.yml and test-bolt-deploy-secrets.sh"
else
    fail "10.1 Error strings diverge between deploy.yml and test-bolt-deploy-secrets.sh"
fi

if grep -qF "$EXPIRED_ERROR" "$DEPLOY_YML" && grep -qF "$EXPIRED_ERROR" "$TEST_SCRIPT"; then
    pass "10.2 Expired cert error string consistent in both files"
else
    fail "10.2 Expired cert error string missing from one or more files"
fi

# ── Summary ──────────────────────────────────────────────────────────────────
echo ""
echo "========================================"
echo "TLS Fail-Closed TDD Suite Results"
echo "========================================"
echo "PASS: ${PASS}  FAIL: ${FAIL}"
echo "========================================"

[[ $FAIL -eq 0 ]]
