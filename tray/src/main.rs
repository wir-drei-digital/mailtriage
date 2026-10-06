use clap::Parser;
use mailtriage_tray::{
    args::{Args, Sub},
    autostart,
};

fn main() {
    let args = Args::parse();
    let code = match &args.command {
        Some(Sub::Autostart { action, json }) => autostart::run(&args, *action, *json),
        // The tray (Task 7) and the window (Task 8).
        _ => 0,
    };
    std::process::exit(code);
}
