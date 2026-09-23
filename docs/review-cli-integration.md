# Independent CLI and integration review

2026-09-23. Reviewed the integrated service, store, provider, normalization, policy, Himalaya adapter, CLI, and the design contract. Reproductions below used the local debug binary and temporary directories; no mailbox or provider credential was used. Findings were sent to the coordinator for fixes in owned modules.

## Verified findings and disposition

1. **P1 — Mailbox identity can change behind a stable Himalaya config path. Fixed and reverified.** `Service::ensure` originally bound a namespace to `account.identity`, the selected Himalaya account name, and the config *path*, but did not inspect the source's server/login identity. I classified a mail with a dummy `himalaya.toml` at one path, changed that file from mailbox A to mailbox B, then ran `list --account work --view all`. It exited 0 and returned mailbox A's stored mail. The coordinator added a locally parsed IMAP binding fingerprint with credential values scrubbed. A fresh reproduction changing `imap.server` now exits 5. Opaque token broker principals still rely on the configured account identity.

2. **P1 — Semantic revisions automatically reclassify Done messages. Fixed and reverified.** I classified a message, marked it Done, edited a category description, and applied the taxonomy. Initially `list --view all` showed the Done row as `pending`; a later `sync` reported one new classification. Automatic generation requeue is now limited to open rows. A fresh reproduction leaves the Done row classified and `sync` reports `classified: 0`; reopening a stale Done row requeues it.

3. **P2 — A label rename leaves an old pagination cursor valid while the listing changes. Fixed and reverified.** With two messages, I took page 1 at revision 5, renamed `correspondence`, then requested page 2 with the old cursor. Initially it exited 0 at revision 5 and showed the new category label. The cursor now includes the loaded config hash. A fresh reproduction rejects the old cursor with exit 5.

4. **P2 — Missing source occurrences suppress fetch-failure Attention. Fixed by code inspection.** In `Service::item`, `source_present == Some(false)` originally replaced all attention reasons with an empty vector, including `pending` and `failed`. `reconcile_range` can remove the last occurrence of a still-unfetched message and terminate its job. The coordinator restricted suppression to `ready` rows. Pending and failed envelopes now continue through the attention policy; this path has not been rerun against a synthetic disappearing mailbox.

5. **P2 — Sync summary omits required counts. Open at review time.** `Service::sync` returns discovered/classified/cached/failed/scan_errors and coverage, but no fetched or pending count. The design calls for both in the summary, so an operator cannot distinguish a large fetch backlog from a completed pass without a separate query. Count newly fetched bodies and queued/retry work, including fetch failures, in the response.

6. **P2 — Read has no source locator provenance. Open at review time.** `Service::read` returns the normalized content and raw hash, but `item()` does not expose mailbox, UIDVALIDITY, or UID. For a Himalaya message, consumers cannot tell which source occurrence supplied it or whether an alias exists. Add a safe provenance array to `read` (and export), without exposing credentials or raw command output.

## CLI and delivery verification

The CLI round-trip integration test passes offline: init, doctor, classify, list, read, correct, Done/reopen, export, category export/validate/apply, and reclassify dry run. Invalid CLI flags emit a versioned JSON error with exit 2. The release workflow uses native GitHub-hosted runner labels for Linux x64/arm64 and macOS arm64; [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) lists these labels. Workflow artifacts are configured, not yet produced by a GitHub run. Real OpenRouter, IMAP Seen/UID behavior, and the headless Hermes runtime remain live gates.

## Coordinator closure

All six findings are addressed in the delivered implementation. Source absence has a persistent regression test; sync now reports `fetched` and `pending`; read and export include safe `source_occurrences` provenance. `tests/sync.rs` checks those fields through the full fake-Himalaya pipeline, including reconciliation and epoch reset.
