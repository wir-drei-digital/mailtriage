# Classifier and models

To sort your mail, mailtriage asks a classifier three questions about each message. You choose which one:

| Classifier | Good for | API key |
| --- | --- | --- |
| OpenRouter with Jev | The most accurate results. Message text goes to OpenRouter. | Needed |
| [Local Kev](./local-kev.md) | Keeping mail on an Apple Silicon Mac. | None |
| [Your own Kev server](#run-your-own-kev-server) | Keeping mail on your own hardware when your Mac cannot run Kev, for example on a Linux machine with a GPU. | Only for a server on another machine |

Setup asks which one you want, and `mailtriage setup --update --account work` changes it later. The OpenRouter default model is `typesafe/jev-latest`. The `fake` provider is for offline testing and needs no key.

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

## Run your own Kev server

You can run Kev on a machine you control, such as a Linux server with a GPU, and send mail there for classification. Its System One API answers the same questions as OpenRouter's Jev.

On that machine, with Python 3.12 or 3.13 and [uv](https://docs.astral.sh/uv/):

```sh
git clone --branch kev-1.0 https://github.com/jaredpalmer/kev.git && cd kev
uv sync --extra serve
uv run --extra serve python -m kev.serve --run jaredpalmer/kev-4b@v1.0 --host 127.0.0.1 --port 8009
```

Kev uses CUDA or ROCm when available. `--run jaredpalmer/kev-9b@v1.0` serves the larger Kev-9B; [Kev's README](https://github.com/jaredpalmer/kev/blob/kev-1.0/README.md#models) describes its hardware needs.

::: warning Important
A server on another machine needs HTTPS and a key so your message text and key are encrypted in transit. mailtriage accepts `http://` only for `127.0.0.1`, `localhost` and `[::1]`. It does not set up the HTTPS proxy or Kev service for you.
:::

For a server on another machine:

1. Set `KEV_API_KEY` in Kev's service environment, using your service manager's secret settings. Kev then requires that key for `/v1/*`. Generate a key with `openssl rand -hex 24` and keep it in your password manager.
2. Put an HTTPS proxy, such as Caddy or nginx, in front of Kev, forwarding to `127.0.0.1:8009`.
3. On your Mac, run setup with the server URL. Setup asks where to keep the key, as it does for OpenRouter, and stores it under the name `kev`:

```sh
mailtriage setup --update --account work --provider kev --kev-url https://kev.example.com/v1/systemone
```

Setup checks that the server answers and serves `kev-latest` (or your `--model` value). For a Kev server on the same machine as mailtriage, use `http://127.0.0.1:8009/v1/systemone`; it needs no key.

If the server is unreachable, classification waits and your mail stays queued without using a retry attempt. An HTTP error or invalid classification answer uses an attempt.

::: warning Important
When you switch your server to another checkpoint while keeping its URL and `kev-latest` name, mailtriage cannot detect the change. Run `mailtriage reclassify --account work` to classify open mail again with the new model.
:::

## Detailed reference

- <span id="key-command-rules"></span>[Key command rules](../reference/provider.md#key-command-rules)
