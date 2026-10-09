# Releases

Repository: https://github.com/wir-drei-digital/mailtriage

Every branch push and pull request runs the shared build workflow on Linux
amd64, Linux arm64 and macOS arm64. Each job checks formatting, runs clippy and
tests, builds with Cargo.lock, smoke-tests the executable, and uploads a
versioned archive with a SHA-256 checksum as a workflow artifact.

## Every stable release is a deployment

Installed copies with `updates: auto`, the default, install every published
stable release within about a day, and their background services switch to it
between passes (see [Updates](../guide/updates.md)). Before you push a `vX.Y.Z`
tag:

- Try the build on one machine first with a release candidate tag,
  `vX.Y.Z-rc.N`. It is published as a prerelease, which mailtriage never
  installs automatically; install it there by hand from its archive.
- Every database migration must keep the previous release's running processes
  correct: only new tables, and new columns that are nullable or have
  defaults; no renames, no drops, no changed meanings. During an update,
  processes of the old and the new release use one state database at the same
  time. A release whose migration cannot follow this rule must not be
  published as a stable release.
- A bad release is fixed by a newer one; there is no rollback command (see
  [Rolling back by hand](../guide/updates.md#rolling-back-by-hand)).

## Tested Himalaya versions

`src/engine/himalaya-versions.json` lists the Himalaya versions mailtriage
accepts, with the SHA-256 of each one's release archives; every release
compiles it in. The Dovecot end-to-end workflow (`e2e.yml`) reads it and runs
once per listed version, so the workflow never changes when a version is added.

**The weekly check.** `.github/workflows/himalaya-compat.yml` runs every
Monday and on demand, in three jobs:

1. `test` (read-only) reads pimalaya/himalaya's newest stable release and
   stops when it is listed. Otherwise it adds it with
   `scripts/add-himalaya-version.sh` and runs the Dovecot suite against it in
   both namespace layouts. The candidate binary runs only in this job.
2. `propose`, when the suite passed: pushes the edited file to the branch
   `himalaya/VERSION`, opens the pull request `Test Himalaya VERSION`, and
   starts `ci.yml` and `e2e.yml` on the branch (a branch pushed with the
   workflow's token starts no workflow by itself). When a pull request from
   that branch exists already, open or decided, it does nothing; a branch
   left without one (an earlier run failed before opening it) is replaced.
3. `report`, when the suite failed: opens the issue `Himalaya VERSION fails the
   end-to-end suite`, or comments on the open one, with the end of the
   failing log.

Before merging such a pull request, read the new version's `--mailbox`
resolver in Himalaya's source for role changes: the script copies `roles` from
the previous version.

Also update the places in `README.md` and the guide pages
(`docs/guide/himalaya.md`, `setup.md`, `manual-setup.md` and
`configuration.md`) that name the tested versions or the newest one. Push these
edits to the pull request's branch, never to `main` directly. The tests derive
the tested versions from the data file, so a valid new entry needs no test
change.

The workflow needs one repository setting, made once:
Settings → Actions → General → Workflow permissions → "Allow GitHub Actions
to create and approve pull requests".

**Adding a version by hand,** on a new branch:

```sh
git switch -c himalaya/2.3.0
scripts/add-himalaya-version.sh 2.3.0
```

It reads the release's asset digests from the GitHub API (set `GH_TOKEN` to
avoid the anonymous rate limit), fails without changing anything when a
platform's digest is missing, and appends the entry with the previous
version's `roles`. Then:

1. Check the roles against the new version's `--mailbox` resolver; correct
   `roles` and the tests that pin them (`src/engine/versions.rs`,
   `src/engine/targets.rs`) if it changed.
2. Update the places in `README.md` and the guide pages
   (`docs/guide/himalaya.md`, `setup.md`, `manual-setup.md` and
   `configuration.md`) that name the tested versions or the newest one.
3. Push the branch and open a pull request; never push to `main` directly.
   The pull request runs CI and `e2e.yml`, which runs the suite for every
   listed version.

## Publish a version

1. Update the package version in `Cargo.toml` and in `tray/Cargo.toml` (the
   same version; CI refuses a mismatch) and regenerate `Cargo.lock`.
2. Commit the version change and push it to `main`; wait for CI to pass.
3. Create and push the matching annotated tag:

   ```sh
   git tag -a v0.1.0 -m 'Release v0.1.0'
   git push origin v0.1.0
   ```

Use the actual package version in place of the example. Pushing the tag
publishes a GitHub Release after all three native builds and their tests pass.
The tag must point to a commit on `main` and exactly match the package version.

Tags such as `v0.2.0-rc.1` become prereleases: they are never marked Latest and
never installed automatically. A stable tag is marked Latest only when it is
higher than every published stable release, so publishing a fix for an older
line does not move Latest back.

Releases publish one at a time: the publish jobs of all tags share one
concurrency group, and a later one waits for the running one. GitHub keeps only
one waiting run per group, so when you push several tags at once, a waiting
publish can be canceled by a newer one; rerun it as in
[Retry a failed release](#retry-a-failed-release).

Each release contains:

- `mailtriage-vVERSION-linux-amd64.tar.gz`
- `mailtriage-vVERSION-linux-arm64.tar.gz`
- `mailtriage-vVERSION-macos-arm64.tar.gz`
- `mailtriage-tray-vVERSION-linux-amd64.tar.gz`
- `mailtriage-tray-vVERSION-linux-arm64.tar.gz`
- `mailtriage-tray-vVERSION-macos-arm64.tar.gz`
- A `.sha256` file for each archive, and a combined `SHA256SUMS` that lists
  all six archives.
- `install.sh`, the install script, with this release as its default version.

The build checks that every archive holds exactly its files at the top level
(`mailtriage`, `LICENSE`, `README.md`; the tray's `mailtriage-tray`,
`LICENSE`), because the install script refuses anything else. macOS builds
pack them with `COPYFILE_DISABLE=1`, so no `._*` metadata entries appear.

After a release, run **Install check** (`install-check.yml`) from GitHub
Actions. It runs the install script from `main` against that release on
Linux amd64, Linux arm64 and macOS with `--yes --no-setup`, then
`mailtriage --version` and `mailtriage himalaya install`.

The `mailtriage` archives contain the executable, README and license, and stay
free of GUI libraries. The `mailtriage-tray` archives contain `mailtriage-tray`
and the license. `mailtriage update` installs a tray archive over a
`mailtriage-tray` next to the CLI (see [Updates](../guide/updates.md)).

Linux builds use Ubuntu 24.04's GNU libc environment; the macOS executables are
unsigned and not notarized. Validate the Linux build in the target Hermes
image before rollout.

The workflow creates a draft while uploading files, then publishes it only
after assets pass checksum verification. Published release files cannot be
overwritten by a rerun. Fixes require a new version/tag.

## Homebrew tap

The release workflow's `homebrew` job updates `Formula/mailtriage.rb` in
[wir-drei-digital/homebrew-tap](https://github.com/wir-drei-digital/homebrew-tap)
after a stable release is published:

1. It runs only for the highest published stable release (the same check as
   the Latest marking), one at a time in the concurrency group
   `homebrew-tap`. GitHub keeps one waiting run per group, so a newer waiting
   run replaces an older one; rerun a replaced one only when it was the
   highest.
2. It renders `packaging/homebrew/mailtriage.rb.in` with
   `packaging/homebrew/render.sh VERSION SHA256SUMS`, which fails when an
   archive is missing from `SHA256SUMS`, and checks the result with `ruby -c`.
3. It checks out the tap with the secret `HOMEBREW_TAP_TOKEN` and runs
   `packaging/homebrew/publish.sh`. When the tap already has a higher
   version, nothing changes; the same version with the same file needs no
   commit; otherwise it commits `mailtriage X.Y.Z` and pushes, and after a
   push conflict it fetches, decides again and retries once.

`HOMEBREW_TAP_TOKEN` is a fine-grained personal access token with
**Contents: Read and write** on `wir-drei-digital/homebrew-tap` only, stored as
an Actions secret of this repository. Without it the job ends with a notice
and the release stays published; set it and rerun the job.

The tap's own workflow installs and tests the formula on macOS and Ubuntu
after every push. Its README and workflow are kept in
`packaging/homebrew/tap/`. CI here renders the template with dummy checksums
on macOS and runs `ruby -c` and `brew style` on it.

## Retry a failed release

Rerun the failed workflow, or dispatch **Release** from GitHub Actions with an
existing tag. A pre-existing draft can be completed. The workflow never creates
a missing tag or bypasses version, ancestry, test or build checks.

Publishing needs no additional secret: only the publish job receives
`contents: write` through GitHub's built-in token. The Homebrew tap needs
`HOMEBREW_TAP_TOKEN` (see [Homebrew tap](#homebrew-tap)). Model API keys and mailbox
credentials are not used in CI or releases.

The repository is public, so installed copies read releases without a token.
To verify downloads by hand:

```sh
sha256sum --check SHA256SUMS
# macOS alternative:
shasum -a 256 --check SHA256SUMS
```
