---
title: How to Rotate the Bolt TLS Certificate
description: Step-by-step guide for provisioning and rotating the TLS certificate used by sedandelivery-bolt in Azure Container Apps deployments
updated: 2026-03-28
---

# How to Rotate the Bolt TLS Certificate

Use this guide when:

- A deploy fails with `DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid`
- You are provisioning TLS secrets for a new GitHub environment
- You are renewing a certificate that is genuinely expired

## When NOT to Rotate

**Do not rotate the cert** unless it fails `openssl x509 -checkend 0`. The deploy gate uses
`BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"` (the default), which means it only rejects certs that are
**already expired or not yet valid**. A cert expiring in a few hours or even a few minutes will
still pass the deploy gate.

A common mistake is to run `openssl x509 -checkend 86400 -noout` to check validity, see a failure,
and immediately rotate. This is incorrect: `checkend 86400` may report failure even when the deploy
gate would accept the cert, because `checkend 86400` rejects certs expiring within 24 hours. The
deploy gate does not use `checkend 86400` — it uses `checkend 0`.

**Warning**: Do NOT use `checkend 86400` to decide whether to rotate. Use `checkend 0` (Step 1 below).
If `checkend 0` passes, the cert is valid and no rotation is needed for the deploy gate to succeed.

If `checkend 0` passes but CI still fails, check:

1. The `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` value in `.github/workflows/deploy.yml` — it must be `"0"`.
   If it has been set to `86400` or any other large value, that value is causing a false-positive failure.
2. Whether the correct GitHub environment secret is being used (the secret name and environment must match).

---

## Prerequisites

- `gh` CLI authenticated to the repository
- `openssl` installed locally
- A valid PEM certificate chain and private key (from your CA, Let's Encrypt, or self-signed for testing)

---

## Step 1: Verify whether rotation is actually needed

Run `checkend 0` to determine if the cert is genuinely expired:

```bash
# Check if cert is valid right now (matches the deploy gate's checkend 0 behavior)
openssl x509 -in cert.pem -checkend 0 -noout && echo "VALID — no rotation needed" || echo "EXPIRED or NOT YET VALID — rotation required"

# Also inspect the expiry date for context
openssl x509 -in cert.pem -noout -dates -subject
```

If `checkend 0` passes, the cert is currently valid and the deploy gate will accept it.
Stop here — rotation is not needed.

If `checkend 0` fails, the cert is either expired or not yet valid. Continue to Step 2.

---

## Step 2: Determine the target GitHub environment

The deploy workflow derives the GitHub environment name from the branch. For `main`, the
environment is `main`. For feature branches:

```bash
BRANCH_NAME="$(git rev-parse --abbrev-ref HEAD)"
BRANCH_ENV="$(printf '%s' "$BRANCH_NAME" \
  | sed 's/[^a-zA-Z0-9-]/-/g' \
  | tr '[:upper:]' '[:lower:]' \
  | cut -c1-20 \
  | sed 's/^-*//;s/-*$//')"
echo "GitHub environment: ${BRANCH_ENV}"
```

---

## Step 3: Update the secrets

Set both `DEPLOY_BOLT_TLS_CERT_PEM` and `DEPLOY_BOLT_TLS_KEY_PEM` for the environment:

```bash
# Replace BRANCH_ENV with the value from step 2 (e.g. "main" or "fix-issue-1021")
BRANCH_ENV="main"

gh secret set DEPLOY_BOLT_TLS_CERT_PEM \
  --env "${BRANCH_ENV}" \
  --repo cloud-ecosystem-security/SedanDelivery \
  < bolt-cert.pem

gh secret set DEPLOY_BOLT_TLS_KEY_PEM \
  --env "${BRANCH_ENV}" \
  --repo cloud-ecosystem-security/SedanDelivery \
  < bolt-key.pem
```

Both secrets must be set together. The deploy gate rejects a deploy if either is absent.

---

## Step 4: Re-run the failed deploy

```bash
# Re-run the specific failed job
gh run rerun <RUN_ID> --job "deploy-bolt"

# Or push an empty commit to trigger a fresh run
git commit --allow-empty -m "chore: re-trigger deploy after cert rotation"
git push
```

---

## Step 5: Confirm the gate passes

Watch the deploy step output in GitHub Actions. Successful validation looks like:

```
Deploying standalone Bolt server: sd2-<env>-bolt
...
TLS smoke test passed — Bolt endpoint presented the configured, unexpired TLS certificate
```

If you still see the `expired or not yet valid` error after rotation, the cert may have an
incorrect `Not Before` date. Check with:

```bash
openssl x509 -in bolt-cert.pem -noout -dates
```

A `notBefore` in the future means the cert is **not yet valid** — wait until that timestamp
passes, or reissue the cert with a `notBefore` in the past.

---

## Generating a self-signed certificate for testing

For PR or branch environments where you control the endpoint:

```bash
# Generate a 90-day self-signed cert (valid immediately)
openssl req -x509 -newkey rsa:4096 -keyout bolt-key.pem -out bolt-cert.pem \
  -days 90 -nodes \
  -subj "/CN=bolt.example.internal"

# Verify with checkend 0 (matches the deploy gate)
openssl x509 -in bolt-cert.pem -checkend 0 -noout && echo "Valid"
```

Then set the secrets as in step 3. Clients must use `bolt+ssc://` (skip cert verification)
when connecting to an endpoint backed by a self-signed cert.

---

## Troubleshooting

### Error: `DEPLOY_BOLT_TLS_CERT_PEM is expired or not yet valid`

The deploy gate ran `openssl x509 -checkend 0` against the secret value and it returned non-zero.

1. Verify with `openssl x509 -in cert.pem -checkend 0 -noout` locally.
2. If the cert is genuinely expired, obtain a new certificate from your CA.
3. Verify it locally: `openssl x509 -in new-cert.pem -checkend 0 -noout && echo "OK"`
4. Set the new secret: `gh secret set DEPLOY_BOLT_TLS_CERT_PEM --env <env> < new-cert.pem`
5. Re-run the deploy.

### Error: `DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set`

One or both secrets are missing for this GitHub environment. Run step 3 above to set both.

### Deploy passes but clients see a certificate mismatch

The running Container App is presenting a different cert than the one you set. This can happen
when:

- The Container App retains an old secret version — the smoke test verifies the live cert
  against the configured one; an error here means the rollout is still in progress (wait and retry).
- The wrong environment was targeted — verify `BRANCH_ENV` from step 2 matches the environment
  shown in the deploy job logs.

---

## Related

- [Bolt TLS Deploy Gate Reference](../reference/bolt-tls-deploy.md) — full reference for
  `BOLT_TLS_CERT_LOOKAHEAD_SECONDS`, `cert_pem_is_current()`, and failure semantics
- [Provision TLS Secrets](./provision-tls-secrets.md) — initial provisioning guide for new environments
- [Secrets Configuration](../secrets.md) — secret inventory and rotation cadence
- [Deploy Your Own Instance](../DEPLOY.md) — full deployment guide including TLS secret setup
