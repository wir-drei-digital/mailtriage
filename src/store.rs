use crate::domain::{MailboxSnapshot, NormalizedMessage, SourceEnvelope};
use crate::filing::{open_states_sql, StageOptions};
use anyhow::{bail, Result};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, time::Duration as StdDuration};
use uuid::Uuid;

pub struct Store {
    pub(crate) db: Connection,
}
#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub account: String,
    pub normalized: Option<NormalizedMessage>,
    pub envelope: Value,
    pub status: String,
    pub classification: Option<Value>,
    pub overrides: Value,
    pub review_state: String,
    pub observed_at: String,
    pub error: Option<String>,
    pub generation: String,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(StdDuration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let version: u32 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 3 {
            bail!("database schema is newer than this binary");
        }
        if version == 0 {
            db.execute_batch("BEGIN IMMEDIATE;
CREATE TABLE metadata(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
INSERT INTO metadata VALUES('revision',0);
CREATE TABLE accounts(name TEXT PRIMARY KEY, identity TEXT NOT NULL, generation TEXT NOT NULL);
CREATE TABLE messages(id TEXT PRIMARY KEY, account TEXT NOT NULL REFERENCES accounts(name), fingerprint TEXT,
 normalized TEXT, envelope TEXT NOT NULL, status TEXT NOT NULL, classification TEXT, overrides TEXT NOT NULL DEFAULT '{}',
 review_state TEXT NOT NULL DEFAULT 'open', observed_at TEXT NOT NULL, error TEXT, generation TEXT NOT NULL);
CREATE UNIQUE INDEX content_identity ON messages(account,fingerprint) WHERE fingerprint IS NOT NULL;
CREATE INDEX message_account ON messages(account,observed_at,id);
CREATE TABLE aliases(account TEXT NOT NULL, alias TEXT NOT NULL, canonical TEXT NOT NULL REFERENCES messages(id), PRIMARY KEY(account,alias));
CREATE TABLE occurrences(account TEXT NOT NULL, mailbox TEXT NOT NULL, epoch INTEGER NOT NULL, uid INTEGER NOT NULL,
 message_id TEXT NOT NULL REFERENCES messages(id), PRIMARY KEY(account,mailbox,epoch,uid));
CREATE TABLE checkpoints(account TEXT NOT NULL, mailbox TEXT NOT NULL, epoch INTEGER NOT NULL, last_uid INTEGER NOT NULL,
 scanned_at TEXT, complete INTEGER NOT NULL DEFAULT 0, error TEXT, PRIMARY KEY(account,mailbox));
CREATE TABLE jobs(message_id TEXT PRIMARY KEY REFERENCES messages(id), state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
 next_after TEXT NOT NULL, lease_until TEXT, generation TEXT NOT NULL);
CREATE TABLE attempts(id INTEGER PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id), created_at TEXT NOT NULL,
 generation TEXT NOT NULL, result TEXT, error TEXT);
PRAGMA user_version=1; COMMIT;")?;
        }
        if version < 2 {
            db.execute_batch("BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN source_managed INTEGER NOT NULL DEFAULT 0;
CREATE TABLE reconciliation(account TEXT NOT NULL, mailbox TEXT NOT NULL, epoch INTEGER NOT NULL, cursor INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(account,mailbox));
PRAGMA user_version=2; COMMIT;")?;
        }
        if version < 3 {
            db.execute_batch("BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN rfc_message_id TEXT;
ALTER TABLE messages ADD COLUMN size INTEGER;
ALTER TABLE messages ADD COLUMN internal_date TEXT;
CREATE INDEX message_rfc_id ON messages(account, rfc_message_id);
CREATE TABLE filing_state(account TEXT PRIMARY KEY, mode TEXT NOT NULL DEFAULT 'off', enabled_at TEXT,
 bootstrap_done INTEGER NOT NULL DEFAULT 0, last_pass TEXT);
CREATE TABLE placements(account TEXT NOT NULL, message_id TEXT PRIMARY KEY REFERENCES messages(id),
 source_folder TEXT NOT NULL, home_folder TEXT, home_epoch INTEGER, home_uid INTEGER,
 location_state TEXT NOT NULL DEFAULT 'known', absent_since TEXT, desired_target TEXT,
 pinned INTEGER NOT NULL DEFAULT 0, eligible_once INTEGER NOT NULL DEFAULT 0, desired_rev INTEGER NOT NULL DEFAULT 0,
 filed_at TEXT, filed_by TEXT, flag_attempted_at TEXT, flagged_at TEXT,
 done_inferred INTEGER NOT NULL DEFAULT 0, blocked_reason TEXT);
CREATE INDEX placement_account ON placements(account);
CREATE TABLE folders(account TEXT NOT NULL, native TEXT NOT NULL, configured TEXT, category_id TEXT, origin TEXT,
 state TEXT NOT NULL, role_verified INTEGER NOT NULL DEFAULT 0, confirmed INTEGER NOT NULL DEFAULT 0,
 subscribed INTEGER NOT NULL DEFAULT 0, pause_reason TEXT, epoch INTEGER, watch_from_uid INTEGER,
 rescan_epoch INTEGER, rescan_below_uid INTEGER, rescan_complete INTEGER NOT NULL DEFAULT 1,
 checked_at TEXT, error TEXT, PRIMARY KEY(account, native));
CREATE TABLE arrivals(id INTEGER PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, epoch INTEGER NOT NULL,
 uid INTEGER NOT NULL, message_id TEXT NOT NULL, rfc_message_id TEXT, state TEXT NOT NULL DEFAULT 'pending',
 kind TEXT, intent_id INTEGER, created_at TEXT NOT NULL, resolved_at TEXT, UNIQUE(account, folder, epoch, uid));
CREATE INDEX arrival_state ON arrivals(account, state);
CREATE TABLE rescan_sets(account TEXT NOT NULL, folder TEXT NOT NULL, epoch INTEGER NOT NULL, message_id TEXT NOT NULL,
 PRIMARY KEY(account, folder, epoch, message_id));
CREATE TABLE filing_intents(id INTEGER PRIMARY KEY, account TEXT NOT NULL, message_id TEXT NOT NULL, kind TEXT NOT NULL,
 folder TEXT NOT NULL, epoch INTEGER NOT NULL, uid INTEGER NOT NULL, target TEXT, target_epoch INTEGER,
 target_uid_next INTEGER, target_uid INTEGER, desired_rev INTEGER NOT NULL DEFAULT 0,
 consumes_eligible INTEGER NOT NULL DEFAULT 0, batch TEXT, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
 next_after TEXT, dispatched_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, error TEXT);
CREATE INDEX intent_state ON filing_intents(account, state);
CREATE TABLE filing_reverts(id INTEGER PRIMARY KEY, account TEXT NOT NULL, parent_intent INTEGER NOT NULL,
 folder TEXT NOT NULL, folder_epoch INTEGER NOT NULL, uid INTEGER NOT NULL, target TEXT NOT NULL,
 target_epoch INTEGER NOT NULL, state TEXT NOT NULL, target_uid INTEGER, created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL, error TEXT);
CREATE TABLE filing_events(id INTEGER PRIMARY KEY, account TEXT NOT NULL, message_id TEXT, folder TEXT,
 at TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '{}');
CREATE INDEX event_account ON filing_events(account, id);
PRAGMA user_version=3; COMMIT;")?;
        }
        Ok(Self { db })
    }
    pub fn revision(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT value FROM metadata WHERE key='revision'", [], |r| {
                r.get(0)
            })?)
    }
    pub fn ensure_account(&mut self, name: &str, identity: &str, generation: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT identity,generation FROM accounts WHERE name=?",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_identity, old_generation)) = previous {
            if old_identity != identity {
                bail!("account identity changed; use a new account namespace");
            }
            if old_generation != generation {
                tx.execute(
                    "UPDATE accounts SET generation=? WHERE name=?",
                    params![generation, name],
                )?;
                tx.execute(
                    "UPDATE messages SET status='pending',generation=?,error=NULL WHERE account=? AND review_state='open'",
                    params![generation, name],
                )?;
                tx.execute("INSERT INTO jobs(message_id,state,attempts,next_after,generation) SELECT id,'queued',0,?,? FROM messages WHERE account=? AND review_state='open' ON CONFLICT(message_id) DO UPDATE SET state='queued',attempts=0,next_after=excluded.next_after,lease_until=NULL,generation=excluded.generation",params![now(),generation,name])?;
                bump(&tx)?;
            }
        } else {
            tx.execute(
                "INSERT INTO accounts VALUES(?,?,?)",
                params![name, identity, generation],
            )?;
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn ingest(
        &mut self,
        account: &str,
        msg: &NormalizedMessage,
        generation: &str,
    ) -> Result<String> {
        if let Some(id) = self
            .db
            .query_row(
                "SELECT id FROM messages WHERE account=? AND fingerprint=?",
                params![account, msg.raw_sha256],
                |r| r.get(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        let id = format!("msg_{}", Uuid::new_v4().simple());
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO messages(id,account,fingerprint,normalized,envelope,status,observed_at,generation) VALUES(?,?,?,?,?,'pending',?,?)",params![id,account,msg.raw_sha256,serde_json::to_string(msg)?,json!({"subject":msg.subject,"from":msg.from,"sent_at":msg.sent_at}).to_string(),now(),generation])?;
        tx.execute(
            "INSERT INTO jobs(message_id,state,next_after,generation) VALUES(?,'queued',?,?)",
            params![id, now(), generation],
        )?;
        bump(&tx)?;
        tx.commit()?;
        Ok(id)
    }
    pub fn record(&self, account: &str, id: &str) -> Result<Option<Record>> {
        let canonical: Option<String> = self
            .db
            .query_row(
                "SELECT canonical FROM aliases WHERE account=? AND alias=?",
                params![account, id],
                |r| r.get(0),
            )
            .optional()?;
        let id = canonical.as_deref().unwrap_or(id);
        let mut stmt=self.db.prepare("SELECT id,account,normalized,envelope,status,classification,overrides,review_state,observed_at,error,generation FROM messages WHERE account=? AND id=?")?;
        Ok(stmt
            .query_row(params![account, id], row_record)
            .optional()?)
    }
    pub fn records(&self, account: &str) -> Result<Vec<Record>> {
        let mut stmt=self.db.prepare("SELECT id,account,normalized,envelope,status,classification,overrides,review_state,observed_at,error,generation FROM messages WHERE account=? ORDER BY observed_at DESC,id DESC")?;
        let rows = stmt
            .query_map([account], row_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    /// Metadata projection deliberately never loads normalized message bodies.
    pub fn metadata_records(&self, account: &str) -> Result<Vec<Record>> {
        let mut stmt=self.db.prepare("SELECT id,account,NULL,envelope,status,classification,overrides,review_state,observed_at,error,generation FROM messages WHERE account=? ORDER BY observed_at DESC,id DESC")?;
        let rows = stmt
            .query_map([account], row_record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn provenance(&self, account: &str, id: &str) -> Result<Vec<Value>> {
        let mut stmt=self.db.prepare("SELECT mailbox,epoch,uid FROM occurrences WHERE account=? AND message_id=? ORDER BY mailbox,uid")?;
        let rows=stmt.query_map(params![account,id],|r|Ok(json!({"backend":"imap","mailbox":r.get::<_,String>(0)?,"uid_validity":r.get::<_,u64>(1)?,"uid":r.get::<_,u64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn update_overrides(
        &mut self,
        account: &str,
        id: &str,
        expected: &Value,
        overrides: &Value,
    ) -> Result<bool> {
        let tx = self.db.transaction()?;
        let changed = tx.execute(
            "UPDATE messages SET overrides=? WHERE account=? AND id=? AND overrides=?",
            params![overrides.to_string(), account, id, expected.to_string()],
        )?;
        if changed == 1 {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }
    pub fn review(&mut self, account: &str, id: &str, done: bool) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE messages SET review_state=? WHERE account=? AND id=?",
            params![if done { "done" } else { "open" }, account, id],
        )?;
        bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub fn requeue(&mut self, account: &str, ids: &[String], generation: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            tx.execute("UPDATE messages SET status='pending',error=NULL,generation=? WHERE account=? AND id=?",params![generation,account,id])?;
            tx.execute("INSERT INTO jobs(message_id,state,next_after,generation) SELECT id,'queued',?,? FROM messages WHERE account=? AND id=? ON CONFLICT(message_id) DO UPDATE SET state='queued',attempts=0,next_after=excluded.next_after,lease_until=NULL,generation=excluded.generation",params![now(),generation,account,id])?;
        }
        if !ids.is_empty() {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn queued(&self, account: &str, limit: usize) -> Result<Vec<String>> {
        let mut stmt=self.db.prepare("SELECT j.message_id FROM jobs j JOIN messages m ON m.id=j.message_id WHERE m.account=? AND ((j.state IN ('queued','retry') AND j.next_after<=?) OR (j.state='leased' AND j.lease_until<=?)) ORDER BY m.observed_at,j.message_id LIMIT ?")?;
        let rows = stmt
            .query_map(params![account, now(), now(), limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn lease(&mut self, id: &str, generation: &str, seconds: u64) -> Result<bool> {
        let tx = self.db.transaction()?;
        let count=tx.execute("UPDATE jobs SET state='leased',attempts=attempts+1,lease_until=? WHERE message_id=? AND generation=? AND ((state IN ('queued','retry') AND next_after<=?) OR (state='leased' AND lease_until<=?))",params![(Utc::now()+Duration::seconds(seconds as i64)).to_rfc3339(),id,generation,now(),now()])?;
        tx.commit()?;
        Ok(count == 1)
    }
    pub fn finish(&mut self, id: &str, generation: &str, result: &Value) -> Result<bool> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO attempts(message_id,created_at,generation,result) VALUES(?,?,?,?)",
            params![id, now(), generation, result.to_string()],
        )?;
        let n=tx.execute("UPDATE messages SET classification=?,status=?,error=NULL WHERE id=? AND generation=? AND EXISTS(SELECT 1 FROM jobs WHERE message_id=? AND generation=? AND state='leased')",params![result.to_string(),result["state"].as_str().unwrap_or("uncertain"),id,generation,id,generation])?;
        if n == 1 {
            tx.execute("UPDATE jobs SET state='complete',lease_until=NULL WHERE message_id=? AND generation=?",params![id,generation])?;
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(n == 1)
    }
    pub fn fail(
        &mut self,
        id: &str,
        generation: &str,
        message: &str,
        max_attempts: u32,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        let attempts: Option<u32> = tx
            .query_row(
                "SELECT attempts FROM jobs WHERE message_id=? AND generation=?",
                params![id, generation],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(attempts) = attempts {
            tx.execute(
                "INSERT INTO attempts(message_id,created_at,generation,error) VALUES(?,?,?,?)",
                params![id, now(), generation, message],
            )?;
            tx.execute(
                "UPDATE messages SET status='failed',error=? WHERE id=? AND generation=?",
                params![message, id, generation],
            )?;
            let delay = 30_i64.saturating_mul(2_i64.pow(attempts.min(7)));
            tx.execute("UPDATE jobs SET state=?,next_after=?,lease_until=NULL WHERE message_id=? AND generation=?",params![if attempts>=max_attempts {"terminal"} else {"retry"},(Utc::now()+Duration::seconds(delay)).to_rfc3339(),id,generation])?;
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn checkpoint(
        &mut self,
        account: &str,
        mailbox: &str,
        snapshot: &MailboxSnapshot,
    ) -> Result<u64> {
        let tx = self.db.transaction()?;
        let prev: Option<(u64, u64)> = tx
            .query_row(
                "SELECT epoch,last_uid FROM checkpoints WHERE account=? AND mailbox=?",
                params![account, mailbox],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((epoch, last_uid)) = prev {
            if epoch == snapshot.uid_validity {
                return Ok(last_uid);
            }
            capture_rescan_set(&tx, account, mailbox, snapshot)?;
            tx.execute(
                "DELETE FROM occurrences WHERE account=? AND mailbox=?",
                params![account, mailbox],
            )?;
            tx.execute("UPDATE messages SET status='failed',error='source epoch changed; awaiting rediscovery' WHERE account=? AND normalized IS NULL AND NOT EXISTS(SELECT 1 FROM occurrences o WHERE o.message_id=messages.id)",[account])?;
            tx.execute("UPDATE jobs SET state='terminal',lease_until=NULL WHERE message_id IN(SELECT id FROM messages WHERE account=? AND normalized IS NULL AND NOT EXISTS(SELECT 1 FROM occurrences o WHERE o.message_id=messages.id))",[account])?;
        }
        tx.execute("INSERT INTO checkpoints(account,mailbox,epoch,last_uid,complete) VALUES(?,?,?,0,0) ON CONFLICT(account,mailbox) DO UPDATE SET epoch=excluded.epoch,last_uid=0,complete=0,error=NULL",params![account,mailbox,snapshot.uid_validity])?;
        bump(&tx)?;
        tx.commit()?;
        Ok(0)
    }
    #[allow(clippy::too_many_arguments)] // One atomic mailbox discovery checkpoint.
    pub fn stage(
        &mut self,
        account: &str,
        mailbox: &str,
        epoch: u64,
        through: u64,
        envelopes: &[SourceEnvelope],
        generation: &str,
        complete: bool,
    ) -> Result<usize> {
        let opts = StageOptions {
            record_arrivals: false,
            known_targets: &BTreeMap::new(),
            rescan_filter: None,
        };
        self.stage_with(
            account, mailbox, epoch, through, envelopes, generation, complete, &opts,
        )
    }
    pub fn scan_error(&mut self, account: &str, mailbox: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("INSERT INTO checkpoints(account,mailbox,epoch,last_uid,complete,error) VALUES(?,?,0,0,0,'mailbox scan failed') ON CONFLICT(account,mailbox) DO UPDATE SET complete=0,error='mailbox scan failed'",params![account,mailbox])?;
        bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    pub fn locator(&self, account: &str, id: &str) -> Result<Option<(String, u64, u64)>> {
        Ok(self.db.query_row("SELECT mailbox,epoch,uid FROM occurrences WHERE account=? AND message_id=? ORDER BY mailbox LIMIT 1",params![account,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?)
    }
    pub fn attach(&mut self, account: &str, id: &str, msg: &NormalizedMessage) -> Result<String> {
        let tx = self.db.transaction()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM messages WHERE account=? AND fingerprint=? AND id<>?",
                params![account, msg.raw_sha256, id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            // Preserve explicit user edits instead of silently discarding them on coalescing.
            let (overrides, review): (String, String) = tx.query_row(
                "SELECT overrides,review_state FROM messages WHERE account=? AND id=?",
                params![account, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if overrides != "{}" || review != "open" {
                bail!("duplicate source has local edits; manual reconciliation required");
            }
            tx.execute(
                "UPDATE occurrences SET message_id=? WHERE account=? AND message_id=?",
                params![existing, account, id],
            )?;
            tx.execute(
                "UPDATE messages SET source_managed=1 WHERE id=?",
                [&existing],
            )?;
            tx.execute(
                "UPDATE arrivals SET message_id=? WHERE account=? AND message_id=?",
                params![existing, account, id],
            )?;
            tx.execute("INSERT OR IGNORE INTO rescan_sets SELECT account,folder,epoch,?1 FROM rescan_sets WHERE account=?2 AND message_id=?3",params![existing,account,id])?;
            tx.execute(
                "DELETE FROM rescan_sets WHERE account=? AND message_id=?",
                params![account, id],
            )?;
            merge_transport(&tx, account, &existing, id)?;
            tx.execute(
                "UPDATE aliases SET canonical=? WHERE account=? AND canonical=?",
                params![existing, account, id],
            )?;
            tx.execute(
                "INSERT INTO aliases VALUES(?,?,?)",
                params![account, id, existing],
            )?;
            tx.execute("DELETE FROM jobs WHERE message_id=?", [id])?;
            tx.execute(
                "UPDATE attempts SET message_id=? WHERE message_id=?",
                params![existing, id],
            )?;
            tx.execute(
                "DELETE FROM messages WHERE account=? AND id=?",
                params![account, id],
            )?;
            bump(&tx)?;
            tx.commit()?;
            return Ok(existing);
        }
        let mut envelope = envelope_of(&tx, account, id)?.unwrap_or_else(|| json!({}));
        merge_envelope(
            &mut envelope,
            &json!({"subject":msg.subject,"from":msg.from,"sent_at":msg.sent_at}),
            &["subject", "from", "sent_at"],
        );
        tx.execute(
            "UPDATE messages SET fingerprint=?,normalized=?,envelope=? WHERE account=? AND id=?",
            params![
                msg.raw_sha256,
                serde_json::to_string(msg)?,
                envelope.to_string(),
                account,
                id
            ],
        )?;
        bump(&tx)?;
        tx.commit()?;
        Ok(id.to_string())
    }
    pub fn source_presence(&self, account: &str, id: &str) -> Result<Option<bool>> {
        let (managed,present):(bool,bool)=self.db.query_row("SELECT source_managed,EXISTS(SELECT 1 FROM occurrences WHERE message_id=messages.id) FROM messages WHERE account=? AND id=?",params![account,id],|r|Ok((r.get(0)?,r.get(1)?)))?;
        Ok(if managed { Some(present) } else { None })
    }
    pub fn reconcile_cursor(&self, account: &str, mailbox: &str, epoch: u64) -> Result<u64> {
        Ok(self
            .db
            .query_row(
                "SELECT cursor FROM reconciliation WHERE account=? AND mailbox=? AND epoch=?",
                params![account, mailbox, epoch],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_range(
        &mut self,
        account: &str,
        mailbox: &str,
        epoch: u64,
        after: u64,
        through: u64,
        present: &[u64],
        finished: bool,
    ) -> Result<usize> {
        Ok(self
            .reconcile_range_ids(account, mailbox, epoch, after, through, present, finished)?
            .len())
    }
    pub fn coverage(&self, account: &str) -> Result<Value> {
        let mut stmt=self.db.prepare("SELECT mailbox,epoch,last_uid,scanned_at,complete,error FROM checkpoints WHERE account=? ORDER BY mailbox")?;
        let scans=stmt.query_map([account],|r|Ok(json!({"mailbox":r.get::<_,String>(0)?,"uid_validity":r.get::<_,u64>(1)?,"last_uid":r.get::<_,u64>(2)?,"scanned_at":r.get::<_,Option<String>>(3)?,"complete":r.get::<_,bool>(4)?,"error":r.get::<_,Option<String>>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let pending: i64 = self.db.query_row(
            "SELECT count(*) FROM messages WHERE account=? AND status IN ('pending','failed') ",
            [account],
            |r| r.get(0),
        )?;
        let pending_jobs:i64=self.db.query_row("SELECT count(*) FROM jobs j JOIN messages m ON m.id=j.message_id WHERE m.account=? AND j.state IN ('queued','retry','leased')",[account],|r|r.get(0))?;
        let failed: i64 = self.db.query_row(
            "SELECT count(*) FROM messages WHERE account=? AND status='failed'",
            [account],
            |r| r.get(0),
        )?;
        let complete = scans.iter().all(|s| s["complete"] == true) && pending == 0;
        Ok(
            json!({"scans":scans,"pending_or_failed":pending,"pending_jobs":pending_jobs,"failed":failed,"complete":complete,"scope":"locally indexed messages and configured mailbox UID windows"}),
        )
    }
}
pub(crate) fn row_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    fn decode<T: serde::de::DeserializeOwned>(s: String) -> rusqlite::Result<T> {
        serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
    }
    Ok(Record {
        id: r.get(0)?,
        account: r.get(1)?,
        normalized: r.get::<_, Option<String>>(2)?.map(decode).transpose()?,
        envelope: decode(r.get(3)?)?,
        status: r.get(4)?,
        classification: r.get::<_, Option<String>>(5)?.map(decode).transpose()?,
        overrides: decode(r.get(6)?)?,
        review_state: r.get(7)?,
        observed_at: r.get(8)?,
        error: r.get(9)?,
        generation: r.get(10)?,
    })
}
/// Epoch reset of a watched folder (spec "Folders"): remember which messages
/// to look for in the new epoch before its occurrences are deleted.
fn capture_rescan_set(
    tx: &Connection,
    account: &str,
    mailbox: &str,
    snapshot: &MailboxSnapshot,
) -> Result<()> {
    let new = snapshot.uid_validity;
    let intents = format!("INSERT OR IGNORE INTO rescan_sets SELECT account,target,?3,message_id FROM filing_intents WHERE account=?1 AND target=?2 AND state IN {}", open_states_sql());
    for sql in [
        "INSERT OR IGNORE INTO rescan_sets SELECT account,mailbox,?3,message_id FROM occurrences WHERE account=?1 AND mailbox=?2",
        "INSERT OR IGNORE INTO rescan_sets SELECT account,home_folder,?3,message_id FROM placements WHERE account=?1 AND home_folder=?2",
        &intents,
        "INSERT OR IGNORE INTO rescan_sets SELECT account,folder,?3,message_id FROM arrivals WHERE account=?1 AND folder=?2 AND state='pending'",
        // An absent message may have come back here unseen; done inference
        // relies on the rescan finding it (spec "Done inference").
        "INSERT OR IGNORE INTO rescan_sets SELECT account,?2,?3,message_id FROM placements WHERE account=?1 AND location_state='absent'",
    ] {
        tx.execute(sql, params![account, mailbox, new])?;
    }
    tx.execute(
        "UPDATE arrivals SET state='vanished',resolved_at=?3 WHERE account=?1 AND folder=?2 AND state='pending'",
        params![account, mailbox, now()],
    )?;
    tx.execute(
        "UPDATE folders SET rescan_epoch=?3,rescan_below_uid=?4,rescan_complete=0,epoch=?3,watch_from_uid=NULL WHERE account=?1 AND native=?2",
        params![account, mailbox, new, snapshot.uid_next],
    )?;
    Ok(())
}
/// Filing-aware merge of transport metadata into the canonical row: it keeps
/// what it has and fills gaps from the provisional row, whose flags are newer.
fn merge_transport(
    tx: &Connection,
    account: &str,
    existing: &str,
    provisional: &str,
) -> Result<()> {
    tx.execute("UPDATE messages SET rfc_message_id=COALESCE(rfc_message_id,(SELECT rfc_message_id FROM messages WHERE id=?2)),size=COALESCE(size,(SELECT size FROM messages WHERE id=?2)),internal_date=COALESCE(internal_date,(SELECT internal_date FROM messages WHERE id=?2)) WHERE account=?3 AND id=?1",params![existing,provisional,account])?;
    let (Some(mut envelope), Some(from)) = (
        envelope_of(tx, account, existing)?,
        envelope_of(tx, account, provisional)?,
    ) else {
        return Ok(());
    };
    let keys: Vec<&str> = ["message_id", "internal_date", "size"]
        .into_iter()
        .filter(|k| envelope.get(*k).is_none_or(Value::is_null))
        .chain(["flags"])
        .filter(|k| from.get(*k).is_some_and(|v| !v.is_null()))
        .collect();
    merge_envelope(&mut envelope, &from, &keys);
    tx.execute(
        "UPDATE messages SET envelope=? WHERE account=? AND id=?",
        params![envelope.to_string(), account, existing],
    )?;
    Ok(())
}
/// The stored envelope JSON of a message, if the message exists.
pub(crate) fn envelope_of(db: &Connection, account: &str, id: &str) -> Result<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT envelope FROM messages WHERE account=? AND id=?",
            params![account, id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.map(|s| serde_json::from_str(&s)).transpose()?)
}
/// Copies `keys` present in `from` into the envelope object, keeping every other key.
pub(crate) fn merge_envelope(envelope: &mut Value, from: &Value, keys: &[&str]) {
    if !envelope.is_object() {
        *envelope = json!({});
    }
    if let (Some(obj), Some(src)) = (envelope.as_object_mut(), from.as_object()) {
        for key in keys {
            if let Some(v) = src.get(*key) {
                obj.insert((*key).to_string(), v.clone());
            }
        }
    }
}
pub(crate) fn bump(db: &Connection) -> Result<()> {
    db.execute("UPDATE metadata SET value=value+1 WHERE key='revision'", [])?;
    Ok(())
}
pub fn now() -> String {
    Utc::now().to_rfc3339()
}
