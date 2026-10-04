//! Filing persistence (schema v3): the store API every filing step uses.
use super::{
    mode_str, open_states_sql, planner::Action, rfc_message_id, Arrival, CheckpointState,
    FilingStateRow, FolderRecord, HydrationBatch, Intent, IntentPatch, LocationState, MessageMeta,
    NewIntent, Placement, Revert, StageOptions,
};
use crate::domain::{FilingMode, MailboxSnapshot, SourceEnvelope};
use crate::store::{bump, envelope_of, merge_envelope, now, row_record, Record, Store};
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

const PLACEMENT_COLUMNS: &str = "account,message_id,source_folder,home_folder,home_epoch,home_uid,location_state,absent_since,desired_target,pinned,eligible_once,desired_rev,filed_at,filed_by,flag_attempted_at,flagged_at,done_inferred,blocked_reason";
const FOLDER_COLUMNS: &str = "account,native,configured,category_id,origin,state,role_verified,confirmed,subscribed,pause_reason,epoch,watch_from_uid,rescan_epoch,rescan_below_uid,rescan_complete,checked_at,error";
const ARRIVAL_COLUMNS: &str = "id,account,folder,epoch,uid,message_id,rfc_message_id,state,kind,intent_id,created_at,resolved_at";
const INTENT_COLUMNS: &str = "id,account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid_next,target_uid,desired_rev,consumes_eligible,batch,state,attempts,next_after,dispatched_at,created_at,updated_at,error";
const REVERT_COLUMNS: &str = "id,account,parent_intent,folder,folder_epoch,uid,target,target_epoch,state,target_uid,created_at,updated_at,error";

