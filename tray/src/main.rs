use clap::Parser;
use mailtriage_tray::{
    args::{Args, Sub},
    autostart, editor, tray,
};

fn main() {
    let args = Args::parse();
    let code = match &args.command {
        None => tray::run(&args),
        Some(Sub::Autostart { action, json }) => autostart::run(&args, *action, *json),
        Some(Sub::Categories { account }) => editor::run(&args, account.clone()),
    };
    std::process::exit(code);
}
