# GitHub Actions Trusted Publishing Design

## Goal

Automatically publish each tagged `cog_validator` release to crates.io and
create the corresponding GitHub Release in `mapseekai/cog_validator`, without
storing a long-lived crates.io API token in GitHub.

## Context

- This repository is a single Rust library package named `cog_validator`.
- `Cargo.toml` currently declares version `0.2.2`; that version already exists
  on crates.io and passes `cargo publish --dry-run --locked --registry
  crates-io` locally.
- The local `origin` and the package metadata still point at the former
  `zwishing/cog_validator` repository.
- crates.io Trusted Publishing has been configured by the repository owner.

## Scope

The change will:

1. Move repository metadata and the local `origin` remote to
   `mapseekai/cog_validator`.
2. Add a single tag-triggered release workflow at
   `.github/workflows/release.yml`.
3. Publish through crates.io Trusted Publishing using a short-lived OIDC token.
4. Create or reconcile a GitHub Release only after the crate version exists on
   crates.io.

It will not add separate pull-request CI, change the crate API, publish binary
assets, or introduce a release-management dependency.

## Trusted Publisher Contract

The externally configured trusted publisher must match these exact values:

| Field | Value |
| --- | --- |
| GitHub owner | `mapseekai` |
| GitHub repository | `cog_validator` |
| Workflow filename | `release.yml` |
| GitHub Environment | `crates-io` |

The GitHub repository must contain an Environment named `crates-io`. To retain
fully automatic publishing, it has no required reviewers; it should only allow
deployments from tags matching `v*`.

## Release Flow

Pushing a tag matching `v*` starts one `publish` job on GitHub-hosted Ubuntu.
The job uses only the following permissions:

```yaml
permissions:
  contents: write
  id-token: write
```

`contents: write` permits creation of the GitHub Release; `id-token: write`
permits GitHub to issue the OIDC identity token that crates.io exchanges for a
temporary publishing token.

The job performs these operations in order:

1. Check out the tagged source and install the stable Rust toolchain plus the
   GDAL development headers and `pkg-config` required by `gdal-sys`.
2. Read the package version from Cargo metadata and require the tag to equal
   `v<package-version>`. A mismatch fails before any remote publication.
3. Run `cargo test --locked` and
   `cargo publish --dry-run --locked --registry crates-io`.
4. Obtain a temporary token with
   `rust-lang/crates-io-auth-action@v1`; the token is supplied only to the
   `cargo publish` command as `CARGO_REGISTRY_TOKEN`.
5. Query the exact crate/version on crates.io. If it does not exist, run
   `cargo publish --locked --registry crates-io`; if it already exists, skip
   uploading so a retry can proceed.
6. Create a GitHub Release with generated notes. If one already exists for the
   same tag, reconcile it rather than fail a retried workflow run.

The workflow uses a per-tag concurrency group so duplicate runs for the same
tag cannot publish concurrently.

## Failure and Retry Semantics

- An invalid tag, compilation/test failure, or failed dry run prevents both
  publication and release creation.
- Trusted Publishing authorization failures prevent `cargo publish`; no token
  is persisted in repository settings or workflow logs.
- crates.io publication is immutable. If publication succeeds but GitHub Release
  creation later fails, rerunning the job detects the existing crate version
  and finishes the GitHub Release instead of attempting a duplicate upload.
- A GitHub Release is never created before the crate is published or confirmed
  to already exist.

## Repository URL Migration

The implementation updates:

- `Cargo.toml` package `repository` to
  `https://github.com/mapseekai/cog_validator`;
- the README clone command to
  `https://github.com/mapseekai/cog_validator.git`;
- the local `origin` URL to
  `git@github.com:mapseekai/cog_validator.git`.

The author email is unrelated to the repository move and remains unchanged.

## Verification

Configuration files do not have a meaningful unit-test-first cycle. Verification
will therefore use workflow syntax linting when available, a source-level
inspection of permissions/triggers/environment, and Cargo's real non-uploading
publication check. The final end-to-end check is a new version bump followed by
the matching tag, for example `0.2.3` and `v0.2.3`.

No test tag will use `v0.2.2`, because crates.io already contains `0.2.2`.
