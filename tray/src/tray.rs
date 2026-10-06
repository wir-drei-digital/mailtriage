//! The tray process: an icon and a menu drawn from `model::menu` on the
//! main thread's `tao` event loop (which also drives GTK on Linux). Clicks
//! and worker answers reach the loop through an `EventLoopProxy`; every
//! native call happens on the main thread. No logic tests need lives here.
use crate::{
    args::Args,
    autostart,
    controller::{self, Controller, Done, Flags, Work},
    icons,
    instances::{self, Windows, WINDOWS_ENV},
    model::{
        health::IconState,
        menu::{Entry, Menu},
    },
    paths,
    restart::{self, Restarter},
};
use chrono::{Local, Utc};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};
use tao::{
    event::{Event, StartCause},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
};
use tray_icon::{
    menu::{
        CheckMenuItem, IsMenuItem, Menu as NativeMenu, MenuEvent, MenuItem, PredefinedMenuItem,
        Submenu,
    },
    TrayIcon, TrayIconBuilder,
};

enum UserEvent {
    Menu(String),
    Done(Box<Done>),
}

/// Appends `entries` through `append`; each clickable entry's id is its
/// action's id, which a click resolves against the menu shown at that
/// moment (`Menu::action`).
fn add(entries: &[Entry], append: &mut dyn FnMut(&dyn IsMenuItem)) {
    for entry in entries {
        match entry {
            Entry::Text(text) => append(&MenuItem::new(text, false, None)),
            Entry::Item { label, action } => {
                append(&MenuItem::with_id(action.id(), label, true, None));
            }
            Entry::Check {
                label,
                checked,
                action,
            } => {
                append(&CheckMenuItem::with_id(
                    action.id(),
                    label,
                    true,
                    *checked,
                    None,
                ));
            }
            Entry::Separator => append(&PredefinedMenuItem::separator()),
            Entry::Submenu { label, entries } => {
                let submenu = Submenu::new(label, true);
                add(entries, &mut |item| {
                    let _ = submenu.append(item);
                });
                append(&submenu);
            }
        }
    }
}

fn draw(menu: &Menu) -> NativeMenu {
    let native = NativeMenu::new();
    add(&menu.entries, &mut |item| {
        let _ = native.append(item);
    });
    native
}

/// The tray icon. Without it the tray would be invisible yet keep
/// `tray.lock`, so a failure ends the loop with exit code 3, which
/// releases the lock.
fn icon_or_exit(control_flow: &mut ControlFlow) -> Option<TrayIcon> {
    match TrayIconBuilder::new().with_tooltip("mailtriage").build() {
        Ok(icon) => Some(icon),
        Err(e) => {
            eprintln!("mailtriage-tray: cannot show the tray icon: {e}");
            *control_flow = ControlFlow::ExitWithCode(3);
            None
        }
    }
}

fn set_icon(tray: &TrayIcon, state: IconState) {
    #[cfg(target_os = "macos")]
    let _ = tray.set_icon_templated(Some(icons::icon(state)));
    #[cfg(not(target_os = "macos"))]
    let _ = tray.set_icon(Some(icons::icon(state)));
}

/// `mailtriage-tray` without a subcommand. Returns only when it cannot
/// start (0 when another tray runs).
pub fn run(args: &Args) -> i32 {
    let env = paths::Env::current();
    let Some(cache) = paths::cache_dir(env.home.as_deref(), env.xdg_cache_home.as_deref()) else {
        eprintln!("mailtriage-tray: HOME is not set");
        return 3;
    };
    let lock = match instances::tray_lock(&cache) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            println!("mailtriage-tray is already running");
            return 0;
        }
        Err(e) => {
            eprintln!("mailtriage-tray: {}: {e}", cache.display());
            return 3;
        }
    };
    let windows = Windows::from_env(std::env::var(WINDOWS_ENV).ok().as_deref());
    let restarter = Restarter::start(controller::TRAY_VERSION);
    let tray_exe = restarter
        .as_ref()
        .map(|r| r.path().to_path_buf())
        .or_else(|| env.own_exe.clone())
        .unwrap_or_else(|| PathBuf::from("mailtriage-tray"));
    let controller = Controller::new(
        Flags {
            mailtriage: args.mailtriage.clone(),
            config: args.config.clone(),
        },
        env,
        tray_exe,
        autostart::Env::detect().ok(),
        windows,
    );
    event_loop(controller, restarter, lock)
}

