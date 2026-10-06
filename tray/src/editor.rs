//! The categories window: draws `model::window` with egui and runs the
//! commands it asks for on worker threads, so the window never freezes.
//! AccessKit is on: every control has an accessible label, which screen
//! readers, tests and agents use.
use crate::{
    args::Args,
    cli::{self, Cli, Failure, Request},
    instances::{self, EditorLock},
    model::{
        window::{Check, Dialog, Effect, Field, Msg, Notice, Options, Refile, Running, Window},
        words,
    },
    paths,
};
use chrono::Utc;
use eframe::egui::{
    self, pos2, text::LayoutJob, Align, Button, CollapsingHeader, Color32, ComboBox, FontId, Frame,
    Id, Key, KeyboardShortcut, Layout, Margin, Modal, Modifiers, Rect, RichText, ScrollArea,
    Spinner, TextEdit, TextFormat, TextStyle, UiBuilder, ViewportCommand, WidgetInfo, WidgetType,
};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicI32, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
    time::Instant,
};

/// Width of the form's text fields.
pub const FIELD_WIDTH: f32 = 400.0;
const LIST_WIDTH: f32 = 220.0;
const REFILE_HEIGHT: f32 = 232.0;
/// The margins of the header, the footer and the refile panel.
const BAR_MARGIN: Margin = Margin::symmetric(16, 8);
/// Why a control is off while a load runs.
const WAIT_LOADING: &str = "Wait until loading finishes";
/// Why a control is off while a command that writes runs.
const WAIT_ACTION: &str = "Wait until the current action finishes";

/// What the window needs to work.
pub struct Setup {
    pub cli: Cli,
    pub account: Option<String>,
    pub options: Options,
    /// The private directory (0700) for draft files.
    pub drafts: PathBuf,
    /// Held while the window is open: the window lock and the temporary
    /// directory, removed when the window closes.
    pub lock: Option<File>,
    pub temp: Option<tempfile::TempDir>,
}

/// The eframe app of the window.
pub struct Editor {
    model: Window,
    cli: Option<Cli>,
    drafts: PathBuf,
    _lock: Option<File>,
    /// Removed on exit, and when the editor is dropped.
    temp: Option<tempfile::TempDir>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    exit: Arc<AtomicI32>,
    allow_close: bool,
    closed: bool,
    /// The footer's height and the width of its buttons, as measured in
    /// the last pass (the footer is laid out before it is drawn).
    footer_height: f32,
    actions_width: f32,
}

/// Text 14 pt, headings 18 pt, margins and gaps on an 8 px grid.
pub fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(16.0, 4.0);
        style.spacing.interact_size.y = 24.0;
        style.spacing.window_margin = Margin::same(16);
        style.spacing.menu_margin = Margin::same(8);
        style.spacing.indent = 16.0;
        style.spacing.icon_spacing = 8.0;
    });
}

/// The private (0700) temporary directory for a window's draft files.
pub fn drafts_dir() -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .prefix("mailtriage-tray-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
}

impl Editor {
    /// A window that loads at once, or that shows why it cannot (`Err`).
    pub fn new(
        cc: &eframe::CreationContext,
        setup: Result<Setup, String>,
        exit: Arc<AtomicI32>,
    ) -> Self {
        style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let mut editor = match setup {
            Ok(setup) => Self {
                model: Window::new(setup.options, setup.account),
                cli: Some(setup.cli),
                drafts: setup.drafts,
                _lock: setup.lock,
                temp: setup.temp,
                tx,
                rx,
                ctx: cc.egui_ctx.clone(),
                exit,
                allow_close: false,
                closed: false,
                footer_height: 24.0,
                actions_width: 0.0,
            },
            Err(message) => Self {
                model: Window::failed(message),
                cli: None,
                drafts: PathBuf::new(),
                _lock: None,
                temp: None,
                tx,
                rx,
                ctx: cc.egui_ctx.clone(),
                exit,
                allow_close: false,
                closed: false,
                footer_height: 24.0,
                actions_width: 0.0,
            },
        };
        if editor.cli.is_some() {
            editor.dispatch(Msg::Start);
        }
        editor.exit.store(editor.model.exit_code, Ordering::SeqCst);
        editor
    }

    pub fn model(&self) -> &Window {
        &self.model
    }

    /// The window asked to close (tests have no real window).
    pub fn closed(&self) -> bool {
        self.closed
    }

