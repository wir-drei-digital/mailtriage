//! The mailbox Himalaya opens for `--mailbox NAME` (only `message read`
//! gets one from mailtriage), following each tested version's resolver as
//! read from Himalaya's source (`Account::resolve_mailbox`, `MailboxArg`):
//!
//! 1. the merged alias map: the global `mailbox.alias` table, overridden key
//!    by key by the account's `accounts.<name>.mailbox.alias`; keys are
//!    lowercased (`str::to_lowercase`) when loaded and looked up lowercased,
//!    `aliases` is accepted as another spelling of `alias`;
//! 2. then the version's `roles` table (2.2 and later), by the lowercased name;
//! 3. else the literal name.
//!
//! A folder whose target is not the folder itself, with `INBOX` compared
//! case-insensitively, would read another mailbox: an alias conflict.
use std::collections::BTreeMap;

/// Where `--mailbox NAME` leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Mailbox(String),
    /// Two alias keys in one table differ only in case and name different
    /// mailboxes; which one Himalaya uses is not defined.
    Ambiguous,
}

/// One account's resolver for one Himalaya version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolver {
    /// Lowercased key to target; `None` for an ambiguous key.
    aliases: BTreeMap<String, Option<String>>,
    /// Lowercase role name to mailbox.
    roles: BTreeMap<String, String>,
}

impl Resolver {
    /// The resolver of `account` in the Himalaya TOML `text`, with a
    /// version's `roles`. `Err` (why) when the TOML cannot be parsed or an
    /// alias table is malformed: Himalaya would refuse the file too, and
    /// mailtriage then reads no folder.
    pub fn from_toml(
        text: &str,
        account: &str,
        roles: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let parsed: toml::Value =
            toml::from_str(text).map_err(|_| "it is not valid TOML".to_owned())?;
        let mut aliases = alias_table(parsed.get("mailbox"), "mailbox")?;
        let own = parsed
            .get("accounts")
            .and_then(|accounts| accounts.get(account))
            .and_then(|account| account.get("mailbox"));
        let own = alias_table(own, &format!("accounts.{account}.mailbox"))?;
        aliases.extend(own);
        Ok(Self {
            aliases,
            roles: roles.clone(),
        })
    }

    /// The mailbox `--mailbox folder` opens.
    pub fn target(&self, folder: &str) -> Target {
        let key = folder.to_lowercase();
        match self.aliases.get(&key) {
            Some(Some(native)) => Target::Mailbox(native.clone()),
            Some(None) => Target::Ambiguous,
            None => Target::Mailbox(
                self.roles
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| folder.to_owned()),
            ),
        }
    }

    /// The folders, in order, whose target is not the folder itself.
    pub fn conflicts(&self, folders: &[String]) -> Vec<String> {
        folders
            .iter()
            .filter(|folder| match self.target(folder) {
                Target::Mailbox(native) => !same_mailbox(&native, folder),
                Target::Ambiguous => true,
            })
            .cloned()
            .collect()
    }
}

/// The aliases of one `mailbox` table, keys lowercased. Both spellings at
/// once, a non-table or a non-string target is malformed.
fn alias_table(
    mailbox: Option<&toml::Value>,
    at: &str,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let Some(mailbox) = mailbox else {
        return Ok(BTreeMap::new());
    };
    let mailbox = mailbox
        .as_table()
        .ok_or_else(|| format!("{at} is not a table"))?;
    let table = match (mailbox.get("alias"), mailbox.get("aliases")) {
        (Some(_), Some(_)) => {
            return Err(format!("{at} has both alias and aliases"));
        }
        (Some(table), None) | (None, Some(table)) => table,
        (None, None) => return Ok(BTreeMap::new()),
    };
    let table = table
        .as_table()
        .ok_or_else(|| format!("{at}.alias is not a table"))?;
    let mut aliases: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (key, native) in table {
        let native = native
            .as_str()
            .ok_or_else(|| format!("{at}.alias.{key} is not a string"))?;
        let key = key.to_lowercase();
        match aliases.get(&key) {
            None => {
                aliases.insert(key, Some(native.to_owned()));
            }
            Some(Some(seen)) if seen == native => {}
            Some(_) => {
                aliases.insert(key, None);
            }
        }
    }
    Ok(aliases)
}