impl Store {
    pub fn schema_version(&self) -> Result<u32> {
        Ok(self.db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// The account's filing state; the default (`Off`, nulls) if never synced.
    pub fn filing_state(&self, account: &str) -> Result<FilingStateRow> {
        let row: Option<(String, Option<String>, bool, Option<String>)> = self
            .db
            .query_row(
                "SELECT mode,enabled_at,bootstrap_done,last_pass FROM filing_state WHERE account=?",
                [account],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((mode, enabled_at, bootstrap_done, last_pass)) = row else {
            return Ok(FilingStateRow {
                account: account.to_string(),
                mode: FilingMode::Off,
                enabled_at: None,
                bootstrap_done: false,
                last_pass: None,
            });
        };
        Ok(FilingStateRow {
            account: account.to_string(),
            mode: parse_mode(&mode)?,
            enabled_at,
            bootstrap_done,
            last_pass: last_pass.map(|s| serde_json::from_str(&s)).transpose()?,
        })
    }

    /// Applies the configured mode (spec "Filing mode"): off→on sets
    /// `enabled_at = now` and restarts the bootstrap; other changes keep both.
    pub fn sync_filing_mode(
        &mut self,
        account: &str,
        mode: FilingMode,
        now: &str,
    ) -> Result<FilingStateRow> {
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO filing_state(account) VALUES(?)",
            [account],
        )?;
        let (old, enabled_at): (String, Option<String>) = tx.query_row(
            "SELECT mode, enabled_at FROM filing_state WHERE account=?",
            [account],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let new = mode_str(mode);
        let turning_on = old == "off" && new != "off";
        let enabled_at = if turning_on {
            Some(now.to_string())
        } else {
            enabled_at
        };
        tx.execute(
            "UPDATE filing_state SET mode=?, enabled_at=? WHERE account=?",
            params![new, enabled_at, account],
        )?;
        if turning_on {
            tx.execute(
                "UPDATE filing_state SET bootstrap_done=0 WHERE account=?",
                [account],
            )?;
        }
        tx.commit()?;
        self.filing_state(account)
    }

    pub fn set_bootstrap_done(&mut self, account: &str, done: bool) -> Result<()> {
        self.db.execute(
            "INSERT INTO filing_state(account,bootstrap_done) VALUES(?1,?2) ON CONFLICT(account) DO UPDATE SET bootstrap_done=excluded.bootstrap_done",
            params![account, done],
        )?;
        Ok(())
    }

    pub fn set_last_pass(&mut self, account: &str, summary: &Value) -> Result<()> {
        self.db.execute(
            "INSERT INTO filing_state(account,last_pass) VALUES(?1,?2) ON CONFLICT(account) DO UPDATE SET last_pass=excluded.last_pass",
            params![account, summary.to_string()],
        )?;
        Ok(())
    }

    pub fn placement(&self, account: &str, id: &str) -> Result<Option<Placement>> {
        Ok(self
            .db
            .query_row(
                &format!(
                    "SELECT {PLACEMENT_COLUMNS} FROM placements WHERE account=? AND message_id=?"
                ),
                params![account, id],
                row_placement,
            )
            .optional()?)
    }

    pub fn placements(&self, account: &str) -> Result<Vec<Placement>> {
        let mut st = self.db.prepare(&format!(
            "SELECT {PLACEMENT_COLUMNS} FROM placements WHERE account=? ORDER BY message_id"
        ))?;
        let rows = st
            .query_map([account], row_placement)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Creates the placement of a fingerprinted message with an occurrence and
    /// no placement yet. Home: the first source folder by `sources` order, then
    /// lowest UID; else the lowest (folder, UID). Returns whether it created one.
    pub fn ensure_placement(
        &mut self,
        account: &str,
        id: &str,
        sources: &[String],
    ) -> Result<bool> {
        let tx = self.db.transaction()?;
        let created = insert_placement(&tx, account, id, sources)?;
        if created {
            bump(&tx)?;
            tx.commit()?;
        }
        Ok(created)
    }

    /// Bootstrap (spec "Identity and placements"): `ensure_placement` for every
    /// source-managed message with a fingerprint and an occurrence, in one
    /// transaction. Returns how many placements it created.
    pub fn bootstrap_placements(&mut self, account: &str, sources: &[String]) -> Result<usize> {
        let tx = self.db.transaction()?;
        let ids: Vec<String> = {
            let mut st = tx.prepare("SELECT id FROM messages m WHERE account=? AND source_managed=1 AND fingerprint IS NOT NULL
 AND NOT EXISTS(SELECT 1 FROM placements p WHERE p.message_id=m.id)
 AND EXISTS(SELECT 1 FROM occurrences o WHERE o.message_id=m.id) ORDER BY observed_at, id")?;
            let rows = st
                .query_map([account], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut created = 0;
        for id in &ids {
            if insert_placement(&tx, account, id, sources)? {
                created += 1;
            }
        }
        if created > 0 {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(created)
    }

    /// Placements awaiting hydration (message `size IS NULL`) whose known home
    /// occurrence is in its folder's checkpoint epoch, at most `per_folder`
    /// per home folder, lowest UIDs first.
    pub fn unhydrated_homes(
        &self,
        account: &str,
        per_folder: usize,
    ) -> Result<Vec<HydrationBatch>> {
        let mut st = self.db.prepare("SELECT p.home_folder,p.home_epoch,p.home_uid,p.message_id FROM placements p
 JOIN messages m ON m.id=p.message_id
 JOIN checkpoints c ON c.account=p.account AND c.mailbox=p.home_folder AND c.epoch=p.home_epoch
 WHERE p.account=? AND m.size IS NULL AND p.location_state='known'
 AND EXISTS(SELECT 1 FROM occurrences o WHERE o.account=p.account AND o.mailbox=p.home_folder AND o.epoch=p.home_epoch AND o.uid=p.home_uid AND o.message_id=p.message_id)
 ORDER BY p.home_folder, p.home_uid")?;
        let rows = st
            .query_map([account], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out: Vec<HydrationBatch> = Vec::new();
        for (folder, epoch, uid, id) in rows {
            match out.last_mut() {
                Some(batch) if batch.folder == folder => {
                    if batch.members.len() < per_folder {
                        batch.members.push((uid, id));
                    }
                }
                _ => out.push(HydrationBatch {
                    folder,
                    epoch,
                    members: vec![(uid, id)],
                }),
            }
        }
        Ok(out)
    }

    /// Whether the rescan of `folder` in `epoch` finished (spec "Folders"): its
    /// checkpoint reached `below_uid - 1` in that epoch and no message occurring
    /// in it still waits for content with a non-terminal job.
    pub fn rescan_finished(
        &self,
        account: &str,
        folder: &str,
        epoch: u64,
        below_uid: u64,
    ) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE account=?1 AND mailbox=?2 AND epoch=?3 AND last_uid>=?4)
 AND NOT EXISTS(SELECT 1 FROM occurrences o JOIN messages m ON m.id=o.message_id JOIN jobs j ON j.message_id=m.id
  WHERE o.account=?1 AND o.mailbox=?2 AND m.normalized IS NULL AND j.state<>'terminal')",
            params![account, folder, epoch, below_uid.saturating_sub(1)],
            |r| r.get(0),
        )?)
    }

    /// `queued`, minus messages that still need a fetch whose fetch locator
    /// (the occurrence `locator` picks) is in `blocked`; those stay queued
    /// without a lease or attempt.
    pub fn queued_outside(
        &self,
        account: &str,
        limit: usize,
        blocked: &BTreeSet<String>,
    ) -> Result<Vec<String>> {
        if blocked.is_empty() {
            return self.queued(account, limit);
        }
        let mut st = self.db.prepare("SELECT j.message_id FROM jobs j JOIN messages m ON m.id=j.message_id WHERE m.account=?1
 AND ((j.state IN ('queued','retry') AND j.next_after<=?2) OR (j.state='leased' AND j.lease_until<=?2))
 AND NOT (m.normalized IS NULL AND EXISTS(SELECT 1 FROM json_each(?3) b WHERE b.value=
  (SELECT mailbox FROM occurrences o WHERE o.account=m.account AND o.message_id=m.id ORDER BY mailbox LIMIT 1)))
 ORDER BY m.observed_at,j.message_id LIMIT ?4")?;
        let rows = st
            .query_map(
                params![
                    account,
                    now(),
                    serde_json::to_string(blocked)?,
                    limit as i64
                ],
                |r| r.get(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Folders named by any rescan-set row.
    pub fn rescan_folders(&self, account: &str) -> Result<BTreeSet<String>> {
        let mut st = self
            .db
            .prepare("SELECT DISTINCT folder FROM rescan_sets WHERE account=?")?;
        let rows = st
            .query_map([account], |r| r.get(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(rows)
    }

    /// Writes every field of an existing placement; with `Some(rev)` only when
    /// the stored `desired_rev` still equals `rev`. Returns whether it wrote.
    pub fn save_placement(&mut self, p: &Placement, expected_rev: Option<i64>) -> Result<bool> {
        let tx = self.db.transaction()?;
        let n = tx.execute(
            "UPDATE placements SET source_folder=?3,home_folder=?4,home_epoch=?5,home_uid=?6,location_state=?7,absent_since=?8,desired_target=?9,pinned=?10,eligible_once=?11,desired_rev=?12,filed_at=?13,filed_by=?14,flag_attempted_at=?15,flagged_at=?16,done_inferred=?17,blocked_reason=?18 WHERE account=?1 AND message_id=?2 AND (?19 IS NULL OR desired_rev=?19)",
            params![
                p.account,
                p.message_id,
                p.source_folder,
                p.home_folder,
                p.home_epoch,
                p.home_uid,
                p.location_state.as_str(),
                p.absent_since,
                p.desired_target,
                p.pinned,
                p.eligible_once,
                p.desired_rev,
                p.filed_at,
                p.filed_by,
                p.flag_attempted_at,
                p.flagged_at,
                p.done_inferred,
                p.blocked_reason,
                expected_rev
            ],
        )?;
        if n == 1 {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(n == 1)
    }

    pub fn folder_record(&self, account: &str, native: &str) -> Result<Option<FolderRecord>> {
        Ok(self
            .db
            .query_row(
                &format!("SELECT {FOLDER_COLUMNS} FROM folders WHERE account=? AND native=?"),
                params![account, native],
                row_folder,
            )
            .optional()?)
    }

    pub fn folder_records(&self, account: &str) -> Result<Vec<FolderRecord>> {
        let mut st = self.db.prepare(&format!(
            "SELECT {FOLDER_COLUMNS} FROM folders WHERE account=? ORDER BY native"
        ))?;
        let rows = st
            .query_map([account], row_folder)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Inserts or replaces every field of a folder record.
    pub fn save_folder(&mut self, f: &FolderRecord) -> Result<()> {
        self.db.execute(
            &format!("INSERT INTO folders({FOLDER_COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
 ON CONFLICT(account,native) DO UPDATE SET configured=excluded.configured,category_id=excluded.category_id,origin=excluded.origin,state=excluded.state,role_verified=excluded.role_verified,confirmed=excluded.confirmed,subscribed=excluded.subscribed,pause_reason=excluded.pause_reason,epoch=excluded.epoch,watch_from_uid=excluded.watch_from_uid,rescan_epoch=excluded.rescan_epoch,rescan_below_uid=excluded.rescan_below_uid,rescan_complete=excluded.rescan_complete,checked_at=excluded.checked_at,error=excluded.error"),
            params![
                f.account,
                f.native,
                f.configured,
                f.category_id,
                f.origin,
                f.state,
                f.role_verified,
                f.confirmed,
                f.subscribed,
                f.pause_reason,
                f.epoch,
                f.watch_from_uid,
                f.rescan_epoch,
                f.rescan_below_uid,
                f.rescan_complete,
                f.checked_at,
                f.error
            ],
        )?;
        Ok(())
    }

    /// `stage` plus filing: transport metadata columns, COPYUID-known targets
    /// joining their message, the rescan filter, and arrival rows. Returns the
    /// number of new (provisional) messages.
    #[allow(clippy::too_many_arguments)] // One atomic mailbox discovery checkpoint.
    pub fn stage_with(
        &mut self,
        account: &str,
        mailbox: &str,
        epoch: u64,
        through: u64,
        envelopes: &[SourceEnvelope],
        generation: &str,
        complete: bool,
        opts: &StageOptions<'_>,
    ) -> Result<usize> {
        let tx = self.db.transaction()?;
        let mut inserted = 0;
        let current: Option<u64> = tx
            .query_row(
                "SELECT epoch FROM checkpoints WHERE account=? AND mailbox=?",
                params![account, mailbox],
                |r| r.get(0),
            )
            .optional()?;
        if current != Some(epoch) {
            bail!("mailbox epoch changed before checkpoint commit");
        }
        for env in envelopes {
            if env.uid > through {
                bail!("source returned UID beyond requested window");
            }
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM occurrences WHERE account=? AND mailbox=? AND epoch=? AND uid=?)",params![account,mailbox,epoch,env.uid],|r|r.get(0))?;
            if exists {
                continue;
            }
            let rfc = rfc_message_id(env);
            let (id, intent_id) = if let Some((id, intent)) = opts.known_targets.get(&env.uid) {
                (id.clone(), Some(*intent))
            } else {
                if opts
                    .rescan_filter
                    .is_some_and(|f| env.uid < f.below_uid && !f.rfc_ids.contains(&rfc))
                {
                    continue;
                }
                let id = format!("msg_{}", Uuid::new_v4().simple());
                tx.execute("INSERT INTO messages(id,account,envelope,status,observed_at,generation,source_managed,rfc_message_id,size,internal_date) VALUES(?,?,?,'pending',?,?,1,?,?,?)",params![id,account,serde_json::to_string(env)?,now(),generation,rfc,env.size,env.internal_date])?;
                tx.execute(
                    "INSERT INTO jobs(message_id,state,next_after,generation) VALUES(?,'queued',?,?)",
                    params![id, now(), generation],
                )?;
                inserted += 1;
                (id, None)
            };
            tx.execute(
                "INSERT INTO occurrences VALUES(?,?,?,?,?)",
                params![account, mailbox, epoch, env.uid, id],
            )?;
            if opts.record_arrivals {
                tx.execute("INSERT INTO arrivals(account,folder,epoch,uid,message_id,rfc_message_id,intent_id,created_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(account,folder,epoch,uid) DO UPDATE SET message_id=excluded.message_id,rfc_message_id=excluded.rfc_message_id,state='pending',kind=NULL,intent_id=excluded.intent_id,created_at=excluded.created_at,resolved_at=NULL",params![account,mailbox,epoch,env.uid,id,rfc,intent_id,now()])?;
            }
        }
        tx.execute("UPDATE checkpoints SET last_uid=?,scanned_at=?,complete=?,error=NULL WHERE account=? AND mailbox=? AND epoch=?",params![through,now(),complete,account,mailbox,epoch])?;
        bump(&tx)?;
        tx.commit()?;
        Ok(inserted)
    }

    /// First watch of a folder starts at `start_uid`; an existing checkpoint
    /// behaves exactly like `checkpoint`. The epoch-0/UID-0 row `scan_error`
    /// leaves behind for a folder never scanned counts as no checkpoint.
    pub fn checkpoint_start_at(
        &mut self,
        account: &str,
        mailbox: &str,
        snapshot: &MailboxSnapshot,
        start_uid: u64,
    ) -> Result<u64> {
        let tx = self.db.transaction()?;
        let created = tx.execute(
            "INSERT INTO checkpoints(account,mailbox,epoch,last_uid,complete) VALUES(?,?,?,?,0)
 ON CONFLICT(account,mailbox) DO UPDATE SET epoch=excluded.epoch,last_uid=excluded.last_uid,complete=0,error=NULL
 WHERE checkpoints.epoch=0 AND checkpoints.last_uid=0",
            params![account, mailbox, snapshot.uid_validity, start_uid],
        )?;
        if created == 0 {
            drop(tx);
            return self.checkpoint(account, mailbox, snapshot);
        }
        bump(&tx)?;
        tx.commit()?;
        Ok(start_uid)
    }

    /// Arrivals oldest first, optionally only those in `state`.
    pub fn arrivals(&self, account: &str, state: Option<&str>) -> Result<Vec<Arrival>> {
        let mut st = self.db.prepare(&format!(
            "SELECT {ARRIVAL_COLUMNS} FROM arrivals WHERE account=?1 AND (?2 IS NULL OR state=?2) ORDER BY id"
        ))?;
        let rows = st
            .query_map(params![account, state], row_arrival)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Sets an arrival's state and kind; `resolved_at` is `now` unless the
    /// arrival goes back to `pending`.
    pub fn resolve_arrival(
        &mut self,
        id: i64,
        state: &str,
        kind: Option<&str>,
        now: &str,
    ) -> Result<()> {
        let n = self.db.execute(
            "UPDATE arrivals SET state=?2,kind=?3,resolved_at=CASE ?2 WHEN 'pending' THEN NULL ELSE ?4 END WHERE id=?1",
            params![id, state, kind, now],
        )?;
        if n == 0 {
            bail!("unknown arrival");
        }
        Ok(())
    }

    pub fn insert_intent(&mut self, i: &NewIntent<'_>) -> Result<i64> {
        self.db.execute(
            "INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid_next,desired_rev,consumes_eligible,batch,state,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
            params![
                i.account,
                i.message_id,
                i.kind,
                i.folder,
                i.epoch,
                i.uid,
                i.target,
                i.target_epoch,
                i.target_uid_next,
                i.desired_rev,
                i.consumes_eligible,
                i.batch,
                i.state,
                i.now
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Test hook: lets the retry backoff of every open intent elapse.
    #[doc(hidden)]
    pub fn expire_intent_backoff_for_tests(&mut self, account: &str) -> Result<()> {
        self.db.execute(
            &format!(
                "UPDATE filing_intents SET next_after='1970-01-01T00:00:00+00:00' WHERE account=? AND state IN {}",
                open_states_sql()
            ),
            [account],
        )?;
        Ok(())
    }

    /// Intents oldest first; `open_only` keeps `OPEN_INTENT_STATES`.
    pub fn intents(&self, account: &str, open_only: bool) -> Result<Vec<Intent>> {
        let filter = if open_only {
            format!(" AND state IN {}", open_states_sql())
        } else {
            String::new()
        };
        let mut st = self.db.prepare(&format!(
            "SELECT {INTENT_COLUMNS} FROM filing_intents WHERE account=?{filter} ORDER BY id"
        ))?;
        let rows = st
            .query_map([account], row_intent)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn intent(&self, id: i64) -> Result<Option<Intent>> {
        Ok(self
            .db
            .query_row(
                &format!("SELECT {INTENT_COLUMNS} FROM filing_intents WHERE id=?"),
                [id],
                row_intent,
            )
            .optional()?)
    }

    /// Re-claims an open move intent for a retry in one immediate transaction:
    /// refused (`false`) when the intent is no longer open, the placement's
    /// `desired_rev` is no longer `intent.desired_rev`, or the placement is
    /// blocked. Writes `in_flight` with `intent`'s source locator (`epoch`,
    /// `uid`), target snapshot (`target_epoch`, `target_uid_next`), `attempts`
    /// and `next_after`, sets `dispatched_at = now`, and clears the previous
    /// attempt's `target_uid` and `error`.
    pub fn reclaim_move(&mut self, intent: &Intent, now: &str) -> Result<bool> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ok: bool = tx
            .query_row(
                &format!(
                    "SELECT i.kind='move' AND i.state IN {} AND p.desired_rev=?2 AND p.blocked_reason IS NULL
 FROM filing_intents i JOIN placements p ON p.account=i.account AND p.message_id=i.message_id WHERE i.id=?1",
                    open_states_sql()
                ),
                params![intent.id, intent.desired_rev],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !ok {
            return Ok(false);
        }
        tx.execute(
            "UPDATE filing_intents SET state='in_flight',epoch=?2,uid=?3,target_epoch=?4,target_uid_next=?5,target_uid=NULL,attempts=?6,next_after=?7,dispatched_at=?8,updated_at=?8,error=NULL WHERE id=?1",
            params![
                intent.id,
                intent.epoch,
                intent.uid,
                intent.target_epoch,
                intent.target_uid_next,
                intent.attempts,
                intent.next_after,
                now
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The checkpoint of a folder: (epoch, last UID, complete, scanned_at).
    pub fn checkpoint_state(&self, account: &str, folder: &str) -> Result<Option<CheckpointState>> {
        Ok(self
            .db
            .query_row(
                "SELECT epoch,last_uid,complete,scanned_at FROM checkpoints WHERE account=? AND mailbox=?",
                params![account, folder],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?)
    }

    /// Arrivals in one folder epoch at `uid >= min_uid`, lowest UID first.
    pub fn arrivals_at(
        &self,
        account: &str,
        folder: &str,
        epoch: u64,
        min_uid: u64,
    ) -> Result<Vec<Arrival>> {
        let mut st = self.db.prepare(&format!(
            "SELECT {ARRIVAL_COLUMNS} FROM arrivals WHERE account=? AND folder=? AND epoch=? AND uid>=? ORDER BY uid, id"
        ))?;
        let rows = st
            .query_map(params![account, folder, epoch, min_uid], row_arrival)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Every placement with its message (without content) and transport
    /// metadata, in message id order.
    pub fn records_for_planning(
        &self,
        account: &str,
    ) -> Result<Vec<(Record, Placement, MessageMeta)>> {
        let placement: Vec<String> = PLACEMENT_COLUMNS
            .split(',')
            .map(|c| format!("p.{c}"))
            .collect();
        let mut st = self.db.prepare(&format!(
            "SELECT m.id,m.account,NULL,m.envelope,m.status,m.classification,m.overrides,m.review_state,m.observed_at,m.error,m.generation,{},
 m.rfc_message_id,m.size,m.internal_date,m.fingerprint IS NOT NULL,m.source_managed
 FROM placements p JOIN messages m ON m.id=p.message_id WHERE p.account=? ORDER BY p.message_id",
            placement.join(",")
        ))?;
        let rows = st
            .query_map([account], |r| {
                let record = row_record(r)?;
                let placement = row_placement_at(r, 11)?;
                let meta = MessageMeta {
                    rfc_message_id: r.get(29)?,
                    size: r.get(30)?,
                    internal_date: r.get(31)?,
                    flags: envelope_flags(&record.envelope),
                    fingerprinted: r.get(32)?,
                    source_managed: r.get(33)?,
                };
                Ok((record, placement, meta))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Sets the state and `updated_at`; patch fields that are `Some` overwrite.
    pub fn update_intent(
        &mut self,
        id: i64,
        state: &str,
        patch: IntentPatch,
        now: &str,
    ) -> Result<()> {
        let n = self.db.execute(
            "UPDATE filing_intents SET state=?2,updated_at=?3,target_uid=COALESCE(?4,target_uid),attempts=COALESCE(?5,attempts),next_after=COALESCE(?6,next_after),dispatched_at=COALESCE(?7,dispatched_at),error=COALESCE(?8,error) WHERE id=?1",
            params![
                id,
                state,
                now,
                patch.target_uid,
                patch.attempts,
                patch.next_after,
                patch.dispatched_at,
                patch.error
            ],
        )?;
        if n == 0 {
            bail!("unknown filing intent");
        }
        Ok(())
    }

    /// Claims a move in one transaction: refused (`None`) when the placement's
    /// `desired_rev` moved on, it is blocked, or a move intent is already open.
    pub fn claim_move(
        &mut self,
        account: &str,
        action: &Action,
        target_epoch: u64,
        target_uid_next: u64,
        batch: &str,
        now: &str,
    ) -> Result<Option<i64>> {
        let Action::Move {
            message_id,
            from,
            to,
            desired_rev,
            consumes_eligible,
        } = action
        else {
            bail!("claim_move needs a Move")
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ok: bool = tx
            .query_row(
                &format!(
                    "SELECT desired_rev=? AND blocked_reason IS NULL AND NOT EXISTS(SELECT 1 FROM filing_intents
           WHERE message_id=placements.message_id AND kind='move' AND state IN {})
         FROM placements WHERE account=? AND message_id=?",
                    open_states_sql()
                ),
                params![desired_rev, account, message_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !ok {
            return Ok(None);
        }
        tx.execute("INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid_next,desired_rev,consumes_eligible,batch,state,dispatched_at,created_at,updated_at)
                VALUES(?,?,'move',?,?,?,?,?,?,?,?,?,'in_flight',?,?,?)",
            params![account, message_id, from.folder, from.epoch, from.uid, to, target_epoch, target_uid_next, desired_rev, consumes_eligible, batch, now, now, now])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }

    /// Claims the one flag attempt a placement ever gets: refused (`None`) when
    /// `flag_attempted_at` is set, it is blocked, or a flag intent is open.
    pub fn claim_flag(
        &mut self,
        account: &str,
        action: &Action,
        batch: &str,
        now: &str,
    ) -> Result<Option<i64>> {
        let Action::Flag { message_id, at } = action else {
            bail!("claim_flag needs a Flag")
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rev: Option<i64> = tx
            .query_row(
                &format!(
                    "SELECT desired_rev FROM placements WHERE account=? AND message_id=? AND flag_attempted_at IS NULL AND blocked_reason IS NULL
                     AND NOT EXISTS(SELECT 1 FROM filing_intents WHERE message_id=placements.message_id AND kind='flag' AND state IN {})",
                    open_states_sql()
                ),
                params![account, message_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(rev) = rev else {
            return Ok(None);
        };
        tx.execute(
            "UPDATE placements SET flag_attempted_at=? WHERE account=? AND message_id=?",
            params![now, account, message_id],
        )?;
        tx.execute("INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,desired_rev,batch,state,dispatched_at,created_at,updated_at)
                VALUES(?1,?2,'flag',?3,?4,?5,?6,?7,'in_flight',?8,?8,?8)",
            params![account, message_id, at.folder, at.epoch, at.uid, rev, batch, now])?;
        let id = tx.last_insert_rowid();
        bump(&tx)?;
        tx.commit()?;
        Ok(Some(id))
    }

    /// Inserts every field of `r` except `id`; returns the new id.
    pub fn insert_revert(&mut self, r: &Revert) -> Result<i64> {
        self.db.execute(
            "INSERT INTO filing_reverts(account,parent_intent,folder,folder_epoch,uid,target,target_epoch,state,target_uid,created_at,updated_at,error) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                r.account,
                r.parent_intent,
                r.folder,
                r.folder_epoch,
                r.uid,
                r.target,
                r.target_epoch,
                r.state,
                r.target_uid,
                r.created_at,
                r.updated_at,
                r.error
            ],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Reverts oldest first; `open_only` keeps `pending` and `in_flight`.
    pub fn reverts(&self, account: &str, open_only: bool) -> Result<Vec<Revert>> {
        let mut st = self.db.prepare(&format!(
            "SELECT {REVERT_COLUMNS} FROM filing_reverts WHERE account=?1 AND (?2=0 OR state IN ('pending','in_flight')) ORDER BY id"
        ))?;
        let rows = st
            .query_map(params![account, open_only], row_revert)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Sets the state and `updated_at`; `target_uid` and `error` overwrite when `Some`.
    pub fn update_revert(
        &mut self,
        id: i64,
        state: &str,
        target_uid: Option<u64>,
        error: Option<&str>,
        now: &str,
    ) -> Result<()> {
        let n = self.db.execute(
            "UPDATE filing_reverts SET state=?2,target_uid=COALESCE(?3,target_uid),error=COALESCE(?4,error),updated_at=?5 WHERE id=?1",
            params![id, state, target_uid, error, now],
        )?;
        if n == 0 {
            bail!("unknown filing revert");
        }
        Ok(())
    }

    /// Appends one audit event.
    pub fn record_event(
        &mut self,
        account: &str,
        message_id: Option<&str>,
        folder: Option<&str>,
        kind: &str,
        detail: Value,
        now: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO filing_events(account,message_id,folder,at,kind,detail) VALUES(?,?,?,?,?,?)",
            params![account, message_id, folder, now, kind, detail.to_string()],
        )?;
        Ok(())
    }

    /// Events newest first, optionally only those of one message.
    pub fn events(
        &self,
        account: &str,
        message_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut st = self.db.prepare(
            "SELECT id,message_id,folder,at,kind,detail FROM filing_events WHERE account=?1 AND (?2 IS NULL OR message_id=?2) ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = st
            .query_map(params![account, message_id, limit as i64], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "message_id": r.get::<_, Option<String>>(1)?,
                    "folder": r.get::<_, Option<String>>(2)?,
                    "at": r.get::<_, String>(3)?,
                    "kind": r.get::<_, String>(4)?,
                    "detail": decode_json(5, r.get(5)?)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Rescan-set members of a folder epoch: (message id, rfc Message-ID).
    pub fn rescan_members(
        &self,
        account: &str,
        folder: &str,
        epoch: u64,
    ) -> Result<Vec<(String, Option<String>)>> {
        let mut st = self.db.prepare("SELECT r.message_id,m.rfc_message_id FROM rescan_sets r LEFT JOIN messages m ON m.id=r.message_id WHERE r.account=? AND r.folder=? AND r.epoch=? ORDER BY r.message_id")?;
        let rows = st
            .query_map(params![account, folder, epoch], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Occurrences of a message: (folder, epoch, uid).
    pub fn occurrences_of(&self, account: &str, id: &str) -> Result<Vec<(String, u64, u64)>> {
        let mut st = self.db.prepare("SELECT mailbox,epoch,uid FROM occurrences WHERE account=? AND message_id=? ORDER BY mailbox,uid")?;
        let rows = st
            .query_map(params![account, id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn remove_occurrence(
        &mut self,
        account: &str,
        folder: &str,
        epoch: u64,
        uid: u64,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        let n = tx.execute(
            "DELETE FROM occurrences WHERE account=? AND mailbox=? AND epoch=? AND uid=?",
            params![account, folder, epoch, uid],
        )?;
        if n > 0 {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Transport metadata columns plus `flags` from the envelope JSON.
    pub fn message_meta(&self, account: &str, id: &str) -> Result<Option<MessageMeta>> {
        let row = self
            .db
            .query_row(
                "SELECT rfc_message_id,size,internal_date,fingerprint IS NOT NULL,source_managed,envelope FROM messages WHERE account=? AND id=?",
                params![account, id],
                |r| {
                    let meta = MessageMeta {
                        rfc_message_id: r.get(0)?,
                        size: r.get(1)?,
                        internal_date: r.get(2)?,
                        flags: vec![],
                        fingerprinted: r.get(3)?,
                        source_managed: r.get(4)?,
                    };
                    Ok((meta, decode_json(5, r.get(5)?)?))
                },
            )
            .optional()?;
        Ok(row.map(|(meta, envelope)| MessageMeta {
            flags: envelope_flags(&envelope),
            ..meta
        }))
    }

    /// Stores freshly fetched transport metadata: the three columns, and
    /// `message_id`, `internal_date`, `size`, `flags` merged into the envelope.
    /// A field the envelope lacks (a blank Message-ID counts as lacking) keeps
    /// the stored value.
    pub fn hydrate(&mut self, account: &str, id: &str, env: &SourceEnvelope) -> Result<()> {
        let tx = self.db.transaction()?;
        let Some(mut envelope) = envelope_of(&tx, account, id)? else {
            bail!("unknown message");
        };
        let rfc = rfc_message_id(env);
        let fresh = serde_json::to_value(env)?;
        let keys: Vec<&str> = ["message_id", "internal_date", "size", "flags"]
            .into_iter()
            .filter(|k| fresh.get(*k).is_some_and(|v| !v.is_null()))
            .filter(|k| *k != "message_id" || rfc.is_some())
            .collect();
        merge_envelope(&mut envelope, &fresh, &keys);
        tx.execute(
            "UPDATE messages SET rfc_message_id=COALESCE(?,rfc_message_id),size=COALESCE(?,size),internal_date=COALESCE(?,internal_date),envelope=? WHERE account=? AND id=?",
            params![
                rfc,
                env.size,
                env.internal_date,
                envelope.to_string(),
                account,
                id
            ],
        )?;
        bump(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// `reconcile_range`, returning the message ids whose occurrence it removed.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_range_ids(
        &mut self,
        account: &str,
        mailbox: &str,
        epoch: u64,
        after: u64,
        through: u64,
        present: &[u64],
        finished: bool,
    ) -> Result<Vec<String>> {
        let tx = self.db.transaction()?;
        let actual: Option<u64> = tx
            .query_row(
                "SELECT epoch FROM checkpoints WHERE account=? AND mailbox=?",
                params![account, mailbox],
                |r| r.get(0),
            )
            .optional()?;
        if actual != Some(epoch) {
            bail!("mailbox epoch changed during reconciliation");
        }
        if present.iter().any(|u| *u <= after || *u > through) {
            bail!("invalid reconciliation UID");
        }
        let known: Vec<(u64, String)> = {
            let mut stmt=tx.prepare("SELECT uid,message_id FROM occurrences WHERE account=? AND mailbox=? AND epoch=? AND uid>? AND uid<=? ORDER BY uid")?;
            let rows = stmt
                .query_map(params![account, mailbox, epoch, after, through], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut removed = Vec::new();
        for (uid, id) in known {
            if !present.contains(&uid)
                && tx.execute(
                    "DELETE FROM occurrences WHERE account=? AND mailbox=? AND epoch=? AND uid=?",
                    params![account, mailbox, epoch, uid],
                )? > 0
            {
                removed.push(id);
            }
        }
        tx.execute("INSERT INTO reconciliation VALUES(?,?,?,?) ON CONFLICT(account,mailbox) DO UPDATE SET epoch=excluded.epoch,cursor=excluded.cursor",params![account,mailbox,epoch,if finished {0}else{through}])?;
        // Locally retained messages remain readable, but vanished, unfetched sources cannot retry forever.
        tx.execute("UPDATE jobs SET state='terminal',lease_until=NULL WHERE message_id IN (SELECT id FROM messages WHERE account=? AND source_managed=1 AND normalized IS NULL AND NOT EXISTS(SELECT 1 FROM occurrences WHERE message_id=messages.id))",[account])?;
        if !removed.is_empty() {
            tx.execute("UPDATE messages SET status='failed',error='message left watched source before content was fetched' WHERE account=? AND source_managed=1 AND normalized IS NULL AND NOT EXISTS(SELECT 1 FROM occurrences WHERE message_id=messages.id)",[account])?;
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(removed)
    }
}

/// `ensure_placement` inside a caller's transaction; does not bump the revision.
fn insert_placement(tx: &Connection, account: &str, id: &str, sources: &[String]) -> Result<bool> {
    let eligible: bool = tx
        .query_row(
            "SELECT fingerprint IS NOT NULL AND NOT EXISTS(SELECT 1 FROM placements WHERE message_id=messages.id)
         FROM messages WHERE account=? AND id=?",
            params![account, id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false);
    if !eligible {
        return Ok(false);
    }
    let mut occ: Vec<(String, u64, u64)> = {
        let mut st = tx.prepare("SELECT mailbox, epoch, uid FROM occurrences WHERE account=? AND message_id=? ORDER BY mailbox, uid")?;
        let rows = st
            .query_map(params![account, id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if occ.is_empty() {
        return Ok(false);
    }
    let rank = |f: &str| sources.iter().position(|s| s == f).unwrap_or(usize::MAX);
    occ.sort_by(|a, b| {
        rank(&a.0)
            .cmp(&rank(&b.0))
            .then(a.0.cmp(&b.0))
            .then(a.2.cmp(&b.2))
    });
    let home = &occ[0];
    let source = if rank(&home.0) != usize::MAX {
        home.0.clone()
    } else {
        sources.first().cloned().unwrap_or_else(|| "INBOX".into())
    };
    tx.execute("INSERT INTO placements(account, message_id, source_folder, home_folder, home_epoch, home_uid) VALUES(?,?,?,?,?,?)",
        params![account, id, source, home.0, home.1, home.2])?;
    Ok(true)
}

fn parse_mode(s: &str) -> Result<FilingMode> {
    Ok(match s {
        "off" => FilingMode::Off,
        "dry_run" => FilingMode::DryRun,
        "live" => FilingMode::Live,
        _ => bail!("invalid filing mode in state"),
    })
}

fn decode_json(column: usize, s: String) -> rusqlite::Result<Value> {
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn row_placement(r: &Row<'_>) -> rusqlite::Result<Placement> {
    row_placement_at(r, 0)
}

/// A placement whose `PLACEMENT_COLUMNS` start at column `at`.
fn row_placement_at(r: &Row<'_>, at: usize) -> rusqlite::Result<Placement> {
    let location: String = r.get(at + 6)?;
    Ok(Placement {
        account: r.get(at)?,
        message_id: r.get(at + 1)?,
        source_folder: r.get(at + 2)?,
        home_folder: r.get(at + 3)?,
        home_epoch: r.get(at + 4)?,
        home_uid: r.get(at + 5)?,
        location_state: LocationState::parse(&location).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                at + 6,
                rusqlite::types::Type::Text,
                "invalid location_state".into(),
            )
        })?,
        absent_since: r.get(at + 7)?,
        desired_target: r.get(at + 8)?,
        pinned: r.get(at + 9)?,
        eligible_once: r.get(at + 10)?,
        desired_rev: r.get(at + 11)?,
        filed_at: r.get(at + 12)?,
        filed_by: r.get(at + 13)?,
        flag_attempted_at: r.get(at + 14)?,
        flagged_at: r.get(at + 15)?,
        done_inferred: r.get(at + 16)?,
        blocked_reason: r.get(at + 17)?,
    })
}

/// The `flags` array of an envelope JSON object.
fn envelope_flags(envelope: &Value) -> Vec<String> {
    envelope
        .get("flags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn row_folder(r: &Row<'_>) -> rusqlite::Result<FolderRecord> {
    Ok(FolderRecord {
        account: r.get(0)?,
        native: r.get(1)?,
        configured: r.get(2)?,
        category_id: r.get(3)?,
        origin: r.get(4)?,
        state: r.get(5)?,
        role_verified: r.get(6)?,
        confirmed: r.get(7)?,
        subscribed: r.get(8)?,
        pause_reason: r.get(9)?,
        epoch: r.get(10)?,
        watch_from_uid: r.get(11)?,
        rescan_epoch: r.get(12)?,
        rescan_below_uid: r.get(13)?,
        rescan_complete: r.get(14)?,
        checked_at: r.get(15)?,
        error: r.get(16)?,
    })
}

fn row_arrival(r: &Row<'_>) -> rusqlite::Result<Arrival> {
    Ok(Arrival {
        id: r.get(0)?,
        account: r.get(1)?,
        folder: r.get(2)?,
        epoch: r.get(3)?,
        uid: r.get(4)?,
        message_id: r.get(5)?,
        rfc_message_id: r.get(6)?,
        state: r.get(7)?,
        kind: r.get(8)?,
        intent_id: r.get(9)?,
        created_at: r.get(10)?,
        resolved_at: r.get(11)?,
    })
}

fn row_intent(r: &Row<'_>) -> rusqlite::Result<Intent> {
    Ok(Intent {
        id: r.get(0)?,
        account: r.get(1)?,
        message_id: r.get(2)?,
        kind: r.get(3)?,
        folder: r.get(4)?,
        epoch: r.get(5)?,
        uid: r.get(6)?,
        target: r.get(7)?,
        target_epoch: r.get(8)?,
        target_uid_next: r.get(9)?,
        target_uid: r.get(10)?,
        desired_rev: r.get(11)?,
        consumes_eligible: r.get(12)?,
        batch: r.get(13)?,
        state: r.get(14)?,
        attempts: r.get(15)?,
        next_after: r.get(16)?,
        dispatched_at: r.get(17)?,
        created_at: r.get(18)?,
        updated_at: r.get(19)?,
        error: r.get(20)?,
    })
}

fn row_revert(r: &Row<'_>) -> rusqlite::Result<Revert> {
    Ok(Revert {
        id: r.get(0)?,
        account: r.get(1)?,
        parent_intent: r.get(2)?,
        folder: r.get(3)?,
        folder_epoch: r.get(4)?,
        uid: r.get(5)?,
        target: r.get(6)?,
        target_epoch: r.get(7)?,
        state: r.get(8)?,
        target_uid: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        error: r.get(12)?,
    })
}
