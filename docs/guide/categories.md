# Categories

Your categories are the list mailtriage chooses from for every message. When your mail changes, change the list: export it, edit the file, check it, and apply it.

```sh
mailtriage categories export --account work --json > categories.json
mailtriage categories validate --file categories.json --account work --json
mailtriage categories apply --account work --file categories.json --json
mailtriage reclassify --account work --dry-run --json
mailtriage reclassify --account work --since 2026-09-01 --json
```

A category file holds a JSON array of categories or an object with a `categories` array; `categories export` writes the object form.

The [category fields](./configuration.md#a-complete-configuration) are the same as in `mailtriage.json`: a unique `id`, a non-empty `name` and `description`, optional `examples` and `folder`, and exactly one category with `catch_all: true`.

`categories validate --file FILE` checks the file alone. With `--account NAME` it checks the file against that account's configuration, including the folder rules when the account's filing is on. Neither form writes anything.

`categories apply` writes the categories into `mailtriage.json`. What happens next depends on what you changed. Say you only want a clearer name for a category: a rename (a change to `name` only) keeps the ID and does not reclassify.

A change to a category's `id`, `description`, `examples` or `catch_all`, or an added or removed category, advances `taxonomy_revision` and queues all open messages for classification. Done messages stay done; a reopened message is classified under the current categories.

Removing a category that a manual correction uses fails with `category has manual corrections; remap or clear them before removal` (exit code 2); correct those messages to another category or clear the correction first. Keep IDs when you edit category files by hand.

`reclassify` queues the matching stored messages for classification and processes up to `--limit` of them (1 to 500, default 100); later passes process the rest.

`--since YYYY-MM-DD` selects messages first observed on or after that local date. `--dry-run` reports `matched` and `will_process` and changes nothing.

## Checking a change before you apply it

A category file can go stale while you edit it, for example when another window or an agent applies a change meanwhile. The digest from the export makes `apply` refuse instead of overwriting that change:

```sh
mailtriage categories export --account work --json > categories.json
# edit categories.json
mailtriage categories validate --file categories.json --account work --json
mailtriage categories apply --account work --file categories.json --expect-digest "$(jq -r .digest categories.json)" --json
```

- `categories export` and `categories validate --account` report `digest`: `v1:` and a SHA-256 of the account's categories and whether its filing is on. It changes whenever something changes how `apply` would write the categories. The key order of a file and `"folder": null` versus no `folder` do not change it; the category order does.
- `categories apply --expect-digest DIGEST` writes nothing and exits 5 with `categories changed since export; export again` (reason `categories_changed`) when the account's categories changed since the export (another window or agent applied a change), or filing was turned on or off. Export again and redo the edit.
- `categories validate --file FILE --account NAME` also reports `changes`, computed after the same folder rules `apply` uses:
  - `added` lists category IDs;
  - `removed` lists `{"id","folder"}`;
  - `renamed` and `folders_changed` list `{"id","from","to"}`;
  - `edited` lists categories whose description, examples or default flag changed;
  - `reclassifies` is `true` exactly when `apply` would sort all open mail again (it advances `taxonomy_revision`, which uses your OpenRouter key).

  Folder names in `changes` are for display.
