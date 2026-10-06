use clap::Parser;
use mailtriage_tray::{
    args::{Args, Sub},
    autostart, tray,
};

fn main() {
    let args = Args::parse();
    let code = match &args.command {
        None => tray::run(&args),
        Some(Sub::Autostart { action, json }) => autostart::run(&args, *action, *json),
        // The categories window (Task 8).
        Some(Sub::Categories { .. }) => 0,
    };
    std::process::exit(code);
}
