---
name: release
description: Prepare, publish, verify, and recover pi releases. Use for release preparation, local release smoke tests, publishing, and failed release CI or announcements.
---

# Releasing pi

Run repository commands from the repo root (two directories above this skill), unless instructed otherwise.

**Lockstep versioning**: all packages share one version; every release updates all together. `patch` = fixes + additions, `minor` = breaking changes. No major releases.

1. **Audit CHANGELOGs automatically**: before releasing, inspect all commits and diffs since the previous release and audit every affected package's `[Unreleased]` section. Add or correct missing entries, run `npm run check`, commit the changelog updates, and push `main`. Do not ask the user to run `/cl`; the releasing agent owns this audit.

2. **Local smoke test**: build an unpublished release and smoke test from outside the repo (so it can't resolve workspace files):
   ```bash
   npm run release:local -- --out /tmp/pi-local-release --force
   cd /tmp

   # Node package install smoke tests
   /tmp/pi-local-release/node/pi --help
   /tmp/pi-local-release/node/pi --version
   /tmp/pi-local-release/node/pi --list-models
   /tmp/pi-local-release/node/pi -p "Say exactly: ok"
   /tmp/pi-local-release/node/pi

   # Bun binary smoke tests
   /tmp/pi-local-release/bun/pi --help
   /tmp/pi-local-release/bun/pi --version
   /tmp/pi-local-release/bun/pi --list-models
   /tmp/pi-local-release/bun/pi -p "Say exactly: ok"
   /tmp/pi-local-release/bun/pi
   ```
   Verify both Node and Bun startup, model/account listing, interactive startup, and at least one real prompt with the intended default provider. The bare commands `/tmp/pi-local-release/node/pi` and `/tmp/pi-local-release/bun/pi` start interactive mode; run each in tmux, submit a prompt, and wait for the model reply before considering the interactive smoke test passed. Failures are release blockers unless the user explicitly accepts the risk.

   Load and follow [interactive-testing.md](interactive-testing.md) for the tmux workflow. Start each release binary from `/tmp`, not the repo root.

3. **Run the release script**:
   ```bash
   PI_ALLOW_LOCKFILE_CHANGE=1 npm_config_min_release_age=0 npm run release:patch    # fixes + additions
   PI_ALLOW_LOCKFILE_CHANGE=1 npm_config_min_release_age=0 npm run release:minor    # breaking changes
   ```
   Review any lockfile, shrinkwrap, or install lock diffs the release creates before push. The fork retains its published CLI shrinkwrap even though upstream recommends the managed installer.
   If the test suite already passed in `npm run release:local` and only fails from machine load, `PI_RELEASE_SKIP_TESTS=1` skips `./test.sh` in the release script; checks and the packed install check still run. Use it only with the user's approval.

   Use `npm_config_min_release_age=0` only for the release command. The repo's normal npm age gate can otherwise block the release lockfile refresh when the current workspace package version was published recently.

   The release script pins the newest published model catalog for Nix (`nix/model-catalog.json`) and builds the checkout with it as the Nix build does (it fails if that catalog predates main's model changes; publish one first), bumps all package versions, updates changelogs, regenerates release artifacts, runs `npm run check`, commits `Release vX.Y.Z`, tags `vX.Y.Z`, adds fresh `## [Unreleased]` changelog sections, commits `Add [Unreleased] section for next cycle`, then pushes `main` and the tag. Do not rerun the release script after a tag was pushed.

4. **CI builds the fork release and publishes GitHub Packages**: pushing the `vX.Y.Z` tag triggers `.github/workflows/build-binaries.yml`. It builds and verifies the binary assets, stages a draft GitHub Release, publishes it, and dispatches `.github/workflows/publish-github-packages.yml` at that tag. The package workflow builds, checks, tests, and publishes the fork packages to GitHub Packages. This fork does not publish upstream npm packages or pi.dev release announcements.

5. **CI builds the Nix release**: the tag push also triggers `.github/workflows/nix.yml`, which builds the tagged flake on Linux and macOS. The workflow does not write branches or catalog pins back to this fork. `nix run .` runs the local fork checkout. If a tagged Nix build fails, the tag cannot be fixed, so ship the fix in the next release.

6. **If release or package publication fails**: inspect the failed job. Before publishing the GitHub Release, the binary workflow cleans up a failed draft. After the release is public, do not rerun the binary workflow against that tag; rerun the failed GitHub Packages job or dispatch its workflow at the same tag instead. Do not rerun `npm run release:patch` or `npm run release:minor` for the same version.
