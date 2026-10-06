//! The update work inside `watch`, between passes: before each pass and
//! every 5 s while waiting, the restart check. Update errors are printed as
//! events and never fail or end a pass.
use super::{
    events,
    install::{EnvHooks, Hooks},
    restart::{Image, Restarter},
};
use std::path::Path;

pub struct WatchUpdates {
    restarter: Restarter,
    hooks: Box<dyn Hooks>,
}

impl WatchUpdates {
    /// Records the installation path and image first, as `watch` must
    /// before anything else.
    pub fn start(json_mode: bool) -> Self {
        let image = Image::record();
        Self {
            restarter: Restarter::new(image, Box::new(move |e| events::emit(json_mode, e))),
            hooks: Box::new(EnvHooks),
        }
    }

    /// The restart check before a pass of the config at `_config`.
    pub fn before_pass(&mut self, _config: &Path, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }

    /// The restart check alone, for the wait between passes.
    pub fn while_waiting(&mut self, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }
}
