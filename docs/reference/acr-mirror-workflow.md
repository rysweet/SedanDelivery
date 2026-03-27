# ACR Mirror Workflow Reference

## Overview

The `ghcr-publish` workflow publishes SedanDelivery container images to GitHub
Container Registry (GHCR) and then mirrors those same tags into the Azure
Container Registry `sd2mainend655uvglnfeacr.azurecr.io`.

The mirror step does not rebuild images. It copies the already-published GHCR
image references into ACR with `docker buildx imagetools create`, so the
registry contents stay aligned without recompiling Rust crates.

The workflow now does this in a separate `mirror-to-acr` job that depends on
the `build-and-push` matrix. GHCR publication still has to succeed first, but
an ACR outage no longer breaks the build-and-push job itself. When the full
build matrix succeeds and the mirror job is created, that mirror job downloads
the exact published tag set from the successful build job, so rerunning the
failed `mirror-to-acr` job within the same workflow run does not recalculate
date-based tags on a later day.

## Mirrored Images

The workflow mirrors the same image names emitted by the GHCR matrix:

- `sedandelivery`
- `sedandelivery-bolt`
- `sedandelivery-fabric-sync`
- `sedandelivery-sync`

Each tag produced by `docker/metadata-action` is mirrored, including:

- `latest` on the default branch
- date-based tags on the default branch
- full semver tags from `v*` releases (for example `1.2.3`)
- `major.minor` semver tags from `v*` releases (for example `1.2`)
- short SHA tags

## Required Access

The workflow has two gated stages before mirror publication is complete. It
requires three repository secrets plus ACR data-plane push permission for the
service principal:

| Requirement | Purpose |
|-------------|---------|
| `GH_AW_GITHUB_TOKEN` | Sets each GHCR package to public before the mirror step runs |
| `AZURE_CLIENT_ID` | ACR username for the service principal |
| `AZURE_CLIENT_SECRET` | ACR password for the service principal |
| `AcrPush` (or equivalent data-plane push rights) on `sd2mainend655uvglnfeacr` | Allows `docker buildx imagetools create` to publish mirrored manifests into ACR |

If `GH_AW_GITHUB_TOKEN` is unset, the workflow fails in the earlier `Make package
public` step and never reaches the ACR login or mirror loop.

If the Azure service principal can authenticate but does not have push rights on
`sd2mainend655uvglnfeacr`, the ACR login step can still succeed while the later
manifest mirror fails during publication.

The target registry is fixed in the workflow:

```yaml
registry: sd2mainend655uvglnfeacr.azurecr.io
```

## Mirror Flow

After `build-and-push` has already published the GHCR image and made the package
public, the `mirror-to-acr` job:

1. Authenticates to GitHub Container Registry for source-image reads
2. Downloads the `ghcr-tags-<image>` artifact written by `build-and-push`
3. Authenticates to `sd2mainend655uvglnfeacr.azurecr.io`
4. Derives `ACR_IMAGE` from the active matrix image name inside the final mirror
   shell step:

```bash
ACR_IMAGE="sd2mainend655uvglnfeacr.azurecr.io/${{ matrix.image }}"
```

5. Reads each source tag from that artifact
6. Derives the destination tag name

```bash
tag="${src_tag##*:}"
```

7. Copies the manifest into ACR with:

```bash
docker buildx imagetools create \
  --tag "sd2mainend655uvglnfeacr.azurecr.io/<image>:<tag>" \
  "ghcr.io/<owner>/<image>:<tag>"
```

Because the source tags come from the artifact captured by the successful GHCR
publish job, `mirror-to-acr` reuses the exact tag set that was actually built
for that matrix row. That keeps a rerun of the failed mirror job aligned with
the original publish even when the workflow includes date-based tags. Starting a
fresh `ghcr-publish` workflow run is different: it creates a new metadata set
and a new tag artifact for that run. Likewise, if any `build-and-push` matrix
entry fails, GitHub skips the entire `mirror-to-acr` matrix, so there is no
failed mirror job to rerun in that workflow run.

## Troubleshooting

| Symptom | Likely cause | Resolution |
|---------|--------------|------------|
| Mirror step never starts | `GH_AW_GITHUB_TOKEN` missing, package visibility update failed, or one build matrix entry failed before mirror creation | Fix the failing prerequisite, then start a new `ghcr-publish` workflow run |
| ACR login fails | Missing or invalid Azure secrets | Verify `AZURE_CLIENT_ID` and `AZURE_CLIENT_SECRET` |
| ACR login succeeds but mirror push fails | Service principal lacks `AcrPush` (or equivalent) on the target registry | Grant data-plane push rights on `sd2mainend655uvglnfeacr`, then rerun the failed `mirror-to-acr` job in the same workflow run if it exists |
| GHCR image exists but ACR tag is missing | Mirror step failed after publish | Rerun the failed `mirror-to-acr` job in the same workflow run if it exists, and inspect the downloaded tag artifact plus the ACR login / imagetools steps |
| Only some tags appear in ACR | The published tag artifact is incomplete, stale, or you started a new workflow run | Inspect the `ghcr-tags-<image>` artifact from `build-and-push`; if you need a new publish, start a new `ghcr-publish` run instead of treating it as a mirror-job rerun |
