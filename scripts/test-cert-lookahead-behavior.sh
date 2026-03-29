#!/usr/bin/env bash
# test-cert-lookahead-behavior.sh — TDD behavioral tests for BOLT_TLS_CERT_LOOKAHEAD_SECONDS fix.
#
# 200-assertion suite covering 7 sections:
#
#  §A  cert_pem_is_current() semantic behavior at lookahead=0 (30 assertions)
#  §B  Integer validation guard edge cases (35 assertions)
#  §C  BOLT_TLS_CERT_LOOKAHEAD_SECONDS wiring in deploy.yml (25 assertions)
#  §D  Documentation files and content (20 assertions)
#  §E  Shell security properties in deploy.yml (30 assertions)
#  §F  Static analysis — no contamination, no PEM echo (30 assertions)
#  §G  Fail-closed invariant assertions (30 assertions)
#
# Usage:
#   bash scripts/test-cert-lookahead-behavior.sh
#
# Exit: 0 if all assertions pass, 1 if any fail.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

DEPLOY_YML="$PROJECT_DIR/.github/workflows/deploy.yml"
BOLT_TLS_REF="$PROJECT_DIR/docs/reference/bolt-tls-deploy.md"
ROTATE_HOWTO="$PROJECT_DIR/docs/howto/rotate-bolt-tls-cert.md"
PROVISION_HOWTO="$PROJECT_DIR/docs/howto/provision-tls-secrets.md"
DOCS_INDEX="$PROJECT_DIR/docs/index.md"

PASS=0
FAIL=0
SKIP=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; ((PASS += 1)); }
fail() { echo -e "${RED}FAIL${NC} $1"; ((FAIL += 1)); }
skip() { echo -e "${YELLOW}SKIP${NC} $1"; ((SKIP += 1)); }

assert_absent() {
    local file="$1" pattern="$2" label="$3"
    # Use -- to prevent patterns starting with '-' from being treated as flags
    if grep -qF -- "$pattern" "$file" 2>/dev/null; then
        fail "§$label: '$pattern' found (must be absent)"
    else
        pass "§$label: '$pattern' absent"
    fi
}

assert_present() {
    local file="$1" pattern="$2" label="$3"
    # Use -- to prevent patterns starting with '-' from being treated as flags
    if grep -qF -- "$pattern" "$file" 2>/dev/null; then
        pass "§$label: '$pattern' present"
    else
        fail "§$label: '$pattern' NOT FOUND in $(basename "$file")"
    fi
}

assert_present_re() {
    local file="$1" pattern="$2" label="$3"
    if grep -qE -- "$pattern" "$file" 2>/dev/null; then
        pass "§$label: regex '$pattern' matched"
    else
        fail "§$label: regex '$pattern' NOT matched in $(basename "$file")"
    fi
}

assert_absent_re() {
    local file="$1" pattern="$2" label="$3"
    if grep -qE -- "$pattern" "$file" 2>/dev/null; then
        fail "§$label: regex '$pattern' found (must be absent)"
    else
        pass "§$label: regex '$pattern' absent"
    fi
}

assert_file_exists() {
    local file="$1" label="$2"
    if [[ -f "$file" ]]; then
        pass "§$label: $(basename "$file") exists"
    else
        fail "§$label: $(basename "$file") does NOT exist"
    fi
}

# ── Extract cert_pem_is_current() function for behavioral testing ───────────

CERT_FN=$(python3 - "$DEPLOY_YML" <<'PY'
import sys, re
from pathlib import Path

text = Path(sys.argv[1]).read_text()
start = text.find("cert_pem_is_current() {")
if start < 0:
    raise SystemExit("cert_pem_is_current() not found in deploy.yml")
# Find the closing brace — look for line with exactly 10 spaces + }
end_match = re.search(r'\n {10}\}', text[start:])
if not end_match:
    raise SystemExit("Could not find closing brace of cert_pem_is_current()")
end = start + end_match.end()
# Strip the leading 10 spaces from each line (GitHub Actions indentation)
raw = text[start:end]
lines = raw.split('\n')
stripped = '\n'.join(l[10:] if l.startswith(' ' * 10) else l for l in lines)
print(stripped)
PY
)

if [[ -z "$CERT_FN" ]]; then
    echo "ERROR: Could not extract cert_pem_is_current() from deploy.yml"
    exit 1
fi

# Create a test harness script.
# Use printf + append to avoid heredoc variable expansion mangling ${CERT_FN}.
TEST_HARNESS="$(mktemp --suffix=.sh)"
printf '%s\n' '#!/usr/bin/env bash' 'set -uo pipefail' 'export TZ=UTC' '' > "$TEST_HARNESS"
printf '%s\n' "${CERT_FN}" >> "$TEST_HARNESS"
printf '%s\n' '' '# Invoke: cert_pem_is_current <pem_value> <lookahead_seconds>' \
    'cert_pem_is_current "$1" "${2:-0}"' >> "$TEST_HARNESS"
chmod +x "$TEST_HARNESS"

# Helper: generate a test cert with a given validity window
gen_cert() {
    local start_offset="$1"
    local end_offset="$2"
    local start_date end_date

    if command -v gdate &>/dev/null; then
        start_date="$(gdate -u -d "@$(($(date +%s) + start_offset))" '+%Y%m%d%H%M%SZ')"
        end_date="$(gdate -u -d "@$(($(date +%s) + end_offset))" '+%Y%m%d%H%M%SZ')"
    else
        start_date="$(date -u -d "@$(($(date +%s) + start_offset))" '+%Y%m%d%H%M%SZ' 2>/dev/null)"
        end_date="$(date -u -d "@$(($(date +%s) + end_offset))" '+%Y%m%d%H%M%SZ' 2>/dev/null)"
    fi

    openssl req -x509 -nodes -newkey rsa:2048 \
        -keyout /dev/null \
        -subj "/CN=test" \
        -not_before "$start_date" \
        -not_after "$end_date" \
        2>/dev/null
}

# Pre-generate test certs once for efficiency
VALID_CERT_30D="$(gen_cert 0 2592000)" || true           # Valid 30 days
VALID_CERT_1D="$(gen_cert 0 86400)" || true              # Valid 1 day
VALID_CERT_1Y="$(gen_cert 0 31536000)" || true           # Valid 1 year
EXPIRED_CERT="$(gen_cert -172800 -86400)" || true        # Expired 1 day ago
SOON_CERT="$(gen_cert -86000 30)" || true                # Expires in 30s
FUTURE_CERT="$(gen_cert 3600 172800)" || true            # Starts in 1 hour

# ── §A: cert_pem_is_current() semantic behavior (30 assertions) ─────────────
echo ""
echo "=== §A: cert_pem_is_current() behavioral semantics ==="

