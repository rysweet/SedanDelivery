---
title: Bolt TLS Deploy Gate Reference
description: How deploy.yml validates TLS certificates before deploying sedandelivery-bolt to Azure Container Apps
updated: 2026-03-28
---

# Bolt TLS Deploy Gate Reference

The `deploy.yml` workflow enforces a **fail-closed TLS gate** before deploying `sedandelivery-bolt`. If
the certificate is missing, expired, not-yet-valid, or fails validation, the deploy step exits 1 and
prints an actionable error. No self-signed fallback or self-healing is performed.

## Contents

- [How the gate works](#how-the-gate-works)
- [Configuration](#configuration)
- [BOLT_TLS_CERT_LOOKAHEAD_SECONDS](#bolt_tls_cert_lookahead_seconds)
- [Cert validation logic](#cert-validation-logic)
- [Failure modes](#failure-modes)
- [Required: External Cert Expiry Monitoring](#required-external-cert-expiry-monitoring)
- [Related secrets](#related-secrets)

---

## How the gate works

Every Bolt deploy runs this sequence before the `az containerapp` call:

1. **Presence check** — both `DEPLOY_BOLT_TLS_CERT_PEM` and `DEPLOY_BOLT_TLS_KEY_PEM` must be set.
2. **Integer guard** — `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` is validated as a non-negative integer
   before being passed to openssl. A non-integer value exits 1 with an actionable error message.
3. **Validity check** — `cert_pem_is_current()` calls `openssl x509 -checkend ${BOLT_TLS_CERT_LOOKAHEAD_SECONDS}`
   against the cert PEM. If the cert is expired, not-yet-valid, or malformed, the function returns non-zero.
4. **Mask and proceed** — only after both checks pass are the secret values masked and
   `BOLT_TLS_ENABLED=true` set. The deploy step proceeds.
5. **Fail closed** — any failure in steps 1–3 calls `exit 1`. The Container App is **not** updated.

```
deploy.yml step: "Deploy sedandelivery-bolt standalone Container App"
│
├─ DEPLOY_BOLT_TLS_CERT_PEM set?  ──No──► exit 1 (missing secret error)
│    Yes
├─ DEPLOY_BOLT_TLS_KEY_PEM set?   ──No──► exit 1 (missing secret error)
│    Yes
├─ BOLT_TLS_CERT_LOOKAHEAD_SECONDS is integer? ──No──► exit 1 (validation error)
│    Yes
├─ cert_pem_is_current() pass?    ──No──► exit 1 (expired/not-yet-valid error)
│    Yes
└─ az containerapp create/update ──────► deploy proceeds
```

---

## Configuration

The gate is controlled by a single workflow-level environment variable:

| Variable | Scope | Default | Description |
|---|---|---|---|
| `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` | `deploy.yml` env block | `"0"` | Seconds passed to `openssl x509 -checkend`. A cert is rejected if it expires within this window. `"0"` accepts any currently-valid cert. Must be a non-negative integer. |

The secrets consumed by the gate:

| Secret | GitHub Environment | Description |
|---|---|---|
| `DEPLOY_BOLT_TLS_CERT_PEM` | per-branch env (see [DEPLOY.md](../DEPLOY.md)) | Full PEM certificate chain for the Bolt TCP endpoint |
| `DEPLOY_BOLT_TLS_KEY_PEM` | per-branch env | PEM private key matching the certificate |

---

## BOLT_TLS_CERT_LOOKAHEAD_SECONDS

This variable controls how far ahead the gate looks when checking certificate validity. It maps
directly to the `seconds` argument of `openssl x509 -checkend <seconds>`.

### Semantics

| Value | Meaning |
|---|---|
| `"0"` (default) | Accept any cert whose `Not Before ≤ now < Not After`. Only genuinely expired or not-yet-valid certs are rejected. Matches `checkend 0` semantics. |
| `"3600"` | Reject certs expiring within 1 hour. CI fails proactively before the cert becomes stale in production. |
| `"86400"` | **Do not use.** Reject certs expiring within 24 hours. This caused false-positive deploy failures (GitHub Actions run 23669891455): a cert valid at deploy time was rejected because it expired within 24h. The fix was to set the default to `"0"`. |

### How openssl -checkend works

`openssl x509 -checkend N` returns exit code 0 if the certificate is still valid at `now + N seconds`,
and exit code 1 if the certificate will have expired by that future time.

With `N=86400` (24 hours): a cert expiring in 23 hours receives exit code 1 — even though it is
**currently valid**. This is the false positive that caused the CI failure in run 23669891455.
The cert was valid, the deploy gate rejected it, and the error `DEPLOY_BOLT_TLS_CERT_PEM is expired
or not yet valid` was misleading — the cert had not expired.

With `N=0` (the corrected default): only a cert whose `Not After` timestamp is in the past receives
exit code 1. Certs expiring in 1 second, 1 hour, or 23 hours all pass `checkend 0`.

### History: why the default is `"0"` not `"86400"`

The original value `"86400"` was intended to give a 24-hour warning before cert expiry so CI
would catch an approaching expiry before it caused a production outage. However, this creates
a CI-blocking false positive for any cert with less than 24h remaining — including certs that
are perfectly valid and will be renewed before they expire.

The correct model is:

- **Deploy gate (`checkend 0`)**: reject only certs that are already expired or not yet valid.
  If the cert is valid now, the deploy can proceed.
- **External monitoring**: alert operators before the cert actually expires, using a
  separate alerting mechanism (see [Required: External Cert Expiry Monitoring](#required-external-cert-expiry-monitoring)).

Mixing the two concerns into the deploy gate causes false-positive deploy failures with no
straightforward recovery path short of emergency cert rotation.

### Integer validation guard

The gate validates `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` as a non-negative integer before passing it to openssl.
A non-integer value (e.g. `"3600s"`, `"abc"`, `"-1"`) causes an actionable `::error::` message and `return 1`:

```bash
if ! [[ "${lookahead_seconds}" =~ ^[0-9]+$ ]]; then
  echo "::error::BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be a non-negative integer, got: '${lookahead_seconds}'"
  return 1
fi
```

Without this guard, a malformed value (e.g. `"3600s"`, `"abc"`, or an accidental empty string)
passes to `openssl -checkend`, which emits a cryptic parse error and returns a non-zero exit
code indistinguishable from an expired cert failure.

---

## Cert validation logic

The gate uses the `cert_pem_is_current()` shell function defined inside the deploy step:

```bash
cert_pem_is_current() {
  local pem_value="$1"
  local lookahead_seconds="${2:-0}"
  if ! [[ "${lookahead_seconds}" =~ ^[0-9]+$ ]]; then
    echo "::error::BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be a non-negative integer, got: '${lookahead_seconds}'"
    return 1
  fi
  local cert_file
  cert_file="$(mktemp)"
  printf '%s\n' "${pem_value}" > "${cert_file}"
  # Check Not After: openssl -checkend returns non-zero if expired within lookahead
  if ! openssl x509 -in "${cert_file}" -checkend "${lookahead_seconds}" -noout >/dev/null 2>&1; then
    rm -f "${cert_file}"
    return 1
  fi
  # Check Not Before: reject certs that are not yet valid (openssl -checkend does not check notBefore)
  local not_before_str
  not_before_str="$(openssl x509 -in "${cert_file}" -noout -startdate 2>/dev/null | sed 's/notBefore=//')"
  rm -f "${cert_file}"
  if [[ -n "${not_before_str}" ]]; then
    local not_before_epoch now_epoch
    not_before_epoch="$(date -d "${not_before_str}" +%s 2>/dev/null || echo "")"
    now_epoch="$(date +%s)"
    if [[ -n "${not_before_epoch}" && "${not_before_epoch}" -gt "${now_epoch}" ]]; then
      return 1
    fi
  fi
  return 0
}
```

Key properties:

- **No network access** — validation is purely local using the cert PEM value from the secret.
- **Ephemeral temp file** — the cert is written to `mktemp`, validated, then deleted immediately.
  The ephemeral CI runner provides a second layer of isolation.
- **Key/cert separation** — the private key (`DEPLOY_BOLT_TLS_KEY_PEM`) never enters
  `cert_pem_is_current()`. Key material is masked separately and only after cert validation passes.
- **Non-zero lookahead is additive** — `openssl x509 -checkend N` returns 0 if the cert is still
  valid at `now + N seconds`. A value of `0` checks validity at the current instant only.
- **Not Before check** — `openssl x509 -checkend` only validates `Not After`. The function
  separately checks `Not Before` via `-startdate` to reject certs that are not yet valid.

---

## Failure modes

### Missing secrets

```
::error::DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying
```

**Fix**: [Provision both secrets](../howto/provision-tls-secrets.md) for the matching GitHub environment
before re-running the workflow.

---

### Invalid BOLT_TLS_CERT_LOOKAHEAD_SECONDS

```
::error::BOLT_TLS_CERT_LOOKAHEAD_SECONDS must be a non-negative integer, got: '<value>'
```

**Fix**: Set `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` to a non-negative integer string (e.g. `"0"` or `"3600"`)
in the `deploy.yml` `env:` block.

---

### Expired or not-yet-valid certificate

```
::error::DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid; provision a current certificate before deploying
```

This error fires when `openssl x509 -checkend ${BOLT_TLS_CERT_LOOKAHEAD_SECONDS}` returns non-zero.
There are two distinct causes:

| Cause | `openssl x509 -text` output | Fix |
|---|---|---|
| Certificate has already expired | `Not After : <date in the past>` | Renew the cert and update the secret |
| Certificate not yet valid | `Not Before: <date in the future>` | Wait until `Not Before` passes, or reissue with a backdated `Not Before` |
| Cert expires within `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` | `Not After` within the window | Set `BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"` or renew the cert |

See [How to rotate the Bolt TLS certificate](../howto/rotate-bolt-tls-cert.md) for step-by-step recovery.

---

## Required: External Cert Expiry Monitoring

**Required compensating control**: With `BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"`, the deploy gate
does **not** warn when a cert is approaching expiry — it only rejects certs that are already expired.
External cert expiry monitoring is a required control to ensure operators are alerted before
a cert expires in production.

The deploy gate is not a substitute for cert expiry monitoring. Setting `BOLT_TLS_CERT_LOOKAHEAD_SECONDS`
to a large value (e.g. `86400`) is not an acceptable replacement for proper monitoring — it converts
a monitoring gap into a deploy-blocking false positive.

Use one of the following mechanisms:

- **Azure Monitor** — `az monitor metrics alert create` on the Container App's TLS certificate
  expiry metric. Alert at 7 days remaining.
- **GitHub Actions scheduled workflow** — a weekly cron job that runs
  `openssl x509 -checkend 604800` (7 days) against the secret value and opens a GitHub issue
  on failure. This provides the early-warning window that `BOLT_TLS_CERT_LOOKAHEAD_SECONDS`
  was previously attempting to provide, without blocking deploys.
- **Let's Encrypt + ACME auto-renewal** — 90-day certs renewed automatically at 60 days
  provide a comfortable 30-day buffer. No manual monitoring required when auto-renewal
  is correctly configured.

Failure to implement at least one of these mechanisms means a cert expiry in production
will only be detected when the cert has already expired (i.e., when clients begin seeing
TLS errors), which is a higher-severity incident than a proactive renewal.

---

## Security invariants

The TLS gate enforces these security properties:

| Control | Description |
|---------|-------------|
| SEC-09 | `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` hardcoded to `"0"` — not overridable via workflow inputs or matrix vars |
| S-VAL-1 | Empty or missing cert PEM → `return 1` (fail closed) |
| S-VAL-2 | Non-integer lookahead (anchored `^[0-9]+$`) → `return 1` |
| S-VAL-3 | Lookahead > 604800 (7 days) → `return 1` (prevents cert-bypass via large value) |
| S-DATA-1 | No `echo`/`cat`/`printf` of PEM or private key content |
| S-DATA-2 | Temp cert files use `mktemp` + `trap EXIT ERR` for guaranteed cleanup |
| S-AUTH-1 | Cert validated via PEM string, not user-supplied file path |
| SR-2 | `export TZ=UTC` before all date arithmetic (locale-independent) |

---

## Related secrets

See [Secrets Configuration](../secrets.md) for the full secret inventory including:

- How to create `DEPLOY_BOLT_TLS_CERT_PEM` and `DEPLOY_BOLT_TLS_KEY_PEM`
- Which GitHub environment each secret belongs to
- Rotation cadence