    fn draft_file(&self, revision: u64) -> PathBuf {
        self.drafts.join(format!("draft-{revision}.json"))
    }

    fn dispatch(&mut self, msg: Msg) {
        let effects = self.model.update(msg, Instant::now());
        self.execute(effects);
        self.exit.store(self.model.exit_code, Ordering::SeqCst);
    }

    /// Runs `job` off the UI thread; its answer comes back as a message.
    fn spawn(&self, job: impl FnOnce(&Cli) -> Msg + Send + 'static) {
        let Some(cli) = self.cli.clone() else {
            return;
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(job(&cli));
            ctx.request_repaint();
        });
    }

    fn execute(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Status { load } => self.spawn(move |cli| Msg::Status {
                    load,
                    at: Utc::now(),
                    result: cli::status(&cli.run(&Request::Status)),
                }),
                Effect::Export { load, account } => self.spawn(move |cli| Msg::Exported {
                    load,
                    result: cli::categories(&cli.run(&Request::Export(account))),
                }),
                Effect::Check {
                    account,
                    revision,
                    body,
                } => {
                    let file = self.draft_file(revision);
                    self.spawn(move |cli| {
                        let result = match write_draft(&file, &body) {
                            Ok(()) => cli::validation(&cli.run(&Request::Validate {
                                account: account.clone(),
                                file,
                            })),
                            Err(e) => Err(Failure::new(
                                format!("Could not save the draft: {e}"),
                                cli::Details {
                                    command: String::new(),
                                    exit_code: None,
                                    output: String::new(),
                                },
                            )),
                        };
                        Msg::Checked {
                            account,
                            revision,
                            result,
                        }
                    });
                }
                Effect::Apply {
                    account,
                    revision,
                    digest,
                } => {
                    let file = self.draft_file(revision);
                    self.spawn(move |cli| Msg::Applied {
                        revision,
                        result: cli::done(&cli.run(&Request::Apply {
                            account,
                            file,
                            digest,
                        })),
                    });
                }
                Effect::Preview { account } => self.spawn(move |cli| {
                    let finished = cli.run(&Request::Refile {
                        account: account.clone(),
                        folder: None,
                        apply: false,
                    });
                    Msg::Previewed {
                        account,
                        result: cli::refile_preview(&finished),
                    }
                }),
                Effect::Refile { account, folder } => self.spawn(move |cli| {
                    let finished = cli.run(&Request::Refile {
                        account: account.clone(),
                        folder,
                        apply: true,
                    });
                    Msg::Moved {
                        account,
                        result: cli::refile_marked(&finished),
                    }
                }),
                Effect::MovedStatus { account } => self.spawn(move |cli| Msg::MovedStatus {
                    account,
                    at: Utc::now(),
                    result: cli::status(&cli.run(&Request::Status)),
                }),
                Effect::DeleteDraft(revision) => {
                    let _ = fs::remove_file(self.draft_file(revision));
                }
                Effect::Close => {
                    self.allow_close = true;
                    self.closed = true;
                    self.ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    /// Why the form, Add and Revert are off while the model is read-only.
    fn read_only_reason(&self) -> &'static str {
        if self.model.running.is_some() || self.model.closing {
            WAIT_ACTION
        } else {
            WAIT_LOADING
        }
    }

    /// Why the account selector and Reload are off.
    fn selector_reason(&self) -> &'static str {
        if self.model.running.is_some() || self.model.closing {
            WAIT_ACTION
        } else if self.model.loading() {
            WAIT_LOADING
        } else {
            "No accounts to choose from"
        }
    }

    /// Whether a refile command can start now; else why not.
    fn refile_blocked(&self) -> Option<&'static str> {
        if self.model.running.is_some() || self.model.closing {
            Some(WAIT_ACTION)
        } else if self.model.loading() {
            Some(WAIT_LOADING)
        } else {
            None
        }
    }
}

/// Writes a draft once, exclusively and private; a re-check of the same
/// revision reuses the file, which is never rewritten.
fn write_draft(path: &Path, body: &str) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()
}

/// A labelled single-line field; returns the new text when it changed.
/// While it cannot be edited, its tooltip says `why`.
fn text_field(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    hint: &str,
    editable: bool,
    why: &str,
) -> Option<String> {
    let label = ui.label(label);
    let mut text = value.to_owned();
    let response = ui
        .add_enabled_ui(editable, |ui| {
            ui.add_sized(
                [FIELD_WIDTH, 24.0],
                TextEdit::singleline(&mut text).hint_text(hint),
            )
        })
        .inner
        .labelled_by(label.id)
        .on_disabled_hover_text(why);
    response.changed().then_some(text)
}

