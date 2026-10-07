# Releases

Repository: https://github.com/wir-drei-digital/mailtriage

Every branch push and pull request runs the shared build workflow on Linux
amd64, Linux arm64 and macOS arm64. Each job checks formatting, runs clippy and
tests, builds with Cargo.lock, smoke-tests the executable, and uploads a
versioned archive with a SHA-256 checksum as a workflow artifact.

## Every stable release is a deployment

Installed copies with `updates: auto`, the default, install every published
stable release within about a day, and their background services switch to it
between passes (see [Updates](guide.md#updates)). Before you push a `vX.Y.Z`
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
  [Rolling back by hand](guide.md#rolling-back-by-hand)).

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
line does not move Latest back. Releases publish one at a time: the publish
jobs of all tags share one concurrency group, and a later one waits for the
running one. GitHub keeps only one waiting run per group, so when you push
several tags at once, a waiting publish can be cancelled by a newer one; rerun
it as in [Retry a failed release](#retry-a-failed-release).

Each release contains:

- `mailtriage-vVERSION-linux-amd64.tar.gz`
- `mailtriage-vVERSION-linux-arm64.tar.gz`
- `mailtriage-vVERSION-macos-arm64.tar.gz`
- `mailtriage-tray-vVERSION-linux-amd64.tar.gz`
- `mailtriage-tray-vVERSION-linux-arm64.tar.gz`
- `mailtriage-tray-vVERSION-macos-arm64.tar.gz`
- A `.sha256` file for each archive, and a combined `SHA256SUMS` that lists
  all six archives.

The `mailtriage` archives contain the executable, README and license, and stay
free of GUI libraries. The `mailtriage-tray` archives contain `mailtriage-tray`
and the license. `mailtriage update` installs a tray archive over a
`mailtriage-tray` next to the CLI (see [Updates](guide.md#updates)). Linux
builds use Ubuntu 24.04's GNU libc environment; the macOS executables are
unsigned and not notarized. Validate the Linux build in the target Hermes
image before rollout.

The workflow creates a draft while uploading files, then publishes it only
after assets pass checksum verification. Published release files cannot be
overwritten by a rerun. Fixes require a new version/tag.

## Retry a failed release

Rerun the failed workflow, or dispatch **Release** from GitHub Actions with an
existing tag. A pre-existing draft can be completed. The workflow never creates
a missing tag or bypasses version, ancestry, test or build checks.

No additional secret is required: only the publish job receives
`contents: write` through GitHub's built-in token. Model API keys and mailbox
credentials are not used in CI or releases.

The repository is public, so installed copies read releases without a token.
To verify downloads by hand:

```sh
sha256sum --check SHA256SUMS
# macOS alternative:
shasum -a 256 --check SHA256SUMS
```