/// Whether two native names select the same mailbox: equal, or both INBOX,
/// whose name IMAP treats case-insensitively (RFC 3501 5.1).
pub fn same_mailbox(a: &str, b: &str) -> bool {
    a == b || (a.eq_ignore_ascii_case("INBOX") && b.eq_ignore_ascii_case("INBOX"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_2_1() -> BTreeMap<String, String> {
        BTreeMap::from([("inbox".to_owned(), "INBOX".to_owned())])
    }

    fn folders(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_account_overrides_global_aliases_key_by_key() {
        let toml = r#"
[mailbox.alias]
news = "Lists/News"
receipts = "Bills"

[accounts.work]
imap.server = "imaps://x.test"

[accounts.work.mailbox.alias]
News = "News"
"#;
        let r = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap();
        // The account's `News` (lowercased) replaces the global `news`.
        assert_eq!(r.target("news"), Target::Mailbox("News".into()));
        assert_eq!(r.target("NEWS"), Target::Mailbox("News".into()));
        // The global alias applies where the account has none.
        assert_eq!(r.target("Receipts"), Target::Mailbox("Bills".into()));
        assert_eq!(r.target("Archive"), Target::Mailbox("Archive".into()));
        assert_eq!(
            r.conflicts(&folders(&["News", "Receipts", "Archive", "news"])),
            folders(&["Receipts", "news"])
        );
        // Another account sees only the global table.
        let other = Resolver::from_toml(toml, "home", &BTreeMap::new()).unwrap();
        assert_eq!(other.conflicts(&folders(&["News"])), folders(&["News"]));
    }

    #[test]
    fn keys_compare_case_insensitively_and_unicode_lowercases() {
        let toml = "[accounts.work.mailbox.alias]\n\"ÄRCHIV\" = \"Elsewhere\"\n";
        let r = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap();
        assert_eq!(r.target("ärchiv"), Target::Mailbox("Elsewhere".into()));
        assert_eq!(r.target("Ärchiv"), Target::Mailbox("Elsewhere".into()));
    }

    #[test]
    fn the_2_2_1_inbox_role_leads_to_inbox() {
        let r = Resolver::from_toml("", "work", &v2_2_1()).unwrap();
        assert_eq!(r.target("inbox"), Target::Mailbox("INBOX".into()));
        assert_eq!(r.target("Inbox"), Target::Mailbox("INBOX".into()));
        // INBOX itself, in any case, is not a conflict.
        assert!(r
            .conflicts(&folders(&["INBOX", "Inbox", "inbox", "Sent"]))
            .is_empty());
        // 2.1.0 has no roles: the literal name.
        let old = Resolver::from_toml("", "work", &BTreeMap::new()).unwrap();
        assert_eq!(old.target("Inbox"), Target::Mailbox("Inbox".into()));
        // An alias comes before the role.
        let aliased =
            Resolver::from_toml("[mailbox.alias]\ninbox = \"Mail/In\"\n", "work", &v2_2_1())
                .unwrap();
        assert_eq!(aliased.conflicts(&folders(&["INBOX"])), folders(&["INBOX"]));
    }

    #[test]
    fn aliases_spelled_aliases_count_and_ambiguous_keys_conflict() {
        let r = Resolver::from_toml(
            "[accounts.work.mailbox.aliases]\nsent = \"Sent Items\"\n",
            "work",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(r.conflicts(&folders(&["Sent"])), folders(&["Sent"]));
        let r = Resolver::from_toml(
            "[accounts.work.mailbox.alias]\nWork = \"A\"\nwork = \"B\"\nSame = \"X\"\nsame = \"X\"\n",
            "work",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(r.target("work"), Target::Ambiguous);
        assert_eq!(r.conflicts(&folders(&["Work", "X"])), folders(&["Work"]));
        // Equal targets are not ambiguous: `same` leads to X.
        assert_eq!(r.target("SAME"), Target::Mailbox("X".into()));
    }

    #[test]
    fn a_config_himalaya_would_refuse_is_an_error() {
        for (toml, why) in [
            ("[accounts.work\n", "not valid TOML"),
            ("mailbox = 3\n", "mailbox is not a table"),
            (
                "[mailbox]\nalias = {a = \"b\"}\naliases = {c = \"d\"}\n",
                "both alias and aliases",
            ),
            ("[accounts.work.mailbox]\nalias = 7\n", "is not a table"),
            (
                "[accounts.work.mailbox.alias]\nsent = 1\n",
                "accounts.work.mailbox.alias.sent is not a string",
            ),
        ] {
            let error = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap_err();
            assert!(error.contains(why), "{toml}: {error}");
        }
    }
}
