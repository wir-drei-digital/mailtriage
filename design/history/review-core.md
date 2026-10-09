# Domain core and service/store specification review

2026-09-23. This pass checked the implementation against `docs/design.md` and
`docs/implementation-plan.md`, with emphasis on account isolation, durable UID
progress, stale results, manual overlays, category edits, and Attention.

## Verified findings and resolution

1. **Concurrent corrections could lose a manual edit.** Two `Store` handles
   read the same empty overrides. One wrote `urgency=high`; the other then wrote
   `category_id=correspondence` from its stale copy. The final record contained
   only the category. A temporary executable test reproduced this before the
   fix. `Store::update_overrides` now updates only when the stored JSON matches
   the caller's snapshot; `Service::correct` reports a conflict so the caller
   can retry. The fix also makes a category edit and a correction share the
   configuration lock, closing the gap between the category removal check and
   the config save.

2. **Reconciliation briefly hid an unresolved message.** A staged, unfetched
   envelope appeared in Attention. After `reconcile_range` removed its vanished
   source UID, a temporary executable test found Attention empty although the
   record was still `pending`. `Service::item` now keeps pending, failed and
   uncertain rows in Attention even when the source is absent. Reconciliation
   marks an unfetched, orphaned row failed with a specific error and terminates
   its job. A ready message no longer in a watched source can leave Attention
   while its locally retained content remains readable.

3. **Occurrence reconciliation was absent.** Initially, UID presence was only
   added during discovery and removed on UIDVALIDITY reset. The service now
   performs bounded UID range reconciliation on sync and the store removes
   vanished occurrences transactionally. A failed reconciliation marks scan
   coverage incomplete; it does not erase previously indexed content.

4. **Policy state and Attention needed alignment.** Valid classifications now
   use `ready`, matching the service and design. Medium urgency is an Attention
   signal. Pending, failed and uncertain states remain visible. A confident
   low urgency, no-action result is quiet only with review mode off; review mode
   and incomplete input force a reviewable state. The core tests cover these
   cases, along with unknown categories, invalid probabilities and Noul
   uncertainty.

5. **Provider validation was duplicated.** `config::validate` now calls
   `provider::validate_configuration`, so a saved config cannot pass a looser
   endpoint/model rule than the classifier applies.

## Evidence and limits

- `cargo test --test core --offline`: 7 passed.
- `cargo clippy --lib --offline -- -D warnings`: passed after the fixes above.
- Temporary reproductions for findings 1 and 2 ran outside the delivery repo
  in `/private/tmp/mailtriage-core-harness`. After the fixes, the same scenarios
  verified that a stale correction is rejected and an orphaned unfetched item
  stays in Attention with `failed` state. The integrated regression tests belong
  in `tests/storage.rs`.
- This was an offline review. Real OpenRouter behavior, IMAP Seen/epoch
  behavior, and the headless Hermes deployment remain the live gates listed in
  `docs/implementation-plan.md`.
