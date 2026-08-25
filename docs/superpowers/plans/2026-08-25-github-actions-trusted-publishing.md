# GitHub Actions Trusted Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a matching Rust package version to crates.io and create its GitHub Release whenever a `v*` tag is pushed to `mapseekai/cog_validator`.

**Architecture:** A single GitHub Actions workflow is bound to the preconfigured `crates-io` Environment. It checks that the tag and Cargo package version agree, tests and dry-runs the package, exchanges GitHub OIDC identity for a short-lived crates.io token, performs an idempotent publish, and then creates or reconciles the GitHub Release.

**Tech Stack:** GitHub Actions, GitHub OIDC, `rust-lang/crates-io-auth-action@v1`, Cargo, GitHub CLI, GDAL development packages.

**Spec:** `docs/superpowers/specs/2026-08-25-github-actions-trusted-publishing-design.md`

## Global Constraints

- The configured trusted publisher must be `mapseekai/cog_validator`, workflow `release.yml`, Environment `crates-io`.
- No long-lived crates.io token or publishing secret is added to GitHub or the repository.
- The publish job must use only `contents: write` and `id-token: write` permissions.
- A release tag must exactly equal `v` plus the package version in `Cargo.toml`.
- All Cargo publishing commands must pass `--locked --registry crates-io`.
- GitHub Release creation occurs only after the crate is published or confirmed already present on crates.io.

---

### Task 1: Migrate Repository Identity

**Files:**
- Modify: `Cargo.toml:11`
- Modify: `README.md:67`
- Modify: local Git configuration for `origin`
- Test: Cargo metadata and URL checks

**Interfaces:**
- Consumes: the canonical SSH remote `git@github.com:mapseekai/cog_validator.git`.
- Produces: package metadata and clone instructions that identify `mapseekai/cog_validator`.

**Configuration-test exception:** This task changes repository metadata and local Git configuration, not production source behavior. The approved design uses direct metadata and URL acceptance checks instead of a test-first source-code cycle.

- [ ] **Step 1: Update the package repository metadata**

Change the `[package]` `repository` field in `Cargo.toml` to:

~~~
repository = "https://github.com/mapseekai/cog_validator"
~~~

- [ ] **Step 2: Update the documented clone command**

Change the Installation example in `README.md` to:

~~~
git clone https://github.com/mapseekai/cog_validator.git
~~~

- [ ] **Step 3: Point the local origin remote at the canonical SSH URL**

Run:

~~~
git remote set-url origin git@github.com:mapseekai/cog_validator.git
~~~

- [ ] **Step 4: Verify the three repository identities agree**

Run:

~~~
test "$(git remote get-url origin)" = "git@github.com:mapseekai/cog_validator.git"
test "$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].repository')" = "https://github.com/mapseekai/cog_validator"
rg -F "git clone https://github.com/mapseekai/cog_validator.git" README.md
~~~

Expected: all commands exit with status `0`.

- [ ] **Step 5: Commit the repository migration**

~~~
git add Cargo.toml README.md
git commit -m "chore: update repository URL"
~~~

### Task 2: Add Idempotent Trusted-Publishing Workflow

**Files:**
- Create: `.github/workflows/release.yml`
- Test: YAML parse and workflow structural acceptance checks

**Interfaces:**
- Consumes: tags named `v<package-version>`, Environment `crates-io`, and the trusted publisher configured for `release.yml`.
- Produces: a crates.io package publication and GitHub Release for the tag.

**Configuration-test exception:** GitHub Actions YAML is declarative configuration. Its acceptance checks validate syntax plus each security and release-flow invariant; Cargo's dry run validates the actual package without uploading it.

- [ ] **Step 1: Create the release workflow**

Create `.github/workflows/release.yml` with this content:

~~~yaml
name: Publish crate

on:
  push:
    tags:
      - "v*"

permissions:
  contents: write
  id-token: write

concurrency:
  group: publish-${{ github.ref_name }}
  cancel-in-progress: false

