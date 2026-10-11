//! The lock holder owns the runtime. Stop requests address that holder through
//! its directory, never a persisted PID. A dirty journal is written before
//! spawning a child; only confirmed group cleanup can mark it terminated.
use super::Layout;
use serde::{Deserialize, Serialize};
use std::{fs, io, io::Write};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ownership {
    owner: String,
    child_group: Option<u32>,
    terminated: bool,
}

pub(super) fn unconfirmed(layout: &Layout) -> io::Error {
    io::Error::other(format!(
        "local Kev shutdown is unconfirmed; see {} and resolve the previous runtime before mailtriage setup --update",
        layout.log().display()
    ))
}

/// Called while holding the exclusive lock. Unknown/legacy evidence is not
/// proof that a previous owner's children ended.
pub(super) fn confirm_terminated(layout: &Layout, lock: &fs::File) -> io::Result<()> {
    match fs::read(layout.ownership()) {
        Ok(bytes) => {
            let record: Ownership =
                serde_json::from_slice(&bytes).map_err(|_| unconfirmed(layout))?;
            if record.owner.is_empty() || !record.terminated {
                return Err(unconfirmed(layout));
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound && lock.metadata()?.len() == 0 => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(unconfirmed(layout)),
        Err(e) => Err(e),
    }
}

impl Ownership {
    pub(super) fn begin(layout: &Layout) -> io::Result<Self> {
        let record = Self {
            owner: uuid::Uuid::new_v4().to_string(),
            child_group: None,
            terminated: false,
        };
        record.write(layout)?;
        Ok(record)
    }

    pub(super) fn child(&mut self, layout: &Layout, group: u32) -> io::Result<()> {
        self.child_group = Some(group);
        self.write(layout)
    }

    /// The caller has either confirmed the group disappeared or never spawned
    /// a child. Errors retain unconfirmed evidence rather than invent success.
    pub(super) fn finish(&mut self, layout: &Layout) -> io::Result<()> {
        clear_request(layout)?;
        self.terminated = true;
        self.write(layout)
    }

    fn write(&self, layout: &Layout) -> io::Result<()> {
        let partial = layout.ownership().with_extension("json.partial");
        let mut file = fs::File::create(&partial)?;
        file.write_all(&serde_json::to_vec(self).map_err(io::Error::other)?)?;
        file.sync_all()?;
        fs::rename(partial, layout.ownership())
    }
}

pub(super) fn request_stop(layout: &Layout) -> io::Result<()> {
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.stop_request())?;
    file.sync_all()
}

pub(super) fn requested(layout: &Layout) -> io::Result<bool> {
    match fs::metadata(layout.stop_request()) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub(super) fn clear_request(layout: &Layout) -> io::Result<()> {
    match fs::remove_file(layout.stop_request()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