fn spawn(
    work: Work,
    controller: &Controller,
    proxy: &EventLoopProxy<UserEvent>,
    running: &Arc<AtomicUsize>,
) {
    let counted = !matches!(work, Work::Window(_));
    let ctx = controller.context();
    let proxy = proxy.clone();
    let running = Arc::clone(running);
    if counted {
        running.fetch_add(1, Ordering::SeqCst);
    }
    thread::spawn(move || {
        controller::run(work, &ctx, &|done| {
            let _ = proxy.send_event(UserEvent::Done(Box::new(done)));
        });
        if counted {
            running.fetch_sub(1, Ordering::SeqCst);
        }
    });
}

fn event_loop(
    mut controller: Controller,
    mut restarter: Option<Restarter>,
    lock: std::fs::File,
) -> ! {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    {
        let proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = proxy.send_event(UserEvent::Menu(event.id.0));
        }));
    }
    let running = Arc::new(AtomicUsize::new(0));
    let mut clipboard = arboard::Clipboard::new().ok();
    let mut tray: Option<TrayIcon> = None;
    let mut shown: Option<Menu> = None;
    let mut next_refresh = Instant::now() + controller::REFRESH_EVERY;
    let _lock = lock;
    event_loop.run(move |event, _, control_flow| {
        let now = Utc::now();
        let mut work = vec![];
        match event {
            // The icon exists only once the loop runs.
            Event::NewEvents(StartCause::Init) => {
                tray = icon_or_exit(control_flow);
                if tray.is_none() {
                    return;
                }
                work.extend([Work::Refresh, Work::AutostartStatus]);
            }
            // A click on a menu redrawn since resolves by its id: to the
            // same action, or to nothing when the item is gone.
            Event::UserEvent(UserEvent::Menu(id)) => {
                if let Some(action) = shown.as_ref().and_then(|menu| menu.action(&id)) {
                    work = controller.act(action, now);
                }
            }
            Event::UserEvent(UserEvent::Done(done)) => work = controller.done(*done, now),
            _ => {}
        }
        if Instant::now() >= next_refresh {
            next_refresh = Instant::now() + controller::REFRESH_EVERY;
            controller.windows.reap();
            controller.expire(now);
            if controller.idle() && running.load(Ordering::SeqCst) == 0 {
                if let (Some(r), Ok(resolved)) = (restarter.as_mut(), &controller.paths) {
                    if let Some(path) = r.check(Instant::now()) {
                        tray = None;
                        let error =
                            restart::exec(restart::command(&path, resolved, &controller.windows));
                        // Still the old code: show the tray again and try
                        // the file later, as after a failed probe.
                        eprintln!("mailtriage-tray: restart failed: {error}");
                        r.exec_failed(Instant::now());
                        tray = icon_or_exit(control_flow);
                        shown = None;
                        if tray.is_none() {
                            return;
                        }
                    }
                }
            }
            work.push(Work::Refresh);
        }
        for item in work {
            match item {
                Work::Copy(text) => {
                    if let Some(clipboard) = clipboard.as_mut() {
                        let _ = clipboard.set_text(text);
                    }
                }
                Work::Quit => *control_flow = ControlFlow::Exit,
                other => spawn(other, &controller, &proxy, &running),
            }
        }
        let menu = controller.menu(now, Local::now().fixed_offset());
        if let (Some(icon), true) = (&tray, shown.as_ref() != Some(&menu)) {
            icon.set_menu(Some(Box::new(draw(&menu))));
            let _ = icon.set_tooltip(Some(&menu.summary));
            if shown.as_ref().map(|m| m.icon) != Some(menu.icon) {
                set_icon(icon, menu.icon);
            }
            shown = Some(menu);
        }
        if !matches!(*control_flow, ControlFlow::ExitWithCode(_)) {
            *control_flow = ControlFlow::WaitUntil(next_refresh);
        }
    })
}
