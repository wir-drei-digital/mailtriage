//! Command-line arguments of `mailtriage-tray`.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "mailtriage-tray",
    version,
    about = "Menu bar and tray app for mailtriage"
)]
pub struct Args {
    /// mailtriage's config file (default: the one `mailtriage service status` finds).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// The mailtriage program (default: next to this program, else on PATH).
    #[arg(long, global = true)]
    pub mailtriage: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Sub>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Sub {
    /// Open the categories window.
    Categories {
        /// The account to show first (default: the first account).
        #[arg(long)]
        account: Option<String>,
    },
    /// Start the tray when you log in.
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
        /// Print JSON.
        #[arg(long, global = true)]
        json: bool,
    },
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartAction {
    /// Start the tray at login.
    Enable,
    /// Stop starting the tray at login.
    Disable,
    /// Whether the tray starts at login.
    Status,
}
