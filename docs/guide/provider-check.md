# Mail provider compatibility

Live filing needs more than an IMAP connection: moving messages, flags and folder roles must behave as mailtriage expects on your provider.

**No listed provider has a recorded live-filing go yet. Keep filing in `dry_run` until yours has passed.** Dry-run mode previews changes without writing to your mailbox.

## Outcome

| Provider | Live filing status |
| --- | --- |
| Gmail / Google Workspace | Not checked |
| Microsoft 365 / Outlook.com | Not checked |
| iCloud | Not checked |
| Fastmail or a Dovecot host | Not checked |

The automated Dovecot tests check the protocol implementation. They do not establish that Gmail, Microsoft 365, iCloud or Fastmail behave the same way.

## Verify a provider

Use a throwaway test account and follow the [mail provider test checklist](../development/mail-provider-check.md). It covers moves, read state, flags, folders, reply detection and login throttling.

Record the result and redacted evidence in that checklist. Do not include credentials, message bodies or real addresses. A failed check blocks live filing until resolved.

Return to [filing setup](./filing.md#rollout) after your provider has a recorded go.

## Detailed reference

- <span id="gmail-google-workspace"></span>[Gmail / Google Workspace](../development/mail-provider-check.md#gmail-google-workspace)
- <span id="microsoft-365-outlook-com"></span>[Microsoft 365 / Outlook.com](../development/mail-provider-check.md#microsoft-365-outlook-com)
- <span id="icloud"></span>[iCloud](../development/mail-provider-check.md#icloud)
- <span id="fastmail-or-a-dovecot-host"></span>[Fastmail or a Dovecot host](../development/mail-provider-check.md#fastmail-or-a-dovecot-host)