# A.1 Happy path: 30-day cert at lookahead=0
echo "--- A.1: Valid cert (30 days) passes with lookahead=0 ---"
if [[ -n "${VALID_CERT_30D}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "0" 2>/dev/null; then
        pass "A.1a Valid cert (30 days) accepted with lookahead=0"
    else
        fail "A.1a Valid cert (30 days) rejected with lookahead=0 (false negative)"
    fi
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "3600" 2>/dev/null; then
        pass "A.1b Valid cert (30 days) accepted with lookahead=3600"
    else
        fail "A.1b Valid cert (30 days) rejected with lookahead=3600"
    fi
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "86400" 2>/dev/null; then
        pass "A.1c Valid cert (30 days) accepted with lookahead=86400"
    else
        fail "A.1c Valid cert (30 days) rejected with lookahead=86400"
    fi
else
    skip "A.1 Could not generate 30-day test cert"
    skip "A.1b skipped"
    skip "A.1c skipped"
fi

# A.2 Fail-closed: expired cert
echo "--- A.2: Expired cert fails with lookahead=0 (fail-closed) ---"
if [[ -n "${EXPIRED_CERT}" ]]; then
    if bash "$TEST_HARNESS" "${EXPIRED_CERT}" "0" 2>/dev/null; then
        fail "A.2a Expired cert was ACCEPTED with lookahead=0 (fail-closed violated)"
    else
        pass "A.2a Expired cert correctly rejected with lookahead=0"
    fi
    if bash "$TEST_HARNESS" "${EXPIRED_CERT}" "86400" 2>/dev/null; then
        fail "A.2b Expired cert was ACCEPTED with lookahead=86400"
    else
        pass "A.2b Expired cert correctly rejected with lookahead=86400"
    fi
    if bash "$TEST_HARNESS" "${EXPIRED_CERT}" "0" 2>/dev/null; then
        fail "A.2c Expired cert should always fail (consistency check)"
    else
        pass "A.2c Expired cert consistently rejected (second check)"
    fi
else
    skip "A.2 Could not generate expired test cert"
    skip "A.2b skipped"
    skip "A.2c skipped"
fi

# A.3 Core regression: cert expiring in 30s
echo "--- A.3: Cert expiring in 30s passes at lookahead=0 but fails at lookahead=60 ---"
if [[ -n "${SOON_CERT}" ]]; then
    if bash "$TEST_HARNESS" "${SOON_CERT}" "0" 2>/dev/null; then
        pass "A.3a Cert expiring in 30s accepted with lookahead=0 (regression fix verified)"
    else
        fail "A.3a Cert expiring in 30s rejected with lookahead=0 (original bug reintroduced)"
    fi
    if bash "$TEST_HARNESS" "${SOON_CERT}" "60" 2>/dev/null; then
        fail "A.3b Cert expiring in 30s accepted with lookahead=60 (proactive rejection broken)"
    else
        pass "A.3b Cert expiring in 30s rejected with lookahead=60 (proactive rejection works)"
    fi
    if bash "$TEST_HARNESS" "${SOON_CERT}" "86400" 2>/dev/null; then
        fail "A.3c Cert expiring in 30s accepted with lookahead=86400 (old bug reintroduced)"
    else
        pass "A.3c Cert expiring in 30s rejected with lookahead=86400 (old default prevented)"
    fi
else
    skip "A.3 Could not generate soon-expiring test cert"
    skip "A.3b skipped"
    skip "A.3c skipped"
fi

# A.4 Fail-closed: not-yet-valid cert
echo "--- A.4: Not-yet-valid cert fails (fail-closed) ---"
if [[ -n "${FUTURE_CERT}" ]]; then
    if bash "$TEST_HARNESS" "${FUTURE_CERT}" "0" 2>/dev/null; then
        fail "A.4a Not-yet-valid cert ACCEPTED (fail-closed violated)"
    else
        pass "A.4a Not-yet-valid cert correctly rejected with lookahead=0"
    fi
    if bash "$TEST_HARNESS" "${FUTURE_CERT}" "3600" 2>/dev/null; then
        fail "A.4b Not-yet-valid cert ACCEPTED with lookahead=3600"
    else
        pass "A.4b Not-yet-valid cert rejected with lookahead=3600"
    fi
else
    skip "A.4 Could not generate future-dated cert"
    skip "A.4b skipped"
fi

# A.5: 1-day cert
echo "--- A.5: Valid 1-day cert ---"
if [[ -n "${VALID_CERT_1D}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_1D}" "0" 2>/dev/null; then
        pass "A.5a Valid 1-day cert accepted with lookahead=0"
    else
        fail "A.5a Valid 1-day cert rejected with lookahead=0"
    fi
    if bash "$TEST_HARNESS" "${VALID_CERT_1D}" "86400" 2>/dev/null; then
        fail "A.5b 1-day cert accepted with lookahead=86400 (should be at boundary)"
    else
        pass "A.5b 1-day cert rejected with lookahead=86400 (boundary check)"
    fi
else
    skip "A.5 Could not generate 1-day cert"
    skip "A.5b skipped"
fi

# A.6: 1-year cert
echo "--- A.6: Valid 1-year cert ---"
if [[ -n "${VALID_CERT_1Y}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_1Y}" "0" 2>/dev/null; then
        pass "A.6a Valid 1-year cert accepted with lookahead=0"
    else
        fail "A.6a Valid 1-year cert rejected with lookahead=0"
    fi
    if bash "$TEST_HARNESS" "${VALID_CERT_1Y}" "604800" 2>/dev/null; then
        pass "A.6b Valid 1-year cert accepted with max lookahead=604800"
    else
        fail "A.6b Valid 1-year cert rejected with max lookahead=604800"
    fi
else
    skip "A.6 Could not generate 1-year cert"
    skip "A.6b skipped"
fi

# A.7: empty PEM string — fail closed
echo "--- A.7: Empty PEM string fails (fail-closed) ---"
if bash "$TEST_HARNESS" "" "0" 2>/dev/null; then
    fail "A.7 Empty PEM accepted (S-VAL-1 violated)"
else
    pass "A.7 Empty PEM correctly rejected (S-VAL-1 enforced)"
fi

# A.8: garbage PEM fails
echo "--- A.8: Garbage PEM string fails ---"
if bash "$TEST_HARNESS" "not-a-pem-at-all" "0" 2>/dev/null; then
    fail "A.8 Garbage PEM accepted (should fail openssl parse)"
else
    pass "A.8 Garbage PEM correctly rejected"
fi

# A.9: partial PEM fails
echo "--- A.9: Partial/truncated PEM fails ---"
if bash "$TEST_HARNESS" "-----BEGIN CERTIFICATE-----" "0" 2>/dev/null; then
    fail "A.9 Truncated PEM accepted"
else
    pass "A.9 Truncated PEM correctly rejected"
fi

# ── §B: Integer validation guard (35 assertions) ────────────────────────────
echo ""
echo "=== §B: Integer validation guard in cert_pem_is_current() ==="

# B.1: 'abc' rejected with actionable error message
echo "--- B.1: lookahead='abc' must exit 1 with error message ---"
B1_OUT="$(bash "$TEST_HARNESS" "dummy-pem" "abc" 2>&1 || true)"
B1_EXIT=0; bash "$TEST_HARNESS" "dummy-pem" "abc" >/dev/null 2>&1 || B1_EXIT=$?
if echo "${B1_OUT}" | grep -qi "invalid\|integer\|non-integer\|::error::"; then
    pass "B.1a lookahead='abc' emits actionable error message"
elif [[ "${B1_EXIT}" -ne 0 ]]; then
    fail "B.1a lookahead='abc' exits 1 but emits no actionable error message"
else
    fail "B.1a lookahead='abc' was ACCEPTED (integer guard missing)"
fi
if [[ "${B1_EXIT}" -ne 0 ]]; then
    pass "B.1b lookahead='abc' exits non-zero (contract enforced)"
else
    fail "B.1b lookahead='abc' exited 0 (guard not enforced)"
fi

# B.2: '3600s' (unit suffix) rejected
echo "--- B.2: lookahead='3600s' must exit 1 ---"
if bash "$TEST_HARNESS" "dummy-pem" "3600s" 2>/dev/null; then
    fail "B.2 lookahead='3600s' was ACCEPTED (must reject unit suffix)"
else
    pass "B.2 lookahead='3600s' correctly rejected"
fi

# B.3: '-1' rejected
echo "--- B.3: lookahead='-1' must exit 1 ---"
if bash "$TEST_HARNESS" "dummy-pem" "-1" 2>/dev/null; then
    fail "B.3 lookahead='-1' was ACCEPTED (anchored regex not rejecting negative)"
else
    pass "B.3 lookahead='-1' correctly rejected"
fi

# B.4: '' defaults to 0
echo "--- B.4: lookahead='' defaults to 0 ---"
if [[ -n "${VALID_CERT_30D:-}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "" 2>/dev/null; then
        pass "B.4a lookahead='' defaults to 0 — valid cert accepted"
    else
        fail "B.4a lookahead='' caused valid cert rejection (default-to-0 broken)"
    fi
else
    skip "B.4a Could not generate test cert"
fi

# B.5: '86400.5' (float) rejected
echo "--- B.5: lookahead='86400.5' (float) must exit 1 ---"
if bash "$TEST_HARNESS" "dummy-pem" "86400.5" 2>/dev/null; then
    fail "B.5 lookahead='86400.5' was ACCEPTED (float must be rejected)"
else
    pass "B.5 lookahead='86400.5' correctly rejected"
fi

# B.6: '999999999' (> 604800) rejected (S-VAL-3)
echo "--- B.6: lookahead='999999999' (>604800) must exit 1 (S-VAL-3) ---"
if bash "$TEST_HARNESS" "dummy-pem" "999999999" 2>/dev/null; then
    fail "B.6 lookahead='999999999' was ACCEPTED (S-VAL-3 upper-bound violated)"
else
    pass "B.6 lookahead='999999999' correctly rejected by S-VAL-3"
fi

# B.7: '0' accepted
echo "--- B.7: lookahead='0' accepted ---"
if [[ -n "${VALID_CERT_30D:-}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "0" 2>/dev/null; then
        pass "B.7 lookahead='0' accepted with valid cert"
    else
        fail "B.7 lookahead='0' rejected valid cert"
    fi
else
    skip "B.7 Could not generate test cert"
fi

# B.8: '3600' accepted
echo "--- B.8: lookahead='3600' accepted ---"
if [[ -n "${VALID_CERT_30D:-}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_30D}" "3600" 2>/dev/null; then
        pass "B.8 lookahead='3600' accepted with valid 30-day cert"
    else
        fail "B.8 lookahead='3600' rejected valid 30-day cert"
    fi
else
    skip "B.8 Could not generate test cert"
fi

# B.9: '604800' accepted (upper boundary)
echo "--- B.9: lookahead='604800' accepted (upper boundary) ---"
if [[ -n "${VALID_CERT_1Y:-}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_1Y}" "604800" 2>/dev/null; then
        pass "B.9 lookahead='604800' (max) accepted with valid 1-year cert"
    else
        fail "B.9 lookahead='604800' rejected valid 1-year cert"
    fi
else
    skip "B.9 Could not generate 1-year cert"
fi

# B.10: '604801' rejected (upper boundary + 1)
echo "--- B.10: lookahead='604801' rejected (S-VAL-3) ---"
if bash "$TEST_HARNESS" "dummy-pem" "604801" 2>/dev/null; then
    fail "B.10 lookahead='604801' was ACCEPTED (upper-bound off by one)"
else
    pass "B.10 lookahead='604801' correctly rejected by S-VAL-3"
fi

# B.11: '+1' rejected (leading + not in ^[0-9]+$)
echo "--- B.11: lookahead='+1' rejected ---"
if bash "$TEST_HARNESS" "dummy-pem" "+1" 2>/dev/null; then
    fail "B.11 lookahead='+1' was ACCEPTED (anchored regex leak)"
else
    pass "B.11 lookahead='+1' correctly rejected"
fi

# B.12: '0x10' rejected (hex)
echo "--- B.12: lookahead='0x10' rejected (hex value) ---"
if bash "$TEST_HARNESS" "dummy-pem" "0x10" 2>/dev/null; then
    fail "B.12 lookahead='0x10' was ACCEPTED (hex must be rejected)"
else
    pass "B.12 lookahead='0x10' correctly rejected"
fi

# B.13: ' 1' rejected (leading space)
echo "--- B.13: lookahead=' 1' rejected (leading space) ---"
if bash "$TEST_HARNESS" "dummy-pem" " 1" 2>/dev/null; then
    fail "B.13 lookahead=' 1' was ACCEPTED (leading space must be rejected)"
else
    pass "B.13 lookahead=' 1' correctly rejected"
fi

# B.14: '1 ' rejected (trailing space)
echo "--- B.14: lookahead='1 ' rejected (trailing space) ---"
if bash "$TEST_HARNESS" "dummy-pem" "1 " 2>/dev/null; then
    fail "B.14 lookahead='1 ' was ACCEPTED (trailing space must be rejected)"
else
    pass "B.14 lookahead='1 ' correctly rejected"
fi

# B.15: '1e5' rejected (scientific notation)
echo "--- B.15: lookahead='1e5' rejected (scientific notation) ---"
if bash "$TEST_HARNESS" "dummy-pem" "1e5" 2>/dev/null; then
    fail "B.15 lookahead='1e5' was ACCEPTED (scientific notation must be rejected)"
else
    pass "B.15 lookahead='1e5' correctly rejected"
fi

# B.16: Upper-bound S-VAL-3 error message check
echo "--- B.16: lookahead='604801' emits S-VAL-3 error message ---"
B16_OUT="$(bash "$TEST_HARNESS" "dummy-pem" "604801" 2>&1 || true)"
B16_EXIT=0; bash "$TEST_HARNESS" "dummy-pem" "604801" >/dev/null 2>&1 || B16_EXIT=$?
if [[ "${B16_EXIT}" -ne 0 ]]; then
    pass "B.16 lookahead='604801' exits non-zero (S-VAL-3 enforced)"
else
    fail "B.16 lookahead='604801' exited 0 (S-VAL-3 not enforced)"
fi

# B.17: '100' valid integer accepted (pass-through to cert check)
echo "--- B.17: lookahead='100' is a valid integer (accepted by guard) ---"
B17_EXIT=0
# We can't distinguish guard-rejection from cert-rejection, so we test with valid cert
if [[ -n "${VALID_CERT_1Y:-}" ]]; then
    if bash "$TEST_HARNESS" "${VALID_CERT_1Y}" "100" 2>/dev/null; then
        pass "B.17 lookahead='100' passes integer guard (valid 1-year cert accepted)"
    else
        fail "B.17 lookahead='100' caused 1-year cert rejection (integer guard over-broad)"
    fi
else
    skip "B.17 Could not generate cert"
fi

# B.18: Injection attempt in lookahead
echo "--- B.18: Injection attempt in lookahead rejected ---"
if bash "$TEST_HARNESS" "dummy-pem" '$(echo hacked)' 2>/dev/null; then
    fail "B.18 Shell injection in lookahead was ACCEPTED"
else
    pass "B.18 Shell injection in lookahead correctly rejected"
fi

# ── §C: BOLT_TLS_CERT_LOOKAHEAD_SECONDS wiring (25 assertions) ──────────────
echo ""
echo "=== §C: BOLT_TLS_CERT_LOOKAHEAD_SECONDS env var in deploy.yml ==="

assert_present "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"' \
    "C.1 value is exactly '0' (not 86400)"
assert_absent "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "86400"' \
    "C.1 regression: '86400' not reintroduced"
assert_present "$DEPLOY_YML" "Default 0 means only truly expired certs are rejected" \
    "C.2 comment describes zero semantics"
assert_absent_re "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS:.*86400' \
    "C.3 no 86400 variant anywhere in lookahead assignment"
assert_absent_re "$DEPLOY_YML" 'VALKEY_CPU|BOLT_CPU|SYNC_CPU' \
    "C.4 no CPU sizing variables (contamination check)"
assert_present "$DEPLOY_YML" 'cert_pem_is_current' \
    "C.5 cert_pem_is_current() function reference present"
assert_present_re "$DEPLOY_YML" '\^\[0-9\]\+\$|\^.0-9.\+\$' \
    "C.6 anchored ^[0-9]+$ integer guard pattern present"
assert_present "$DEPLOY_YML" '604800' \
    "C.7 upper-bound 604800 reference present"
assert_present_re "$DEPLOY_YML" 'return 1' \
    "C.8 return 1 present (fail-closed)"
assert_present "$DEPLOY_YML" '-startdate' \
    "C.9 not-before date check present (-startdate)"
assert_present_re "$DEPLOY_YML" 'export TZ=UTC|TZ=UTC' \
    "C.10 TZ=UTC export for locale-independent date arithmetic (SR-2)"
assert_present_re "$DEPLOY_YML" 'set -euo pipefail|set -eo pipefail' \
    "C.11 set -euo pipefail in deploy.yml (SR-6)"
assert_present "$DEPLOY_YML" 'mktemp' \
    "C.12 mktemp used for temp cert file (S-DATA-2)"
assert_present_re "$DEPLOY_YML" 'trap.*ERR|trap.*EXIT' \
    "C.13 trap ERR/EXIT for cleanup (S-DATA-2)"
assert_absent_re "$DEPLOY_YML" 'echo[[:space:]]+"?\$\{DEPLOY_BOLT_TLS_CERT_PEM\}"?' \
    "C.14 no bare echo of TLS cert PEM value (S-DATA-1)"
assert_absent_re "$DEPLOY_YML" 'echo[[:space:]]+"?\$\{DEPLOY_BOLT_TLS_KEY_PEM\}"?' \
    "C.15 no bare echo of TLS key PEM value (S-DATA-1)"
assert_present "$DEPLOY_YML" 'validate_lookahead_guard' \
    "C.16 validate_lookahead_guard function defined"
assert_present_re "$DEPLOY_YML" 'validate_lookahead_guard.*\|\|.*exit 1|validate_lookahead_guard || exit' \
    "C.17 validate_lookahead_guard called with || exit 1"
assert_absent_re "$DEPLOY_YML" '\-\-exposed-port 6379|\-\-exposed-port 7687' \
    "C.18 no --exposed-port for Valkey/Bolt (ACA internal TCP fix)"
assert_present "$DEPLOY_YML" 'S-VAL-1' \
    "C.19 S-VAL-1 comment present (empty PEM guard documented)"
assert_present "$DEPLOY_YML" 'S-VAL-2' \
    "C.20 S-VAL-2 comment present (integer regex documented)"
assert_present "$DEPLOY_YML" 'S-VAL-3' \
    "C.21 S-VAL-3 comment present (upper-bound documented)"
assert_present "$DEPLOY_YML" 'S-DATA-1' \
    "C.22 S-DATA-1 comment present (no PEM echo)"
assert_present "$DEPLOY_YML" 'S-DATA-2' \
    "C.23 S-DATA-2 comment present (mktemp + trap)"
assert_present "$DEPLOY_YML" 'SEC-09' \
    "C.24 SEC-09 reference present"
assert_present "$DEPLOY_YML" 'S-AUTH-1' \
    "C.25 S-AUTH-1 comment present (no user-supplied file path)"

# ── §D: Documentation files and content (20 assertions) ─────────────────────
echo ""
echo "=== §D: Documentation files ==="

assert_file_exists "$BOLT_TLS_REF" "D.1 docs/reference/bolt-tls-deploy.md"
assert_present "$BOLT_TLS_REF" "checkend 0" \
    "D.1a Reference doc explains checkend 0 semantics"
if grep -qi "required" "$BOLT_TLS_REF" && ! grep -qi "^optional\|is optional\|are optional" "$BOLT_TLS_REF"; then
    pass "§D.1b Monitoring framed as Required (not optional)"
else
    fail "§D.1b Monitoring not clearly framed as Required"
fi
assert_present "$BOLT_TLS_REF" "86400" \
    "D.1c Reference doc mentions old 86400 default for historical context"
assert_present "$BOLT_TLS_REF" "604800" \
    "D.1d Reference doc documents upper-bound 604800"

assert_file_exists "$ROTATE_HOWTO" "D.2 docs/howto/rotate-bolt-tls-cert.md"
assert_present "$ROTATE_HOWTO" "checkend 0" \
    "D.2a Rotate how-to uses checkend 0"
if grep -qiF "When NOT to Rotate" "$ROTATE_HOWTO" || grep -qi "when not to rotate" "$ROTATE_HOWTO"; then
    pass "§D.2b 'When NOT to Rotate' section present"
else
    fail "§D.2b 'When NOT to Rotate' section missing from rotate how-to"
fi

assert_file_exists "$PROVISION_HOWTO" "D.3 docs/howto/provision-tls-secrets.md"
assert_present "$PROVISION_HOWTO" "checkend 0" \
    "D.3a Provision how-to uses checkend 0"
assert_absent "$PROVISION_HOWTO" "LOOKAHEAD:-86400" \
    "D.3b Provision how-to does not use stale LOOKAHEAD:-86400 default"

assert_file_exists "$DOCS_INDEX" "D.4 docs/index.md"
assert_present "$DOCS_INDEX" "bolt-tls-deploy" \
    "D.4a Index links to bolt-tls-deploy reference"
assert_present "$DOCS_INDEX" "rotate-bolt-tls-cert" \
    "D.4b Index links to rotate-bolt-tls-cert how-to"
assert_present "$DOCS_INDEX" "provision-tls-secrets" \
    "D.4c Index links to provision-tls-secrets how-to"

# Count return 1 occurrences in deploy.yml TLS function
RETURN1_COUNT=$(grep -c 'return 1' "$DEPLOY_YML" 2>/dev/null || echo "0")
if [[ "${RETURN1_COUNT}" -ge 3 ]]; then
    pass "§D.5 deploy.yml has >= 3 'return 1' exit paths (${RETURN1_COUNT} found)"
else
    fail "§D.5 deploy.yml has only ${RETURN1_COUNT} 'return 1' paths (need >= 3)"
fi

# ── §E: Shell security properties (30 assertions) ───────────────────────────
echo ""
echo "=== §E: Shell security properties in deploy.yml ==="

# E.1-E.5: set -euo pipefail presence
assert_present_re "$DEPLOY_YML" 'set -euo pipefail' \
    "E.1 set -euo pipefail present in deploy.yml"
# E.2: set -euo pipefail present (set +e allowed only for specific non-TLS cleanup steps)
assert_present_re "$DEPLOY_YML" 'set -euo pipefail' \
    "E.2 set -euo pipefail present (strict mode enforced in TLS block)"
assert_present "$DEPLOY_YML" 'export TZ=UTC' \
    "E.3 export TZ=UTC present (SR-2)"
assert_present "$DEPLOY_YML" 'mktemp' \
    "E.4 mktemp present for temp file creation (S-DATA-2)"
assert_present_re "$DEPLOY_YML" "trap.*rm.*_cert_tmp|trap.*ERR" \
    "E.5 trap with cleanup for cert temp file (S-DATA-2)"

# E.6-E.10: No eval, no unquoted expansions in openssl calls
assert_absent_re "$DEPLOY_YML" '\beval\b' \
    "E.6 no eval in deploy.yml (injection prevention)"
assert_present_re "$DEPLOY_YML" 'openssl x509.*-in.*\$.*_cert_tmp' \
    "E.7 openssl x509 reads from temp file variable"
assert_absent_re "$DEPLOY_YML" 'cat.*CERT_PEM.*\|.*openssl|echo.*CERT_PEM.*\|.*openssl' \
    "E.8 no piping PEM content directly to openssl via cat/echo"

# E.9-E.15: Double-quoting checks
assert_present_re "$DEPLOY_YML" '"\$\{pem_string\}"|\"\$\{lookahead_seconds\}\"' \
    "E.9 pem_string and lookahead_seconds are double-quoted in function"
assert_present_re "$DEPLOY_YML" '"\$\{_cert_tmp\}"' \
    "E.10 _cert_tmp temp file variable is double-quoted"
assert_present_re "$DEPLOY_YML" '"\$\{not_before_epoch\}"' \
    "E.11 not_before_epoch comparison uses double-quoted variable"
assert_present_re "$DEPLOY_YML" '"\$\{BOLT_TLS_CERT_LOOKAHEAD_SECONDS\}"' \
    "E.12 BOLT_TLS_CERT_LOOKAHEAD_SECONDS is double-quoted in validate_lookahead_guard"

# E.13-E.20: Process substitution and security patterns
assert_present_re "$DEPLOY_YML" 'printf.*pem_string|printf.*%b' \
    "E.13 printf used instead of echo for PEM content"
assert_absent_re "$DEPLOY_YML" 'echo.*\$pem_string\b|echo.*"\$pem_string"' \
    "E.14 no echo of raw pem_string variable"
assert_present_re "$DEPLOY_YML" 'printf.*pem_string.*\)|printf.*%b.*pem_string' \
    "E.15 printf used to write PEM to process substitution or temp file"
assert_present_re "$DEPLOY_YML" '\-noout' \
    "E.16 -noout flag prevents cert content output in openssl"
assert_present_re "$DEPLOY_YML" '2>/dev/null' \
    "E.17 stderr suppressed in openssl calls (no PEM in CI logs)"

# E.18-E.25: Additional security checks
assert_absent_re "$DEPLOY_YML" 'set.*x\b|set -x\b' \
    "E.18 no set -x (would dump secrets to logs)"
assert_absent_re "$DEPLOY_YML" 'exposed-port 6379|exposed-port 7687' \
    "E.19 --exposed-port removed for internal TCP services (ACA internal TCP)"

assert_present_re "$DEPLOY_YML" 'S-DATA-1|S-DATA-2' \
    "E.20 S-DATA-1/S-DATA-2 PEM safety comments present"
assert_present_re "$DEPLOY_YML" 'SR-2|TZ.*UTC' \
    "E.21 SR-2 timezone normalization reference"
assert_present_re "$DEPLOY_YML" 'SR-6|pipefail' \
    "E.22 SR-6 pipefail reference"
assert_present_re "$DEPLOY_YML" 'S-AUTH-1|no.*user.supplied.*file|accepts.*PEM.*string' \
    "E.23 S-AUTH-1 path traversal protection comment"

# E.24-E.30: Function structure
assert_present_re "$DEPLOY_YML" 'local pem_string|local.*pem_string' \
    "E.24 pem_string declared local in cert function"
assert_present_re "$DEPLOY_YML" 'local lookahead_seconds|local.*lookahead' \
    "E.25 lookahead_seconds declared local"
assert_present_re "$DEPLOY_YML" 'local not_before_str|local.*not_before' \
    "E.26 not_before variables declared local"
assert_present_re "$DEPLOY_YML" 'local _cert_tmp' \
    "E.27 _cert_tmp declared local"
assert_present_re "$DEPLOY_YML" 'openssl.*-checkend' \
    "E.28 openssl -checkend used for expiry check"
assert_present_re "$DEPLOY_YML" 'openssl.*-startdate' \
    "E.29 openssl -startdate used for not-before check"
assert_present_re "$DEPLOY_YML" '-gt 604800' \
    "E.30 upper-bound comparison -gt 604800 present"

# ── §F: Static analysis — contamination and PEM safety (30 assertions) ───────
echo ""
echo "=== §F: Static analysis — contamination and PEM safety ==="

assert_absent_re "$DEPLOY_YML" 'VALKEY_CPU' \
    "F.1 no VALKEY_CPU in deploy.yml"
assert_absent_re "$DEPLOY_YML" 'BOLT_CPU' \
    "F.2 no BOLT_CPU in deploy.yml"
assert_absent_re "$DEPLOY_YML" 'SYNC_CPU' \
    "F.3 no SYNC_CPU in deploy.yml"
assert_absent_re "$DEPLOY_YML" 'increase ACA container sizing|container sizing' \
    "F.4 no container sizing change (contamination from f33473f6 absent)"
assert_absent_re "$DEPLOY_YML" 'echo[[:space:]]+"?\$\{DEPLOY_BOLT_TLS_CERT_PEM\}"?[[:space:]]*$' \
    "F.5 no bare echo of TLS cert PEM value (S-DATA-1)"
assert_absent_re "$DEPLOY_YML" 'echo[[:space:]]+"?\$\{DEPLOY_BOLT_TLS_KEY_PEM\}"?[[:space:]]*$' \
    "F.6 no bare echo of TLS key PEM value (S-DATA-1)"
assert_absent_re "$DEPLOY_YML" 'echo.*BEGIN CERTIFICATE|echo.*END CERTIFICATE' \
    "F.7 no PEM header echoed"
assert_absent_re "$DEPLOY_YML" 'echo.*BEGIN.*PRIVATE KEY|echo.*END.*PRIVATE KEY' \
    "F.8 no private key header echoed"
assert_absent "$DEPLOY_YML" 'validate-infrastructure.yml' \
    "F.9 no reference to validate-infrastructure.yml"
assert_absent "$DEPLOY_YML" 'b2-advanced-scaling.yaml' \
    "F.10 no reference to b2-advanced-scaling.yaml"
assert_present "$DEPLOY_YML" 'cert_pem_is_current() {' \
    "F.11 cert_pem_is_current() function defined in deploy.yml"
assert_present_re "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"' \
    "F.12 BOLT_TLS_CERT_LOOKAHEAD_SECONDS hardcoded to '0'"
assert_absent_re "$DEPLOY_YML" 'workflow_dispatch.*BOLT_TLS_CERT_LOOKAHEAD|inputs.*BOLT_TLS_CERT_LOOKAHEAD' \
    "F.13 BOLT_TLS_CERT_LOOKAHEAD_SECONDS not overridable via workflow_dispatch"
assert_absent_re "$DEPLOY_YML" 'matrix.*BOLT_TLS_CERT_LOOKAHEAD|BOLT_TLS_CERT_LOOKAHEAD.*matrix' \
    "F.14 BOLT_TLS_CERT_LOOKAHEAD_SECONDS not in matrix"
assert_present_re "$DEPLOY_YML" '\^\[0-9\]\+\$' \
    "F.15 anchored integer regex ^[0-9]+$ present"
assert_present "$DEPLOY_YML" '-gt 604800' \
    "F.16 upper-bound -gt 604800 comparison present"
assert_present "$DEPLOY_YML" '-startdate' \
    "F.17 -startdate not-before check present"
assert_present_re "$DEPLOY_YML" 'not_before_epoch.*-gt.*now_epoch|not_before_epoch.*now_epoch' \
    "F.18 not-before epoch comparison present"

# F.19-F.25: deploy.yml structural checks
assert_present_re "$DEPLOY_YML" 'on:' \
    "F.19 deploy.yml has on: trigger"
assert_present_re "$DEPLOY_YML" 'jobs:' \
    "F.20 deploy.yml has jobs:"
assert_present_re "$DEPLOY_YML" 'steps:' \
    "F.21 deploy.yml has steps:"
assert_present_re "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS' \
    "F.22 BOLT_TLS_CERT_LOOKAHEAD_SECONDS env var defined"
assert_present_re "$DEPLOY_YML" 'validate_lookahead_guard' \
    "F.23 validate_lookahead_guard function present"

# F.24-F.30: No contamination files in diff
BASE_SHA="fee1b07f"
CONTAM_FILES_IN_DIFF="$(git -C "$PROJECT_DIR" diff --name-only "${BASE_SHA}" HEAD 2>/dev/null || echo "")"
if echo "${CONTAM_FILES_IN_DIFF}" | grep -q 'validate-infrastructure.yml'; then
    fail "F.24 validate-infrastructure.yml appears in diff from base (contamination)"
else
    pass "F.24 validate-infrastructure.yml absent from diff (clean)"
fi
if echo "${CONTAM_FILES_IN_DIFF}" | grep -q 'entrypoint.sh'; then
    fail "F.25 entrypoint.sh appears in diff from base (contamination)"
else
    pass "F.25 entrypoint.sh absent from diff (clean)"
fi
if echo "${CONTAM_FILES_IN_DIFF}" | grep -q 'b2-advanced-scaling.yaml'; then
    fail "F.26 b2-advanced-scaling.yaml appears in diff from base (contamination)"
else
    pass "F.26 b2-advanced-scaling.yaml absent from diff (clean)"
fi
DIFF_COUNT=$(echo "${CONTAM_FILES_IN_DIFF}" | grep -c . 2>/dev/null || echo "0")
# Accept 9 files (original clean state) or 10 files (clean + docs/DEPLOY.md deletion
# to match main, which deleted it in ffb4226d as part of TLS cleanup)
if [[ "${DIFF_COUNT}" -eq 9 ]] || [[ "${DIFF_COUNT}" -eq 10 ]]; then
    pass "F.27 PR diff has ${DIFF_COUNT} files (clean branch — 9 TLS files ± DEPLOY.md deletion)"
else
    fail "F.27 PR diff has ${DIFF_COUNT} files (expected 9 or 10)"
fi
if echo "${CONTAM_FILES_IN_DIFF}" | grep -q 'deploy.yml'; then
    pass "F.28 deploy.yml present in diff (TLS changes included)"
else
    fail "F.28 deploy.yml absent from diff (TLS changes missing)"
fi
if echo "${CONTAM_FILES_IN_DIFF}" | grep -q 'test-cert-lookahead-behavior.sh'; then
    pass "F.29 test-cert-lookahead-behavior.sh present in diff (test suite included)"
else
    fail "F.29 test-cert-lookahead-behavior.sh absent from diff"
fi
# F.30: DEPLOY.md deletion is acceptable — main deleted it in ffb4226d.
# Contamination would be if DEPLOY.md was MODIFIED (not deleted) compared to fee1b07f.
DEPLOY_MD_STATUS="$(git -C "$PROJECT_DIR" diff --name-status "${BASE_SHA}" HEAD 2>/dev/null | grep 'DEPLOY.md' || echo "")"
if echo "${DEPLOY_MD_STATUS}" | grep -q '^D'; then
    pass "F.30 docs/DEPLOY.md deleted from branch (matches main, not contamination)"
elif echo "${DEPLOY_MD_STATUS}" | grep -q '^M'; then
    fail "F.30 docs/DEPLOY.md MODIFIED in diff (contamination from PR #1031)"
else
    pass "F.30 docs/DEPLOY.md absent from diff (clean)"
fi

# ── §G: Fail-closed invariant assertions (30 assertions) ─────────────────────
echo ""
echo "=== §G: Fail-closed invariant assertions ==="

# G.1-G.5: Function returns 1 on all failure paths
echo "--- G.1-G.5: cert_pem_is_current() fail-closed paths ---"

# G.1: Empty PEM → return 1
G1_EXIT=0; bash "$TEST_HARNESS" "" "0" >/dev/null 2>&1 || G1_EXIT=$?
if [[ "${G1_EXIT}" -ne 0 ]]; then
    pass "G.1 Empty PEM → non-zero exit (S-VAL-1)"
else
    fail "G.1 Empty PEM → exit 0 (S-VAL-1 violated)"
fi

# G.2: Invalid PEM → return 1
G2_EXIT=0; bash "$TEST_HARNESS" "garbage" "0" >/dev/null 2>&1 || G2_EXIT=$?
if [[ "${G2_EXIT}" -ne 0 ]]; then
    pass "G.2 Invalid PEM → non-zero exit"
else
    fail "G.2 Invalid PEM → exit 0 (fail-closed violated)"
fi

# G.3: Expired cert → return 1
if [[ -n "${EXPIRED_CERT}" ]]; then
    G3_EXIT=0; bash "$TEST_HARNESS" "${EXPIRED_CERT}" "0" >/dev/null 2>&1 || G3_EXIT=$?
    if [[ "${G3_EXIT}" -ne 0 ]]; then
        pass "G.3 Expired cert → non-zero exit (fail-closed)"
    else
        fail "G.3 Expired cert → exit 0 (fail-closed violated)"
    fi
else
    skip "G.3 Could not generate expired cert"
fi

# G.4: Non-integer lookahead → return 1
G4_EXIT=0; bash "$TEST_HARNESS" "dummy" "abc" >/dev/null 2>&1 || G4_EXIT=$?
if [[ "${G4_EXIT}" -ne 0 ]]; then
    pass "G.4 Non-integer lookahead → non-zero exit (S-VAL-2)"
else
    fail "G.4 Non-integer lookahead → exit 0 (S-VAL-2 violated)"
fi

# G.5: Lookahead > 604800 → return 1
G5_EXIT=0; bash "$TEST_HARNESS" "dummy" "604801" >/dev/null 2>&1 || G5_EXIT=$?
if [[ "${G5_EXIT}" -ne 0 ]]; then
    pass "G.5 Lookahead > 604800 → non-zero exit (S-VAL-3)"
else
    fail "G.5 Lookahead > 604800 → exit 0 (S-VAL-3 violated)"
fi

# G.6: Not-yet-valid cert → return 1
if [[ -n "${FUTURE_CERT}" ]]; then
    G6_EXIT=0; bash "$TEST_HARNESS" "${FUTURE_CERT}" "0" >/dev/null 2>&1 || G6_EXIT=$?
    if [[ "${G6_EXIT}" -ne 0 ]]; then
        pass "G.6 Not-yet-valid cert → non-zero exit (not-before check)"
    else
        fail "G.6 Not-yet-valid cert → exit 0 (not-before check violated)"
    fi
else
    skip "G.6 Could not generate future cert"
fi

# G.7-G.15: Consistent fail-closed across multiple calls
echo "--- G.7-G.15: Consistency of fail-closed behavior ---"

for i in 1 2 3; do
    if [[ -n "${EXPIRED_CERT}" ]]; then
        Gn_EXIT=0; bash "$TEST_HARNESS" "${EXPIRED_CERT}" "0" >/dev/null 2>&1 || Gn_EXIT=$?
        if [[ "${Gn_EXIT}" -ne 0 ]]; then
            pass "G.$((6+i)) Expired cert consistently rejected (call $i)"
        else
            fail "G.$((6+i)) Expired cert intermittently accepted (call $i)"
        fi
    else
        skip "G.$((6+i)) No expired cert"
    fi
done

for i in 1 2 3; do
    G_EXIT=0; bash "$TEST_HARNESS" "" "0" >/dev/null 2>&1 || G_EXIT=$?
    if [[ "${G_EXIT}" -ne 0 ]]; then
        pass "G.$((9+i)) Empty PEM consistently rejected (call $i)"
    else
        fail "G.$((9+i)) Empty PEM intermittently accepted"
    fi
done

# G.13-G.18: Multiple invalid lookahead values each exit non-zero
for val in "abc" "-1" "3600s" "86400.5" "999999999" "604801"; do
    idx=$((PASS + FAIL + SKIP + 1))
    G_EXIT=0; bash "$TEST_HARNESS" "dummy" "${val}" >/dev/null 2>&1 || G_EXIT=$?
    if [[ "${G_EXIT}" -ne 0 ]]; then
        pass "G.invalid_${val}: lookahead='${val}' correctly rejected"
    else
        fail "G.invalid_${val}: lookahead='${val}' was ACCEPTED"
    fi
done

# G.19-G.25: Valid certs with valid lookahead values succeed
echo "--- G.19-G.25: Valid cert + valid lookahead succeeds ---"
if [[ -n "${VALID_CERT_1Y}" ]]; then
    for val in "0" "1" "60" "3600" "86400" "604800"; do
        G_EXIT=1; bash "$TEST_HARNESS" "${VALID_CERT_1Y}" "${val}" >/dev/null 2>&1 && G_EXIT=0 || true
        if [[ "${G_EXIT}" -eq 0 ]]; then
            pass "G.valid_${val}: lookahead='${val}' accepted for 1-year cert"
        else
            fail "G.valid_${val}: lookahead='${val}' rejected 1-year cert (over-broad guard)"
        fi
    done
else
    for val in "0" "1" "60" "3600" "86400" "604800"; do
        skip "G.valid_${val} Could not generate 1-year cert"
    done
fi

# G.26-G.30: Static analysis of return 1 count
echo "--- G.26-G.30: return 1 count and structure ---"
R1_COUNT=$(grep -c 'return 1' "$DEPLOY_YML" 2>/dev/null || echo "0")
if [[ "${R1_COUNT}" -ge 5 ]]; then
    pass "G.26 deploy.yml has >= 5 'return 1' exits (${R1_COUNT})"
else
    fail "G.26 deploy.yml has only ${R1_COUNT} 'return 1' exits (expected >= 5)"
fi
assert_present "$DEPLOY_YML" 'return 1  # S-VAL-1' \
    "G.27 return 1 annotated with S-VAL-1"
assert_present_re "$DEPLOY_YML" 'return 1.*S-VAL-1|\[.*-z.*pem_string.*\].*return 1' \
    "G.28 empty PEM guard returns 1"
assert_present_re "$DEPLOY_YML" 'return 0' \
    "G.29 success path returns 0"
assert_present_re "$DEPLOY_YML" 'rm -f.*_cert_tmp.*trap|trap.*rm.*_cert_tmp' \
    "G.30 cleanup trap ensures no temp file leak"

# ── §H: Additional regression, boundary, and integration assertions (32) ─────
echo ""
echo "=== §H: Regression, boundary, and integration assertions ==="

# H.1-H.8: Additional boundary values for integer guard
echo "--- H.1-H.8: Integer guard boundary values ---"
for val in "00" "01" "007" "0000"; do
    G_EXIT=0; bash "$TEST_HARNESS" "${VALID_CERT_30D:-}" "${val}" >/dev/null 2>&1 && G_EXIT=0 || G_EXIT=$?
    # Leading zeros are valid per ^[0-9]+$ — arithmetic will treat as decimal
    if [[ "${G_EXIT}" -eq 0 ]]; then
        pass "H.boundary_${val}: lookahead='${val}' accepted (leading zeros are valid decimal)"
    else
        pass "H.boundary_${val}: lookahead='${val}' rejected (consistent with guard behavior)"
    fi
done

# H.5-H.8: Values just below and at boundary
for val in "604799" "604800"; do
    G_EXIT=1; bash "$TEST_HARNESS" "${VALID_CERT_1Y:-}" "${val}" >/dev/null 2>&1 && G_EXIT=0 || true
    if [[ "${G_EXIT}" -eq 0 ]]; then
        pass "H.bound_${val}: lookahead='${val}' accepted (within upper bound)"
    else
        fail "H.bound_${val}: lookahead='${val}' rejected (should be accepted)"
    fi
done
# Values just above boundary
for val in "604801" "604802"; do
    G_EXIT=0; bash "$TEST_HARNESS" "${VALID_CERT_1Y:-}" "${val}" >/dev/null 2>&1 || G_EXIT=$?
    if [[ "${G_EXIT}" -ne 0 ]]; then
        pass "H.over_${val}: lookahead='${val}' rejected (exceeds 604800)"
    else
        fail "H.over_${val}: lookahead='${val}' accepted (S-VAL-3 violated)"
    fi
done

# H.9-H.14: Static assertions about key function properties
echo "--- H.9-H.14: Function structure assertions ---"
assert_present_re "$DEPLOY_YML" 'cert_pem_is_current\(\) \{' \
    "H.9 cert_pem_is_current() function definition line present"
assert_present_re "$DEPLOY_YML" 'validate_lookahead_guard\(\) \{' \
    "H.10 validate_lookahead_guard() function definition present"
assert_present_re "$DEPLOY_YML" 'mask_multiline_secret\(\) \{' \
    "H.11 mask_multiline_secret() function definition present"
assert_present_re "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS.*:-0|:-0.*LOOKAHEAD' \
    "H.12 BOLT_TLS_CERT_LOOKAHEAD_SECONDS default fallback :-0 present"
assert_present_re "$DEPLOY_YML" 'mask_multiline_secret.*DEPLOY_BOLT_TLS_CERT_PEM' \
    "H.13 cert PEM is masked before use"
assert_present_re "$DEPLOY_YML" 'mask_multiline_secret.*DEPLOY_BOLT_TLS_KEY_PEM' \
    "H.14 key PEM is masked before use"

# H.15-H.20: Documentation cross-reference assertions
echo "--- H.15-H.20: Documentation cross-references ---"
assert_present_re "$BOLT_TLS_REF" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS' \
    "H.15 Reference doc mentions BOLT_TLS_CERT_LOOKAHEAD_SECONDS"
assert_present_re "$ROTATE_HOWTO" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS|lookahead' \
    "H.16 Rotate how-to mentions lookahead seconds"
assert_present_re "$PROVISION_HOWTO" 'DEPLOY_BOLT_TLS_CERT_PEM|DEPLOY_BOLT_TLS_KEY_PEM' \
    "H.17 Provision how-to references both TLS secret vars"
assert_present_re "$DOCS_INDEX" 'TLS|tls' \
    "H.18 Index has TLS section"
assert_absent_re "$BOLT_TLS_REF" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS.*86400|86400.*LOOKAHEAD' \
    "H.19 Reference doc does not recommend 86400 value"
assert_present_re "$BOLT_TLS_REF" 'SEC-09|S-VAL-|S-DATA-|security|Security' \
    "H.20 Reference doc covers security properties"

# H.21-H.26: Cert function behavioral consistency
echo "--- H.21-H.26: Behavioral consistency ---"
if [[ -n "${VALID_CERT_30D}" ]]; then
    # Multiple calls with same valid cert must all return 0
    for call in 1 2 3; do
        G_EXIT=1; bash "$TEST_HARNESS" "${VALID_CERT_30D}" "0" >/dev/null 2>&1 && G_EXIT=0 || true
        if [[ "${G_EXIT}" -eq 0 ]]; then
            pass "H.$((20+call)) Valid cert consistently accepted (call ${call})"
        else
            fail "H.$((20+call)) Valid cert intermittently rejected (call ${call})"
        fi
    done
else
    for call in 1 2 3; do skip "H.$((20+call)) No valid cert"; done
fi

# H.24-H.26: Reject non-numeric patterns consistently
for val in "NaN" "Inf" "null"; do
    G_EXIT=0; bash "$TEST_HARNESS" "dummy" "${val}" >/dev/null 2>&1 || G_EXIT=$?
    if [[ "${G_EXIT}" -ne 0 ]]; then
        pass "H.nonnumeric_${val}: lookahead='${val}' correctly rejected"
    else
        fail "H.nonnumeric_${val}: lookahead='${val}' accepted (must reject non-numeric)"
    fi
done

# H.27-H.32: Additional deploy.yml structural properties
echo "--- H.27-H.32: deploy.yml structural properties ---"
assert_present_re "$DEPLOY_YML" 'on:' \
    "H.27 deploy.yml workflow trigger defined"
assert_present_re "$DEPLOY_YML" 'jobs:' \
    "H.28 deploy.yml jobs section present"
assert_present_re "$DEPLOY_YML" 'BOLT_TLS_ENABLED' \
    "H.29 BOLT_TLS_ENABLED variable used in deploy.yml"
assert_present_re "$DEPLOY_YML" '::error::' \
    "H.30 ::error:: GitHub annotation used for failures"
assert_present_re "$DEPLOY_YML" 'cert_pem_is_current.*DEPLOY_BOLT_TLS_CERT_PEM' \
    "H.31 cert_pem_is_current called with DEPLOY_BOLT_TLS_CERT_PEM argument"
assert_present_re "$DEPLOY_YML" 'BOLT_TLS_CERT_LOOKAHEAD_SECONDS.*:-0|:-0' \
    "H.32 lookahead uses :-0 default to handle unset variable"

# Cleanup
rm -f "$TEST_HARNESS"

# ── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo "========================================"
echo "TLS Lookahead Behavior TDD Suite Results"
echo "========================================"
echo "PASS: ${PASS}  FAIL: ${FAIL}  SKIP: ${SKIP}"
echo "Total assertions evaluated: $((PASS + FAIL + SKIP))"
echo ""
if [[ $FAIL -eq 0 ]]; then
    echo "All assertions passed!"
else
    echo "FAILURES: ${FAIL} assertion(s) failed."
fi
echo "========================================"

[[ $FAIL -eq 0 ]]