/// A labelled multi-line field.
fn text_area(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    rows: usize,
    editable: bool,
    why: &str,
) -> Option<String> {
    let label = ui.label(label);
    let mut text = value.to_owned();
    let response = ui
        .add_enabled_ui(editable, |ui| {
            ui.add_sized(
                [FIELD_WIDTH, rows as f32 * 24.0],
                TextEdit::multiline(&mut text).desired_rows(rows),
            )
        })
        .inner
        .labelled_by(label.id)
        .on_disabled_hover_text(why);
    response.changed().then_some(text)
}

/// The primary button: white text on a blue dark enough for it in both
/// light and dark mode.
fn primary(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let fill = if ui.visuals().dark_mode {
        Color32::from_rgb(0x2a, 0x6e, 0xd6)
    } else {
        Color32::from_rgb(0x1f, 0x5f, 0xc7)
    };
    ui.add_enabled(
        enabled,
        Button::new(RichText::new(text).color(Color32::WHITE).strong()).fill(fill),
    )
}

/// A panel's frame with margins on the 8 px grid.
fn panel_frame(ui: &egui::Ui, margin: Margin) -> Frame {
    Frame::side_top_panel(ui.style()).inner_margin(margin)
}

/// Reserves a bottom panel with room for `inner_height` inside
/// `BAR_MARGIN` and returns that room. Its contents are drawn later, after
/// the form, so that Tab and screen readers meet them in the visual order
/// (egui follows the order in which controls are created).
fn reserve_bottom(ui: &mut egui::Ui, id: &'static str, inner_height: f32) -> Rect {
    egui::Panel::bottom(id)
        .frame(panel_frame(ui, BAR_MARGIN))
        .resizable(false)
        .exact_size(inner_height + BAR_MARGIN.sum().y)
        .show(ui, |ui| ui.max_rect())
        .inner
}

/// A child of `ui` that draws into `rect`, part of a reserved panel's room,
/// clipped to that panel.
fn reserved_ui(ui: &mut egui::Ui, salt: &str, rect: Rect, layout: Layout) -> egui::Ui {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(layout));
    child.set_clip_rect(ui.clip_rect().intersect(rect + BAR_MARGIN));
    child
}

