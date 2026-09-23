# Releases

Repository: https://github.com/wir-drei-digital/mailtriage

Every branch push and pull request runs the shared build workflow on Linux
amd64, Linux arm64 and macOS arm64. Each job checks formatting, runs clippy and
tests, builds with Cargo.lock, smoke-tests the executable, and uploads a
versioned archive with a SHA-256 checksum as a workflow artifact.

## Publish a version

1. Update the package version in `Cargo.toml` and regenerate `Cargo.lock`.
2. Commit the version change and push it to `main`; wait for CI to pass.
3. Create and push the matching annotated tag:

   ```sh
   git tag -a v0.1.0 -m 'Release v0.1.0'
   git push origin v0.1.0
   ```

Use the actual package version in place of the example. Pushing the tag
publishes a GitHub Release after all three native builds and their tests pass.
The tag must point to a commit on `main` and exactly match the package version.
Tags such as `v0.2.0-rc.1` become prereleases and do not replace Latest.

Each release contains:

- `mailtriage-vVERSION-linux-amd64.tar.gz`
- `mailtriage-vVERSION-linux-arm64.tar.gz`
- `mailtriage-vVERSION-macos-arm64.tar.gz`
- A `.sha256` file for each archive, and a combined `SHA256SUMS`.

Archives contain the executable, README and license. Linux builds use Ubuntu
24.04's GNU libc environment; the macOS executable is unsigned and not notarized.
Validate the Linux build in the target Hermes image before rollout.

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

Download artifacts and releases through authenticated GitHub access while the
repository is private. To verify downloads:

```sh
sha256sum --check SHA256SUMS
# macOS alternative:
shasum -a 256 --check SHA256SUMS
```
