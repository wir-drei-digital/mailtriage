# Provider adapter

Date: 2026-10-09
Status: Approved design, not implemented.
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md),
[Guided setup](2026-10-05-guided-setup-design.md)

## Goal

The classifier sits behind a clean interface, so that another
Decisions-style provider can be added without touching setup, `doctor`, the
service files or the policy layer. OpenRouter accepts any Decisions model,
not only Jev. Only OpenRouter and the offline `fake` provider are implemented.

## Decisions

| Topic | Decision |
| --- | --- |
| Shape | A `DecisionProvider` trait with a registry, like the mail engine's `MailEngine` trait. |
| Other providers | Decisions-style services only: they answer the same typed questions with a choice, a confidence and a probability per label. General chat-model APIs are not a target; they would need a policy change (see [Out of scope](#out-of-scope)). |
| Neutral result | A `Decision`. The policy thresholds apply to it, never to a provider's JSON. |
| Config | `provider` keeps its fields and the config schema version. `kind` names a registered provider. |
| Default | `kind: "openrouter"`, `model: "typesafe/jev-latest"` for new setups. Existing configs keep their model. |
| OpenRouter models | Any non-empty model ID. The `typesafe/jev-` rule goes. |
| OpenRouter endpoint | Still pinned to `https://openrouter.ai/api/alpha/decisions`, or the loopback form tests use. The key is sent nowhere else. |
| Compatibility | Byte-identical OpenRouter requests, the same classification generation hash, the same stored classifications, JSON output and exit codes. Only the Jev-only error text goes. |

## Interface

`src/provider.rs` becomes the module `src/provider/`.

### `mod.rs`

```rust
/// What every provider is asked.
pub struct DecisionRequest {
    pub identity: String,
    pub timezone: String,
    pub brief: String,
    pub evaluated_at: String,          // RFC 3339, set once per request
    pub message: NormalizedMessage,
    pub questions: Questions,          // see questions.rs
}

/// One answered choice question.
pub struct Choice {
    pub choice: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

/// A provider's answer, before policy.
pub struct Decision {
    pub model: String,                 // as the response names it
    pub category: Option<Choice>,      // None: the provider gave no answer
    pub urgency: Option<Choice>,
    pub action_required: Option<f64>,  // probability of "yes"
    pub raw: Value,                    // the provider's response, stored for audit
}

pub trait DecisionProvider: Sync {
    /// Provider-specific rules; the shared rules have already passed.
    fn validate(&self, config: &ProviderConfig) -> Result<()>;
    /// The key store's account name (`openrouter`); `None` when the
    /// provider needs no key.
    fn key_account(&self) -> Option<&'static str>;
    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision>;
}

/// Registered kinds, in the order setup offers them.
pub const KINDS: &[&str] = &["openrouter", "fake"];
pub fn provider_for(kind: &str) -> Result<&'static dyn DecisionProvider>;
```

- `validate_configuration(config)` keeps its name and callers. It checks the
  shared rules (timeout 1 to 300 seconds, a non-empty model, a valid
  `api_key_command`/`api_key_env` when the provider needs a key), then
  `provider_for(kind)?.validate(config)`. An unknown kind fails with
  "unsupported provider kind", as today.
- `classify_with_key(config, account, message, policy, key)` keeps its
  signature. It builds the `DecisionRequest`, resolves the key through the
  `KeyCache` only when `key_account()` is `Some`, calls `decide`, checks the
  `Decision` against the contract below, then calls `policy::apply`.
- The module doc explains how to add a provider: implement the trait, add the
  kind to `KINDS` and `provider_for`, and run the contract suite against it.

### `questions.rs`

Builds the three questions from the account: `category` (one criterion per
category, `"{name}: {description}"`), `urgency` (`low`, `medium`, `high`) and
`action_required` (`true`, `false`). Their instructions and criteria are
today's texts, character for character. `rubric_version` in the generation
hash stays 1.

### `openrouter.rs`

- `validate`: the endpoint rule above. No model rule beyond non-empty.
- `key_account`: `Some("openrouter")`, so existing keychain, secret-service
  and `pass` entries keep working.
- `decide`: turns the `DecisionRequest` into today's request body
  (`{model, state, questions}`), sends it with today's client (timeout,
  no redirects, Bearer key) and parses the response:
  - the 1 MiB limit, the `answers` object check and today's error texts stay;
  - a missing answer becomes `None`;
  - an answer of the wrong `type` (`choice`, `noul`) or a non-number where a
    number belongs is an error with today's text;
  - `raw` is the whole response.

### `fake.rs`

The deterministic offline demo, unchanged in behaviour, returning a
`Decision`. `key_account` is `None`.

### The contract check

Every `Decision` is checked in `mod.rs` before policy, whichever provider made
it. These are today's checks in `policy.rs`, moved and kept with their texts:

- `model` is non-empty;
- a choice is one of the offered labels;
- confidence and every probability lie between 0 and 1 and are finite;
- the probabilities cover exactly the offered labels, include the chosen one,
  and total 1 within 0.031 (the Decisions API rounds to two decimals);
- no label is more than 0.01 more probable than the chosen one;
- `action_required` lies between 0 and 1.

A failed check fails the classification as today: the message stays queued
and counts an attempt.

### `policy.rs`

`apply(decision, account, policy, incomplete) -> Classification` replaces
`decode_response`. Thresholds, reasons (`*_missing`, `*_low_confidence`,
`action_required_uncertain`, `input_incomplete`, `review_mode`) and the
`ready`/`uncertain` state are unchanged. `Classification.raw` is
`decision.raw` and `Classification.model` is `decision.model`.

## Behaviour elsewhere

| Place | Today | After |
| --- | --- | --- |
| `doctor` | key check unless `kind == "fake"` | key check when `key_account()` is `Some` |
| `classification_skipped` (`sync`, `watch`) | only when `kind == "openrouter"` | when `key_account()` is `Some` |
| `service install` note on an environment key | `kind == "openrouter"` | `key_account()` is `Some` |
| `setup` classifier choice, `--provider` | hard-coded `openrouter`, `fake` | `KINDS` |
| `setup` model | default `typesafe/jev-1.13`, must start with `typesafe/jev-` | default `typesafe/jev-latest`, any non-empty ID |
| `setup` key steps | for `openrouter` | when `key_account()` is `Some` |
| `secrets` store and read commands | account `openrouter` | the provider's `key_account()` (still `openrouter`) |

`init` keeps writing the `fake` provider with model `fake/offline`.

### `typesafe/jev-latest`

`latest` is an alias OpenRouter moves. The config does not change when it
moves, so the generation hash does not change and classified mail is not
redone. New mail is classified by the newer model, and each classification
records the model the response named. A user who wants a fixed model names
one, such as `typesafe/jev-1.13`; changing `model` reclassifies, as today.

## Errors

| Case | Result |
| --- | --- |
| Unknown `kind` | exit 2, "unsupported provider kind" |
| Empty model | exit 2, "provider.model cannot be empty" |
| OpenRouter rejects the model | the request's HTTP error ("OpenRouter Decisions returned HTTP 4xx"); the message stays queued |
| Contract check fails | today's text; the classification fails and counts an attempt |

## Docs

- Guide: the provider section and config table: the two kinds, any Decisions
  model for `openrouter`, the default `typesafe/jev-latest` and what `latest`
  means; the `--model` line in setup.
- `docs/development/providers.md`, `docs/development/service-api.md` and `docs/development/verification.md`:
  `jev-1.13` as the default becomes `jev-latest`; the contract notes say that
  any Decisions model is accepted.

## Testing

- **Byte-identical request.** A test pins today's OpenRouter request body for
  a fixed account, message and `evaluated_at`, captured before the refactor.
  The refactored adapter must produce it exactly.
- **Generation hash.** The existing pinned-hash tests pass unchanged.
- **Contract suite.** One suite runs against every registered provider:
  `fake` directly, OpenRouter against the loopback HTTP fixture. It checks a
  valid `Decision` (offered labels, probabilities in range, model kept) and,
  for OpenRouter, an HTTP error, an oversized response, missing `answers`, a
  wrong answer type, a missing answer, and a redirect.
- **Contract check.** Each rule above, against hand-built `Decision` values,
  with today's error texts.
- **Policy.** Today's threshold tests, rewritten to take `Decision` values,
  give the same reasons and states.
- **Registry and setup.** An unknown kind is refused; setup and `--provider`
  offer exactly `KINDS`; any non-empty model ID is accepted; a new setup
  writes `typesafe/jev-latest`; existing configs keep their model.
- **Existing tests** keep passing, except those asserting the Jev-only rule,
  which are removed or changed to expect acceptance.

## Out of scope

- Implementing another provider.
- A provider per account.
- Chat-model APIs (Anthropic, OpenAI, Ollama). They give no calibrated
  probabilities, so the policy would need a rule for answers without
  confidence. This interface does not prepare for that.
- Changing the policy thresholds, the questions or the stored classification
  format.
