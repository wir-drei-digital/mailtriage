# Docs site

Date: 2026-10-09
Status: Approved design, not implemented. This work moves the specs and plans,
this file included, to `design/` (see [Layout](#layout)).
Related: [Provider adapter](2026-10-09-provider-adapter-design.md). Whichever
of the two lands second carries the adapter's docs: into `docs/guide.md` if the
site is not there yet, otherwise into `guide/provider.md` and
`development/providers.md`.

## Goal

mailtriage gets a documentation site built with [VitePress](https://vitepress.dev/)
and hosted on GitHub Pages from this repository. It serves three readers:
people who use mailtriage, agents that drive it, and developers who work on it.
The site's Markdown becomes the one home of the user docs. The pages are
written in one voice, adapted from the wirdrei.digital voice guide.

## Decisions

| Topic | Decision |
| --- | --- |
| Generator | VitePress, default theme. |
| Tooling | [Bun](https://bun.sh/): `bun install`, a committed `bun.lock`, `bun run` scripts. VitePress runs on Bun's runtime (`--bun`); if the build fails there, the scripts run it on Node and Bun stays the package manager. |
| Address | `https://wir-drei-digital.github.io/mailtriage/` (`base: '/mailtriage/'`). No custom domain yet. |
| Publishing | Every push to `main` that touches the site; pull requests build without publishing. |
| Sections | Guide (users), Agents, Development. |
| Source of truth | The site's Markdown. `docs/guide.md` and `docs/hermes.md` go; the README links to the site. |
| Specs and plans | Leave `docs/` for a top-level `design/` folder, off the site. |
| Content | Moved and edited in the [voice](#voice). Every fact (command, flag, default, JSON field, exit code) stays as it is. |
| Main vs release | The site follows `main`. The home page says so: a feature may be newer than the latest release. |

## Layout

```
docs/                      VitePress root; nothing else lives here
  index.md                 home
  guide/
    introduction.md        What mailtriage does and changes; Try it offline
    install.md             script, Homebrew, from source, self install, uninstall
    setup.md               guided setup: steps, key stores, prompts, updating, output, exit codes
    manual-setup.md
    configuration.md       config location, complete configuration
    provider.md            provider kinds, model, the OpenRouter key
    service.md             background service
    updates.md
    himalaya.md            tested versions, folder names, private Himalaya, Homebrew's Himalaya
    daily-use.md           sync and watch, query and correct
    categories.md
    filing.md              filing, reply queue, refile, safety rules, exit codes
    provider-check.md      the live check before `live`
    tray.md
    reference.md           output and exit codes, binding, state and backups, limits
  agents/
    index.md               the agent guide (today's docs/hermes.md)
  development/
    index.md               the repository in brief; a link to design/ on GitHub
    service-api.md         today's docs/service-api.md
    providers.md           adding a provider; contract evidence (docs/provider-contract.md)
    releases.md            today's docs/releases.md
    verification.md        today's docs/verification.md, without the live provider check
  .vitepress/config.mts
  package.json
  bun.lock
design/                    top level, not published
  specs/                   from docs/superpowers/specs
  plans/                   from docs/superpowers/plans
  history/                 docs/design.md, docs/implementation-plan.md, docs/review-*.md
```

Where today's content goes:

| Today | Site page |
| --- | --- |
| `docs/guide.md`, each `##` section | the guide page named in the layout |
| guide "Provider check" and verification "Live provider check" | `guide/provider-check.md`, merged |
| guide "Tray" (install, login item, menu, categories window, Linux, problems) | `guide/tray.md` |
| guide "Reference" | `guide/reference.md` |
| `docs/hermes.md` | `agents/index.md`, titled "Agent guide" |
| `docs/service-api.md`, `docs/releases.md`, `docs/verification.md` | `development/` |
| `docs/provider-contract.md` | `development/providers.md`, with the provider adapter's "adding a provider" |

New prose is limited to the home page, the three section start pages and the
sentence or two a split page needs to open. The home page opens with the
problem (a full inbox), says what mailtriage does, then that a model (Jev,
through OpenRouter) decides the category, then shows the install one-liner and
a "Get started" link to `guide/introduction`.

## Voice

Adapted for English documentation from the wirdrei.digital voice guide
(`workspace/06_marketing/content/docs/voice.md`):

- **Written by a person.** It sounds like someone explaining the tool: plain
  words, contractions allowed, no corporate phrasing ("seamlessly",
  "leverage", "robust solution").
- **"You" for the reader.** "We" is the people behind mailtriage at
  wirdrei.digital, used only where a sentence is about the team ("We test
  against Himalaya 2.1.0 and 2.2.1"). No marketing "we".
- **Problem, then solution, then term.** A page opens with what the reader
  wants to get done. "AI" names the classifier for orientation, never as the
  hook.
- **Honest about limits, at the feature.** A limitation sits in an
  `::: warning Important` callout next to the feature it concerns, never at the
  end of a page.
- **Benefit, then a concrete case.** What a feature does for the reader, then
  an everyday example, then the exact rules and tables.
- **Short paragraphs**, one to three lines. A bullet with an explanation
  reads `**Name**: what it does for you`.
- **No em dashes.** Start a new sentence instead; a plain hyphen where one is
  needed.
- **Calm and warm.** Exclamation marks and emojis are rare; at most one on the
  home page.
- **Names and spelling.** The company is "wirdrei.digital", lower case, one
  word. US spelling, as the current guide uses.
- **Privacy is said plainly.** Mail stays on the reader's server and
  mailtriage runs on their machine; the pages also say what leaves it (message
  text goes to the classifier).

The voice applies to the guide, agent and home pages in full. The
development pages get only the light rules (no em dashes, "you", short
paragraphs); their technical content stays.

## Site configuration

`docs/.vitepress/config.mts`:

- `title: 'mailtriage'`, a one-line `description`, `base: '/mailtriage/'`,
  `cleanUrls: true`.
- Nav: Guide, Agents, Development, and a GitHub link. Each section has its own
  sidebar, in the layout's order.
- `search: { provider: 'local' }`.
- `editLink` to `https://github.com/wir-drei-digital/mailtriage/edit/main/docs/:path`.
- `outline: [2, 3]`.
- Footer: "mailtriage by wirdrei.digital".
- Dead internal links fail the build (VitePress's default; not switched off).
- Theme: the default theme. `.vitepress/theme/` only sets the brand accent
  (`--vp-c-brand-1` and its shades) to `#E8541A`.

`docs/package.json`: `vitepress` pinned to the newest stable 1.x release that
is at least two weeks old; scripts `dev`, `build` and `preview`.
`.gitignore` gains `docs/node_modules/`, `docs/.vitepress/cache/` and
`docs/.vitepress/dist/`.

## Publishing

`.github/workflows/docs.yml`:

- **Triggers**: `push` to `main` with paths `docs/**` and the workflow file;
  `pull_request` with the same paths; `workflow_dispatch`.
- **build** job (every trigger): checkout, `oven-sh/setup-bun` with a pinned
  Bun version, `bun install --frozen-lockfile` in `docs/`, the em-dash check,
  `bun run build`, then `actions/upload-pages-artifact` with
  `docs/.vitepress/dist` (push and dispatch only).
- **deploy** job (push to `main` and dispatch only): `actions/deploy-pages`
  into the `github-pages` environment; permissions `pages: write` and
  `id-token: write`; `concurrency: pages` without cancelling a running deploy.
- Actions are pinned to exact versions, like the other workflows.
- **Em-dash check**: the build fails when a `.md` file under `docs/` contains
  `—`.
- **One-time setting**: GitHub Pages with source "GitHub Actions". It is done
  with `gh api` after the user's go-ahead, or by hand.

## Links that move

- **README**: keeps its short form; its links point to site pages
  (for example `…/mailtriage/guide/filing#reply-queue`). Its doc list names
  the site and its three sections.
- **Setup's go-live message** ("Go live only after the provider checklist
  (docs/verification.md)") names the provider check page's URL. Tests that pin
  the text change with it.
- **Maintainer instructions** in `himalaya-compat.yml` and
  `scripts/add-himalaya-version.sh` name the new page files that list the
  tested Himalaya versions. The comment in `install-check.yml` points to
  `docs/development/releases.md`.
- **Specs, plans and code comments** that cite `docs/superpowers/...` or the
  moved docs files cite their new paths.
- Relative links inside moved pages are fixed; a dead one fails the build.

## Testing

- `bun run build` passes: no dead links.
- No `—` in `docs/**/*.md`.
- **Fact check.** A reviewer compares every site page with the section it came
  from in `docs/guide.md`, `docs/hermes.md` and the development files at the
  commit before the move. Every command, flag, default, config key, JSON field
  and exit code is still there and unchanged; only order and wording differ.
  Nothing is lost.
- **Voice check.** The reviewer checks the guide, agent and home pages
  against the [voice](#voice) rules.
- `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` pass.
- **After the first deploy**: the live site's home page, search, a deep link
  with an anchor, a section sidebar, and the 404 page work.

## Out of scope

- A custom domain.
- Docs per release version.
- Translations.
- rustdoc API pages.
- A custom theme beyond the accent color and footer.
