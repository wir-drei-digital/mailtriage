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
- [Verification](./verification.md): what has been checked, automatically and by hand.