impl Editor {
    fn header(&mut self, ui: &mut egui::Ui, msgs: &mut Vec<Msg>) {
        ui.horizontal(|ui| {
            let enabled = self.model.selector_enabled();
            let why = self.selector_reason();
            let current = self.model.account.clone().unwrap_or_default();
            let label = ui.label("Account");
            ui.add_enabled_ui(enabled, |ui| {
                ComboBox::from_id_salt("account")
                    .selected_text(&current)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for a in &self.model.accounts {
                            if ui.selectable_label(a.name == current, &a.name).clicked() {
                                msgs.push(Msg::SelectAccount(a.name.clone()));
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id)
                    .on_disabled_hover_text(why);
            });
            let reload = ui
                .add_enabled(enabled, Button::new("Reload"))
                .on_disabled_hover_text(why);
            if reload.clicked() {
                msgs.push(Msg::Reload);
            }
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, msgs: &mut Vec<Msg>) {
        ui.heading("Categories");
        let filing_on = self.model.filing_on();
        let weak = ui.visuals().weak_text_color();
        let strong = ui.visuals().text_color();
        ScrollArea::vertical()
            .max_height(ui.available_height() - 48.0)
            .show(ui, |ui| {
                ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                    for (i, c) in self.model.draft.iter().enumerate() {
                        // The name, then the mail folder and the default mark.
                        let mut second = vec![];
                        if filing_on {
                            second.push(if c.folder.trim().is_empty() {
                                c.name.clone()
                            } else {
                                c.folder.trim().to_owned()
                            });
                        }
                        if c.default {
                            second.push("default".to_owned());
                        }
                        let mut job = LayoutJob::default();
                        job.append(
                            &c.name,
                            0.0,
                            TextFormat::simple(FontId::proportional(14.0), strong),
                        );
                        if !second.is_empty() {
                            job.append(
                                &format!("\n{}", second.join(" · ")),
                                0.0,
                                TextFormat::simple(FontId::proportional(12.0), weak),
                            );
                        }
                        if ui
                            .add(Button::selectable(i == self.model.selected, job))
                            .clicked()
                        {
                            msgs.push(Msg::Select(i));
                        }
                    }
                });
                if self.model.draft.len() == 1 {
                    ui.label(RichText::new(words::EMPTY).weak());
                }
            });
        let add = ui
            .add_enabled(!self.model.read_only(), Button::new("Add"))
            .on_disabled_hover_text(self.read_only_reason());
        if add.clicked() {
            msgs.push(Msg::Add);
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, msgs: &mut Vec<Msg>) {
        if self.model.loading() {
            ui.horizontal(|ui| {
                ui.add(Spinner::new());
                ui.label("Loading…");
            });
        }
        let index = self.model.selected;
        let Some(c) = self.model.draft.get(index).cloned() else {
            return;
        };
        let filing_on = self.model.filing_on();
        let editable = !self.model.read_only();
        let why = self.read_only_reason();
        if let Some(v) = text_field(ui, "Name", &c.name, "", editable, why) {
            msgs.push(Msg::Edit(index, Field::Name, v));
        }
        if let Some(v) = text_area(ui, "What belongs here", &c.description, 3, editable, why) {
            msgs.push(Msg::Edit(index, Field::Description, v));
        }
        let mut default = c.default;
        let check = ui
            .add_enabled(
                editable && !c.default,
                egui::Checkbox::new(&mut default, "Use for mail that fits nowhere else"),
            )
            .on_disabled_hover_text(if editable {
                "This is the default category; choose another one to change it"
            } else {
                why
            });
        if check.changed() && default {
            msgs.push(Msg::SetDefault(index));
        }
        if filing_on {
            if let Some(v) = text_field(ui, "Mail folder", &c.folder, &c.name, editable, why) {
                msgs.push(Msg::Edit(index, Field::Folder, v));
            }
        }
        // Opening "Advanced" edits nothing, so it works while the form is
        // read-only; the fields inside follow the form.
        CollapsingHeader::new("Advanced")
            .default_open(false)
            .show(ui, |ui| {
                if c.existing {
                    ui.label(format!("ID: {}", c.id));
                    ui.label(
                        RichText::new("Keep IDs: corrections and folders refer to them").weak(),
                    );
                } else if let Some(v) = text_field(ui, "ID", &c.id, "", editable, why) {
                    msgs.push(Msg::Edit(index, Field::Id, v));
                }
                if let Some(v) =
                    text_area(ui, "Examples, one per line", &c.examples, 3, editable, why)
                {
                    msgs.push(Msg::Edit(index, Field::Examples, v));
                }
                ui.label(
                    RichText::new("Saved with the category; not sent to the classifier").weak(),
                );
            });
        ui.add_space(8.0);
        let remove = ui
            .add_enabled(editable && !c.default, Button::new("Remove category"))
            .on_disabled_hover_text(if editable {
                "Choose another default category first"
            } else {
                why
            });
        if remove.clicked() {
            msgs.push(Msg::RequestRemove(index));
        }
    }

    fn refile(&self, ui: &mut egui::Ui, refile: &Refile, msgs: &mut Vec<Msg>) {
        ui.heading("Move filed mail");
        if let Some(result) = &refile.result {
            ui.label(RichText::new(result).strong());
        }
        if let Some((message, details)) = &refile.error {
            ui.label(message);
            if ui.button("Show details").clicked() {
                msgs.push(Msg::ShowDetails(details.clone()));
            }
        }
        if refile.dry_run {
            ui.label(words::DRY_RUN);
        }
        let Some(preview) = &refile.preview else {
            if refile.error.is_none() {
                ui.horizontal(|ui| {
                    ui.add(Spinner::new());
                    ui.label("Loading…");
                });
            }
            return;
        };
        // Moving is never offered in dry run; otherwise it waits for a
        // running command or load, and says so.
        let blocked = self.refile_blocked();
        let can_move = blocked.is_none();
        let why = blocked.unwrap_or_default();
        for folder in preview.folders.iter().filter(|f| f.retired) {
            ui.horizontal_wrapped(|ui| {
                ui.label(words::retired_row(folder));
                if !refile.dry_run {
                    let label = format!("Move mail from {}", folder.name());
                    let button = ui
                        .add_enabled(can_move, Button::new("Move"))
                        .on_disabled_hover_text(why);
                    button
                        .widget_info(|| WidgetInfo::labeled(WidgetType::Button, can_move, &label));
                    if button.clicked() {
                        msgs.push(Msg::MoveFolder(folder.native.clone()));
                    }
                }
            });
        }
        if let Some(row) = words::all_row(preview) {
            ui.label(row);
            if !refile.dry_run
                && ui
                    .add_enabled(can_move, Button::new(format!("Move all {}", preview.total)))
                    .on_disabled_hover_text(why)
                    .clicked()
            {
                msgs.push(Msg::MoveAll);
            }
        }
        if let Some(row) = words::waiting_row(preview) {
            ui.label(row);
        }
        if preview.total == 0 && preview.waiting == 0 {
            ui.label(words::NOTHING_TO_MOVE);
        }
        let skipped = words::skipped_rows(preview);
        if !skipped.is_empty() {
            CollapsingHeader::new("Not moved")
                .default_open(false)
                .show(ui, |ui| {
                    for row in skipped {
                        ui.label(row);
                    }
                });
        }
    }

    /// The footer, drawn into its reserved `rect` after the rest of the
    /// window: the check result or notice on the left, then "Move filed
    /// mail…", Revert and Apply on the right, created in that reading order
    /// so Tab and screen readers end with Apply. The buttons' width and the
    /// footer's height come from the last pass; when either changes, egui
    /// draws the frame again at once.
    fn footer(&mut self, ui: &mut egui::Ui, rect: Rect, msgs: &mut Vec<Msg>) {
        let gap = ui.spacing().item_spacing.x;
        let row = ui.spacing().interact_size.y;
        let split = (rect.max.x - self.actions_width - gap).max(rect.min.x);
        // The status at its natural height: a long message wraps.
        let mut status = reserved_ui(
            ui,
            "footer-status",
            Rect::from_min_max(rect.min, pos2(split, rect.max.y)),
            Layout::top_down(Align::Min),
        );
        status.horizontal_wrapped(|ui| self.status_line(ui, msgs));
        // One row of buttons, centred on the footer's height.
        let mut actions = reserved_ui(
            ui,
            "footer-actions",
            Rect::from_min_size(
                pos2(rect.max.x - self.actions_width, rect.center().y - row / 2.0),
                egui::vec2(self.actions_width, row),
            ),
            Layout::left_to_right(Align::Center),
        );
        if self.model.filing_on() {
            let blocked = self.refile_blocked();
            let open = actions
                .add_enabled(blocked.is_none(), Button::new("Move filed mail…"))
                .on_disabled_hover_text(blocked.unwrap_or_default());
            if open.clicked() {
                msgs.push(Msg::MoveFiledMail);
            }
        }
        let revert = actions
            .add_enabled(
                self.model.dirty() && !self.model.read_only(),
                Button::new("Revert"),
            )
            .on_disabled_hover_text(if self.model.dirty() {
                self.read_only_reason()
            } else {
                "No changes to revert"
            });
        if revert.clicked() {
            msgs.push(Msg::Revert);
        }
        let blocked = self.model.apply_blocked();
        let apply = primary(&mut actions, "Apply", blocked.is_none())
            .on_disabled_hover_text(blocked.unwrap_or_default());
        if apply.clicked() {
            msgs.push(Msg::RequestApply);
        }
        let width = actions.min_rect().width();
        let height = status.min_rect().height().max(row);
        if (width - self.actions_width).abs() > 0.5 || (height - self.footer_height).abs() > 0.5 {
            self.actions_width = width;
            self.footer_height = height;
            ui.ctx().request_discard("the footer changed size");
        }
    }

    fn status_line(&self, ui: &mut egui::Ui, msgs: &mut Vec<Msg>) {
        let spin = |ui: &mut egui::Ui, verb: &str| {
            ui.add(Spinner::new());
            ui.label(verb);
        };
        match (&self.model.notice, self.model.running) {
            (Some(Notice::Finishing), _) => return spin(ui, "Finishing…"),
            (_, Some(Running::Applying { .. })) => {
                if self.model.notice == Some(Notice::BusyRetrying) {
                    return spin(ui, words::BUSY_RETRYING);
                }
                return spin(ui, "Applying…");
            }
            (_, Some(Running::Moving)) => return spin(ui, "Moving…"),
            _ => {}
        }
        if self.model.loading() {
            return spin(ui, "Loading…");
        }
        match &self.model.notice {
            Some(Notice::Saved(text)) => {
                ui.label(text);
                return;
            }
            Some(Notice::Busy(_)) => {
                ui.label(words::BUSY);
                let blocked = self.model.apply_blocked();
                let again = ui
                    .add_enabled(blocked.is_none(), Button::new("Try again"))
                    .on_disabled_hover_text(blocked.unwrap_or_default());
                if again.clicked() {
                    msgs.push(Msg::TryAgain);
                }
                return;
            }
            Some(Notice::Rechecking) => {
                ui.label(words::RECHECKING);
                return;
            }
            Some(Notice::Error { message, details }) => {
                ui.label(RichText::new(message).color(ui.visuals().error_fg_color));
                if let Some(details) = details {
                    if ui.button("Show details").clicked() {
                        msgs.push(Msg::ShowDetails(details.clone()));
                    }
                }
                return;
            }
            _ => {}
        }
        if self.model.checking() {
            return spin(ui, "Checking…");
        }
        if let Some(text) = self.model.check_text() {
            let invalid = matches!(self.model.check, Check::Invalid { .. });
            if invalid {
                ui.label(RichText::new(text).color(ui.visuals().error_fg_color));
                if let Some(details) = self.model.check_details() {
                    if ui.button("Show details").clicked() {
                        msgs.push(Msg::ShowDetails(details.clone()));
                    }
                }
            } else {
                ui.label(text);
            }
        }
    }

    fn dialog(&self, ctx: &egui::Context, msgs: &mut Vec<Msg>) {
        let Some(dialog) = &self.model.dialog else {
            return;
        };
        let frame = Frame::popup(&ctx.global_style()).inner_margin(16);
        let response = Modal::new(Id::new("dialog")).frame(frame).show(ctx, |ui| {
            ui.set_width(440.0);
            match dialog {
                Dialog::Discard { question, .. } => {
                    ui.heading(question);
                    ui.horizontal(|ui| {
                        if ui.button("Discard").clicked() {
                            msgs.push(Msg::Confirm);
                        }
                        if ui.button("Cancel").clicked() {
                            msgs.push(Msg::Cancel);
                        }
                    });
                }
                Dialog::ConfirmApply {
                    title,
                    lines,
                    warning,
                    ..
                } => {
                    ui.heading(title);
                    for line in lines {
                        ui.label(line);
                    }
                    if let Some(warning) = warning {
                        ui.label(RichText::new(warning).strong());
                    }
                    ui.horizontal(|ui| {
                        if primary(ui, "Apply changes", true).clicked() {
                            msgs.push(Msg::Confirm);
                        }
                        if ui.button("Cancel").clicked() {
                            msgs.push(Msg::Cancel);
                        }
                    });
                }
                Dialog::ConfirmRemove { question, .. } => {
                    ui.label(question);
                    ui.horizontal(|ui| {
                        if ui.button("Remove").clicked() {
                            msgs.push(Msg::Confirm);
                        }
                        if ui.button("Cancel").clicked() {
                            msgs.push(Msg::Cancel);
                        }
                    });
                }
                Dialog::CategoriesChanged => {
                    ui.heading(words::CATEGORIES_CHANGED);
                    ui.horizontal(|ui| {
                        if ui.button("Reload (discard my edits)").clicked() {
                            msgs.push(Msg::ReloadDiscarding);
                        }
                        if ui.button("Keep editing").clicked() {
                            msgs.push(Msg::KeepEditing);
                        }
                    });
                }
                Dialog::Details(details) => {
                    ui.heading("Details");
                    let mut text = details.text();
                    ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut text)
                                .font(TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(details.text());
                        }
                        if ui.button("Close").clicked() {
                            msgs.push(Msg::Cancel);
                        }
                    });
                }
            }
        });
        if response.should_close() {
            msgs.push(match dialog {
                Dialog::CategoriesChanged => Msg::KeepEditing,
                _ => Msg::Cancel,
            });
        }
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(msg) = self.rx.try_recv() {
            self.dispatch(msg);
        }
        self.dispatch(Msg::Tick);
        let mut msgs = vec![];
        let (add, save, close) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::N)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S)),
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::W)),
            )
        });
        // A dialog takes all input. egui's modal takes the keyboard focus
        // and the pointer from the layers below it; an AccessKit action
        // (a screen reader's click) still reaches the controls behind it,
        // so what they send while a dialog is open is dropped.
        let dialog_open = self.model.dialog.is_some();
        if !dialog_open {
            if add {
                msgs.push(Msg::Add);
            }
            if save {
                msgs.push(Msg::RequestApply);
            }
        }
        if close || ctx.input(|i| i.viewport().close_requested()) {
            if !self.allow_close {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            }
            msgs.push(Msg::CloseRequested);
        }
        let mut controls = vec![];
        // Controls are created in the visual order, which is the order Tab
        // and screen readers follow: header, list, form, refile panel, then
        // the footer from left to right. The footer and the refile panel
        // reserve their room first and are drawn after the form.
        egui::Panel::top("header")
            .frame(panel_frame(ui, BAR_MARGIN))
            .show(ui, |ui| self.header(ui, &mut controls));
        let footer = reserve_bottom(ui, "footer", self.footer_height);
        egui::Panel::left("list")
            .frame(panel_frame(ui, Margin::same(16)))
            .exact_size(LIST_WIDTH)
            .resizable(false)
            .show(ui, |ui| self.list(ui, &mut controls));
        // The refile panel sits below the form, above the footer.
        let refile = self.model.refile.clone().map(|refile| {
            let room = REFILE_HEIGHT - BAR_MARGIN.sum().y;
            (refile, reserve_bottom(ui, "refile", room))
        });
        egui::CentralPanel::default()
            .frame(Frame::central_panel(ui.style()).inner_margin(16))
            .show(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| self.form(ui, &mut controls));
            });
        if let Some((refile, rect)) = refile {
            let mut panel = reserved_ui(ui, "refile-contents", rect, Layout::top_down(Align::Min));
            ScrollArea::vertical().show(&mut panel, |ui| self.refile(ui, &refile, &mut controls));
        }
        self.footer(ui, footer, &mut controls);
        if !dialog_open {
            msgs.append(&mut controls);
        }
        // Messages first, so a dialog they open shows in this frame.
        for msg in std::mem::take(&mut msgs) {
            self.dispatch(msg);
        }
        self.dialog(&ctx, &mut msgs);
        for msg in msgs {
            self.dispatch(msg);
        }
        if let Some(due) = self.model.next_deadline() {
            ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
        }
    }

    /// The drafts go with the window: its temporary directory is removed
    /// here, and by dropping the editor should eframe not call this.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(temp) = self.temp.take() {
            let _ = temp.close();
        }
    }
}

