# Run the classifier on your Mac

You can sort mail without sending its text to an outside classifier or getting an API key. On an Apple Silicon Mac, mailtriage installs and runs Kev for you. Once setup finishes, ordinary classification runs offline.

Kev answers the same three questions as OpenRouter's Jev, with a choice, confidence and probabilities for each label. Your categories, urgency and attention list work the same way.

::: warning Important
- Local Kev needs a Mac with Apple Silicon (M1 or later). On another machine, [run your own Kev server](./provider.md#run-your-own-kev-server).
- Kev-4B is less accurate than Jev in [Kev's published comparisons](https://github.com/jaredpalmer/kev/blob/kev-1.0/README.md#what-to-expect). Expect more uncertain mail. Check its choices for a few days before turning on live filing.
- Kev starts when mail needs classifying and stops after 15 minutes without a request. The first message after a pause waits for the model to load. The 10 to 30 second estimate still needs a live check with mailtriage.
:::

## What you need

- **An Apple Silicon Mac**: 32 GB of memory is comfortable. With less, setup warns you, and other programs may slow down during classification.
- **About 15 GB of free disk space**: The model download is about 9 GB.
- **git**: If it is missing, install Apple's command line tools with `xcode-select --install`.
- **uv**: If it is missing and you have Homebrew, setup offers `brew install uv`. Otherwise follow [uv's installation instructions](https://docs.astral.sh/uv/getting-started/installation/).

## Set it up

Run setup and choose **Local Kev on this Mac**:

```sh
mailtriage setup
```

To switch an account you already set up:

```sh
mailtriage setup --update --account work --provider kev
```

Setup checks free space and asks before downloading. It gets Kev at tag `kev-1.0`, prepares its Python environment, downloads `jaredpalmer/kev-4b@v1.0`, classifies a sample message, confirms Kev stopped, then saves the installation and config. The first start shows Kev's download progress.

If a download or install step stops, run the same setup command again. Finished steps are reused. In a script, `--yes` agrees to the download:

```sh
mailtriage setup --update --yes --account work --provider kev
```

## How it starts and stops

When `sync`, `watch` or the background service has mail to classify, mailtriage starts Kev and waits for it to load. After 15 minutes without a request, Kev stops and frees memory. The service needs no separate Kev job.

Kev listens on `127.0.0.1:8009` and needs no key. Ordinary starts keep both uv and the model hub offline after setup. Setup itself can download dependencies and weights.

Kev uses about 12 GB of memory according to its published guidance. [Kev's Apple M5 benchmark](https://github.com/jaredpalmer/kev/blob/kev-1.0/README.md#what-to-expect) reports 721 ms for five questions; this is an upstream result, not a mailtriage measurement. Speed and memory on this Mac still need the live check.

If Kev cannot start, classification waits. Your mail stays queued, no retry attempt is used, and the next pass tries again. The result of `sync` gives a reason:

| Reason | What to do |
| --- | --- |
| `local Kev is not installed; run mailtriage setup --update` | Run `mailtriage setup --update --account work`. |
| `port 8009 is used by another program` | Stop that program. |
| `local Kev did not start; see .../kev.log` | Read the end of `kev.log`, then follow [Repair an installation](#repair-an-installation) if setup cannot recover. |

An unavailable server you run gives `the Kev server did not answer at URL`. Check its address, key and service. A server that returns an HTTP or invalid answer fails that classification and uses an attempt.

`mailtriage doctor --account work` reports whether local Kev is installed and ready. It never starts Kev; a healthy installation can show `running: false`. See [Local Kev diagnostics](../reference/provider.md#doctor-and-kev).

## Where it lives

Kev is shared by your mailtriage configs. Its folder sits next to mailtriage's [private Himalaya](./himalaya.md#a-private-himalaya):

| Path | What it holds |
| --- | --- |
| `~/.local/share/mailtriage/kev/src` | Kev's code and Python environment |
| `~/.local/share/mailtriage/kev/hf` | The model cache, about 9 GB |
| `~/.local/share/mailtriage/kev/python`, `uv-cache` | uv's Python installation and package cache |
| `~/.local/share/mailtriage/kev/kev.log` | Kev's start, stop and process output |
| `~/.local/share/mailtriage/kev/installed.json` | The installed release, checkpoint and uv path |

If `XDG_DATA_HOME` is set, the folder is `$XDG_DATA_HOME/mailtriage/kev`.

## Repair an installation

First try `mailtriage setup --update --yes --account work`, keeping any `--config` and account flags from your original setup command. A missing artifact can be installed again. `doctor` reports installation progress, missing files, an occupied port and uncertain ownership without starting Kev. A present `hf` folder alone cannot prove that every model file is intact.

If setup still cannot load Kev or reports uncertain ownership, stop every mailtriage background service that uses this shared installation with its normal `mailtriage service stop --account ACCOUNT` command, including `--config PATH` where needed. End foreground `watch`, `sync`, `classify` and `reclassify` commands too. Keep all callers stopped during repair.

In Activity Monitor, identify any live `mailtriage kev run --dir DIR` or `mailtriage kev mutate --dir DIR` process and its Kev, git, uv or brew descendants by their current command line and working directory. Quit an owned supervisor normally so it can stop its children. Wait until no such writer or server remains and ownership termination is confirmed. A free lock by itself does not prove that its children stopped. Do not move files while a writer is active or cleanup remains uncertain.

For damaged weights with clean, confirmed ownership, use Finder to move only the known private `DIR/hf` folder aside as a backup and remove `DIR/installed.json`. Then rerun the original setup command with `--update --yes`. If `hf` is missing already, setup can take this route without moving it. Do not guess at files inside the model cache.

If ownership evidence is abandoned, corrupt or unknown, independently identify and end any surviving owned process group using its live process identity. Once every caller and writer has stopped, use Finder to move the whole private `DIR` to a sibling backup, including its ownership evidence. Rerun the original setup command with `--update --yes` for a fresh installation. Keep the backup until the new setup succeeds; allow room for another download.

## Remove it

`mailtriage self uninstall` keeps local Kev because another config may use it. Switch every account that uses it to another classifier with `mailtriage setup --update --account work`. Stop all callers and follow the live process and confirmed ownership checks in [Repair an installation](#repair-an-installation). Once no server or writer remains and ownership is confirmed, remove the Kev folder in Finder if you no longer need it. Keep uncertain ownership evidence until you can verify the old processes have ended.
