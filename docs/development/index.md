# Development

These pages are for you if you work on mailtriage itself.

## The repository in brief

- `src/`: the Rust CLI and library.
- `tray/`: the tray app, `mailtriage-tray`.
- `tests/`: the tests of the CLI and library, including the Dovecot end-to-end test.
- `docs/`: this site.
- `design/`: specs, plans and the design history, in [`design/` on GitHub](https://github.com/wir-drei-digital/mailtriage/tree/main/design). They are not part of this site.

## Pages

- [Service API](./service-api.md): the service functions the CLI calls and the JSON they return.
- [Providers](./providers.md): how to add a classification provider, and the evidence behind the OpenRouter and Himalaya contracts.
- [Releases](./releases.md): how a release is built, published and tested, and how a new Himalaya version is added.
- [Verification](./verification.md): release checklists and dated test records.
- [Mail provider tests](./mail-provider-check.md): the checks required before live filing.

## Edit this documentation

Write everyday instructions in `docs/guide/` and exact command behavior in `docs/reference/`. Keep the guide focused on the task, and link to the reference for flags, JSON fields and edge cases. Preserve existing page URLs and heading anchors when reorganizing content.

Build the site from the repository root:

```sh
cd docs
bun install --frozen-lockfile
bun run build
```

The build checks page links and heading anchors. Use `bun run dev` for a local preview. GitHub Pages publishes changes to `docs/` when they reach `main`; pull requests build the site without publishing.
