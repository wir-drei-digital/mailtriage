//! `mailtriage-tray`: a thin client of the `mailtriage` CLI. `model` holds
//! the logic and is tested without a display; `tray` and `editor` only draw
//! it and send actions back.
pub mod args;
pub mod autostart;
pub mod cli;
pub mod model;
pub mod paths;