jobs:
  publish:
    name: Publish ${{ github.ref_name }}
    runs-on: ubuntu-24.04
    environment: crates-io
    steps:
      - name: Check out the tagged source
        uses: actions/checkout@v6

      - name: Install GDAL development files
        run: |
          sudo apt-get update
          sudo apt-get install --yes libgdal-dev pkg-config

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Validate tag and package version
        id: package
        shell: bash
        run: |
          set -euo pipefail
          metadata="$(cargo metadata --no-deps --format-version 1)"
          package_name="$(jq -r '.packages[0].name' <<<"$metadata")"
          package_version="$(jq -r '.packages[0].version' <<<"$metadata")"

          if [[ "$GITHUB_REF_NAME" != "v$package_version" ]]; then
            echo "Tag $GITHUB_REF_NAME does not match package version $package_version" >&2
            exit 1
          fi

          printf 'name=%s\n' "$package_name" >> "$GITHUB_OUTPUT"
          printf 'version=%s\n' "$package_version" >> "$GITHUB_OUTPUT"

      - name: Test the package
        run: cargo test --locked

      - name: Verify the package can be published
        run: cargo publish --dry-run --locked --registry crates-io

      - name: Authenticate with crates.io
        id: crates_io_auth
        uses: rust-lang/crates-io-auth-action@v1

      - name: Publish the package when its version is absent
        env:
          CARGO_REGISTRY_TOKEN: ${{ steps.crates_io_auth.outputs.token }}
          PACKAGE_NAME: ${{ steps.package.outputs.name }}
          PACKAGE_VERSION: ${{ steps.package.outputs.version }}
        shell: bash
        run: |
          set -euo pipefail
          status="$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' "https://crates.io/api/v1/crates/$PACKAGE_NAME/$PACKAGE_VERSION")"

          case "$status" in
            200)
              echo "$PACKAGE_NAME $PACKAGE_VERSION already exists on crates.io; skipping upload."
              ;;
            404)
              cargo publish --locked --registry crates-io
              ;;
            *)
              echo "Unable to determine crates.io publication state (HTTP $status)." >&2
              exit 1
              ;;
          esac

      - name: Create or reconcile the GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
          TAG_NAME: ${{ github.ref_name }}
        shell: bash
        run: |
          set -euo pipefail

          if gh release view "$TAG_NAME" >/dev/null 2>&1; then
            gh release edit "$TAG_NAME" --title "$TAG_NAME"
          else
            gh release create "$TAG_NAME" --verify-tag --title "$TAG_NAME" --generate-notes
          fi
~~~

- [ ] **Step 2: Parse the workflow YAML**

Run:

~~~bash
ruby -e 'require "yaml"; YAML.parse_file(".github/workflows/release.yml")'
~~~

Expected: exit status `0` with no parsing error.

- [ ] **Step 3: Check the security and flow-critical declarations**

Run:

~~~bash
rg -F "id-token: write" .github/workflows/release.yml
rg -F "contents: write" .github/workflows/release.yml
rg -F "environment: crates-io" .github/workflows/release.yml
rg -F "rust-lang/crates-io-auth-action@v1" .github/workflows/release.yml
rg -F "cargo publish --locked --registry crates-io" .github/workflows/release.yml
rg -F "gh release create" .github/workflows/release.yml
~~~

Expected: each command prints one matching workflow line and exits with status `0`.

- [ ] **Step 4: Review the rendered diff and commit the workflow**

Run:

~~~bash
git diff --check
git diff -- .github/workflows/release.yml
git add .github/workflows/release.yml
git commit -m "ci: publish releases through trusted publishing"
~~~

Expected: no whitespace errors; the commit contains only the release workflow.

### Task 3: Verify Release Readiness Without Publishing

**Files:**
- Verify: `Cargo.toml`
- Verify: `README.md`
- Verify: `.github/workflows/release.yml`
- Test: package test suite and Cargo's non-uploading publish path

**Interfaces:**
- Consumes: the migrated repository URL and new workflow.
- Produces: evidence that the checked-in release source can package and compile for crates.io without uploading a new version.

- [ ] **Step 1: Run the full Rust test suite**

~~~bash
cargo test --locked
~~~

Expected: exit status `0`.

- [ ] **Step 2: Run the crates.io non-uploading publication verification**

~~~bash
cargo publish --dry-run --locked --registry crates-io
~~~

Expected: Cargo packages and verifies `cog_validator` successfully, then reports that upload was aborted because this is a dry run.

- [ ] **Step 3: Confirm the working tree only contains planned changes**

~~~bash
git status --short --branch
git log --oneline -3
~~~

Expected: the branch contains the repository migration and trusted-publishing workflow commits, with no untracked implementation files.

- [ ] **Step 4: Manually validate the first real release protocol**

After changing `Cargo.toml` to the next unpublished version and updating the changelog, run:

~~~bash
git tag v<next-version>
git push origin v<next-version>
~~~

Expected: the `Publish crate` workflow validates the matching version, publishes it through Trusted Publishing, and creates the corresponding GitHub Release.