/// `mailtriage-tray categories`: 0 when closed or when another window for
/// the config is open, 3 when mailtriage is missing or the first load
/// failed (after showing why).
pub fn run(args: &Args, account: Option<String>) -> i32 {
    let env = paths::Env::current();
    let exit = Arc::new(AtomicI32::new(0));
    let setup = match paths::resolve(args.mailtriage.as_deref(), args.config.as_deref(), &env) {
        Err(problem) => Err(problem.text()),
        Ok(resolved) => {
            let Some(cache) = paths::cache_dir(env.home.as_deref(), env.xdg_cache_home.as_deref())
            else {
                eprintln!("mailtriage-tray: HOME is not set");
                return 3;
            };
            // The lock check comes before any window: a second window for
            // the same config only brings the first to the front.
            match instances::editor_lock(&cache, &resolved.config) {
                Ok(EditorLock::HeldBy(pid)) => {
                    if let Some(pid) = pid {
                        instances::bring_to_front(pid);
                    }
                    println!("the categories window is already open");
                    return 0;
                }
                Err(e) => Err(format!("{}: {e}", cache.display())),
                Ok(EditorLock::Taken(lock)) => drafts_dir()
                    .map(|temp| Setup {
                        cli: resolved.cli(),
                        account,
                        options: Options::default(),
                        drafts: temp.path().to_path_buf(),
                        lock: Some(lock),
                        temp: Some(temp),
                    })
                    .map_err(|e| format!("cannot create a temporary directory: {e}")),
            }
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("mailtriage categories")
            .with_app_id("mailtriage-tray")
            .with_inner_size([720.0, 520.0])
            .with_min_inner_size([600.0, 420.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let app_exit = Arc::clone(&exit);
    if let Err(e) = eframe::run_native(
        "mailtriage categories",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, setup, app_exit)))),
    ) {
        eprintln!("mailtriage-tray: the window could not open: {e}");
        return 3;
    }
    exit.load(Ordering::SeqCst)
}
