---
title: How to Provision Bolt TLS Secrets
description: Initial provisioning of DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM for a GitHub environment
updated: 2026-03-28
---

# How to Provision Bolt TLS Secrets

Use this guide when setting up `DEPLOY_BOLT_TLS_CERT_PEM` and `DEPLOY_BOLT_TLS_KEY_PEM` for a
new GitHub environment. If you are rotating an existing cert that has already expired, see
[How to Rotate the Bolt TLS Certificate](./rotate-bolt-tls-cert.md) instead.

## Prerequisites

- `gh` CLI authenticated and authorised to manage secrets for the repository
- `openssl` installed locally
- A valid PEM certificate chain and private key

---

## Step 1: Obtain or generate a certificate

### Production environments (main branch)

Use a certificate from a trusted CA (Let's Encrypt, Azure Certificate Service, or your
organisation's PKI). The cert must be:

- Valid at the time of deploy (`openssl x509 -checkend 0 -noout` exits 0)
- In PEM format (begins with `-----BEGIN CERTIFICATE-----`)
- A full chain if intermediate CAs are required

### Branch / PR environments

A self-signed cert is sufficient for isolated branch environments:

```bash
# Generate a 90-day self-signed cert valid from now
openssl req -x509 -newkey rsa:4096 -keyout bolt-key.pem -out bolt-cert.pem \
  -days 90 -nodes \
  -subj "/CN=bolt.example.internal"
```

---

## Step 2: Verify the certificate before provisioning

The deploy gate uses `openssl x509 -checkend 0` to validate the cert. Run the same check
locally to confirm the cert will pass:

```bash
# Verify using checkend 0 — this matches the deploy gate's default (BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0")
openssl x509 -in bolt-cert.pem -checkend 0 -noout && echo "OK: cert is currently valid"

# Inspect dates and subject for reference
openssl x509 -in bolt-cert.pem -noout -dates -subject
```

The gate checks `checkend 0`. A cert expiring in a few hours is still valid for provisioning
purposes — it will pass the gate today. Do not use a higher checkend value when pre-validating:
only use `checkend 0` to match the gate's behavior.

---

## Step 3: Determine the target GitHub environment

The deploy workflow maps each branch to a GitHub environment:

```bash
BRANCH_NAME="$(git rev-parse --abbrev-ref HEAD)"
BRANCH_ENV="$(printf '%s' "$BRANCH_NAME" \
  | sed 's/[^a-zA-Z0-9-]/-/g' \
  | tr '[:upper:]' '[:lower:]' \
  | cut -c1-20 \
  | sed 's/^-*//;s/-*$//')"
echo "GitHub environment: ${BRANCH_ENV}"
```

For the `main` branch the environment name is `main`.

---

## Step 4: Set the secrets

```bash
# Set both secrets for the target environment
gh secret set DEPLOY_BOLT_TLS_CERT_PEM \
  --env "${BRANCH_ENV}" \
  --repo cloud-ecosystem-security/SedanDelivery \
  < bolt-cert.pem

gh secret set DEPLOY_BOLT_TLS_KEY_PEM \
  --env "${BRANCH_ENV}" \
  --repo cloud-ecosystem-security/SedanDelivery \
  < bolt-key.pem
```

Both secrets must be set together. If either is absent, the deploy gate will fail with:

```
::error::DEPLOY_BOLT_TLS_CERT_PEM and DEPLOY_BOLT_TLS_KEY_PEM must both be set; provision TLS secrets before deploying
```

---

## Step 5: Confirm the gate accepts the cert

After setting the secrets, trigger a deploy and check the step output for:

```
Deploying standalone Bolt server: sd2-<env>-bolt
```

If the gate fails with `expired or not yet valid`, verify the cert locally:

```bash
openssl x509 -in bolt-cert.pem -checkend 0 -noout && echo "Valid" || echo "Expired"
openssl x509 -in bolt-cert.pem -noout -dates
```

A `notBefore` in the future means the cert is not yet valid. Wait until the `notBefore`
timestamp passes, or reissue the cert with an earlier `notBefore`.

---

## Cert expiry monitoring

With `BOLT_TLS_CERT_LOOKAHEAD_SECONDS: "0"` (the default), the deploy gate does not warn
about upcoming expiry. Set up external monitoring to alert before the cert expires:

- Use a scheduled GitHub Actions workflow with `openssl x509 -checkend 604800` (7 days)
- Configure Azure Monitor to alert on cert expiry metrics

See [Bolt TLS Deploy Gate Reference — Required: External Cert Expiry Monitoring](../reference/bolt-tls-deploy.md#required-external-cert-expiry-monitoring)
for guidance on setting up the required compensating control.

---

## Related

- [How to Rotate the Bolt TLS Certificate](./rotate-bolt-tls-cert.md) — when to rotate and step-by-step recovery
- [Bolt TLS Deploy Gate Reference](../reference/bolt-tls-deploy.md) — `BOLT_TLS_CERT_LOOKAHEAD_SECONDS` semantics, failure modes
- [Secrets Configuration](../secrets.md) — full secret inventory and rotation cadence
