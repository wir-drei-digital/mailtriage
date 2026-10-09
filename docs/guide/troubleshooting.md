# Troubleshooting

Start by checking the account from the same environment where mailtriage runs:

```sh
mailtriage doctor --account work
mailtriage service status --account work
```

Replace `work` with your account name. Read the output even if the command succeeds: `doctor` exits 0 when a readiness check reports a problem.

## Mail is not appearing

If the service is installed, look for `running: true` and a recent `last_pass.finished_at`. Start a stopped service with:

```sh
mailtriage service start --account work
```

Without a service, run `mailtriage sync --account work --limit 20`.

The first passes scan existing mail in batches, so a large inbox can take time. `mailtriage list --account work --view all` shows everything already stored, including messages outside the attention list. A partial scan does not mean the rest of the mailbox is empty.

## Classification was skipped

Check `provider.key_present` and `provider.key_error` in the `doctor` result. Common causes are a locked key store or a key set only in your terminal's environment.

Use [a key store](./provider.md#the-openrouter-key) for the background service. Once the key works, later passes classify the queued mail without losing retry attempts from the skipped passes.

If the key is present but every classification fails, check the model ID in `provider.model`. `doctor` does not validate that ID with OpenRouter.

## Some mail was skipped

Exit code 4 means the result is partial. Check:

| Field in a pass result | Meaning |
| --- | --- |
| `scan_errors` | Some watched folders could not be scanned. See `coverage.scans[].error`. |
| `failed` | Some messages could not be fetched or classified. `list --view all` shows their errors. |
| `classification.skipped` | The key was unavailable. |
| `filing.errors` | Some filing actions failed. Check `filing status` and `filing log`. |

Later passes retry failed work within the configured limits. For IMAP errors, use the [Himalaya login checks](./manual-setup.md#_1-set-up-himalaya) to see the server's error, which mailtriage does not print.

## Another worker is already running

Use one worker per account. If the background service is running, use `list` and `read` to inspect its results instead of starting `sync` or another `watch`.

To make a manual pass, stop the service first:

```sh
mailtriage service stop --account work
mailtriage sync --account work --limit 20
mailtriage service start --account work
```

## Himalaya is missing or untested

Install a tested private copy:

```sh
mailtriage himalaya install
```

Then point your account at the path it prints:

```sh
mailtriage setup --update --account work --himalaya-binary PATH
```

See [Himalaya versions](./himalaya.md) if a package-manager upgrade caused this.

## Everything appears in the attention list

This is expected while `policy.review_mode` is `true`, the default. Review the decisions before turning it off in the [configuration](./configuration.md#review-mode-and-classification-costs).

## A service runs another configuration

`service status` reports `service_config` and `config_matches`. If the service belongs to another file, use that file's `--config PATH` to manage it. Run `service install` with a different config only when you intend to switch the service to it.

For configuration lookup rules, see [configuration](./configuration.md#where-mailtriage-finds-the-config).

## Filing is blocked or a folder is paused

```sh
mailtriage filing status --account work
mailtriage filing log --account work --id MESSAGE_ID
```

Check the message in your mail client before lifting a block. Follow the [recovery table](../reference/filing.md#lifting-blocks-and-pauses) for the reported reason. Repeatedly retrying without resolving the cause can repeat the problem.

## The account binding changed

An account name is tied to its mailbox identity, including its address and non-secret IMAP settings. Restore the intended settings, or set up the different mailbox under a new name. See [account binding](./reference.md#account-binding).

For other errors, use the [exit-code reference](./reference.md#output-and-exit-codes) and the [service logs](./service.md#read-the-logs).
