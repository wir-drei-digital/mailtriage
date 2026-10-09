# API keys and models

mailtriage uses OpenRouter's Decisions API to classify real mail. The default model is `typesafe/jev-latest`. The `fake` provider is for offline testing and needs no key.

## The OpenRouter key

The easiest way to configure your key is [guided setup](./setup.md#key-stores). Choose a key store available on your machine: macOS Keychain, Secret Service or `pass`.

mailtriage saves a command that reads the key. The key itself stays in the store. To change the store later, for example on macOS:

```sh
mailtriage setup --update --account work --key-store keychain
```

For a custom store, use `--key-command 'COMMAND'`, where the command prints the key. It must work without asking for terminal input. See [key command rules](../reference/provider.md#key-command-rules).

When `provider.api_key_command` is set, it is the only key source. Otherwise mailtriage reads the variable named by `provider.api_key_env`. It does not read `.env` files.

## Environment variable

For a temporary session in bash or zsh:

```sh
read -rs OPENROUTER_API_KEY
export OPENROUTER_API_KEY
```

At the first line, paste the key and press Enter; it is not echoed. Your config must use `api_key_env: "OPENROUTER_API_KEY"` without an `api_key_command`.

The variable lasts only for that shell. The background service does not inherit it, so use a key store for unattended operation.

## Check the key

```sh
mailtriage doctor --account work
```

Look for `provider.key_present: true`. If it is false, `provider.key_error` explains why. A locked key store is one possible cause.

Without a key, classification pauses and mail stays queued. Later passes resume when the key works. Listing, reading, correcting and marking mail done still work. `doctor` checks that a key is present; it does not test a request to OpenRouter.

## Change the model

```sh
mailtriage setup --update --account work --model typesafe/jev-latest
```

Use a Decisions model ID. Ordinary chat-model APIs do not provide the decision probabilities mailtriage needs.

The `latest` alias follows OpenRouter's newer Jev models. To keep one specific model, use its fixed ID. Changing the model queues open mail for classification again and keeps the existing key source.

A mistyped model ID is detected only when classification runs. See the [provider reference](../reference/provider.md) for model behavior, key errors and retry details.

## Detailed reference

- <span id="key-command-rules"></span>[Key command rules](../reference/provider.md#key-command-rules)
