//! The window: Services, Options and Logs panes over the control app's snapshot. Dark or light
//! follows the system (egui's default theme preference), the scale follows the monitor's DPI,
//! every text comes from the catalog.

use crate::link::Link;
use ams2_connector_ui::api::{Command, OptionsChange, Snapshot};
use ams2_connector_ui::config::{HZ_MAX, HZ_MIN, LOGS};
use ams2_connector_ui::icons::level_rgb;
use ams2_connector_ui::logtail::{LevelFilter, LogTail};
use ams2_connector_ui::texts::Texts;
use ams2_i18n::Arg;
use eframe::egui;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How many log lines "Copy last N" copies.
const COPY_LINES: usize = 50;
/// The window redraws at least this often while open (connection state, logs).
const TICK: Duration = Duration::from_secs(1);

/// The panes, in the order of the navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The services' switches and states.
    Services,
    /// The options.
    Options,
    /// The log viewer.
    Logs,
}

/// The egui application.
pub struct Window {
    link: Link,
    texts: Texts,
    page: Page,
    /// The last `window_request` sequence acted on (`None` until the first snapshot).
    seen_request: Option<u64>,
    announced: bool,
    log: String,
    tail: Option<(PathBuf, LogTail)>,
    last_poll: Instant,
    alerts_only: bool,
    hz: Option<f64>,
    exiting: bool,
    closing: bool,
}

impl Window {
    /// A window on `link`, opening on `page`.
    pub fn new(link: Link, texts: Texts, page: Page) -> Self {
        Self {
            link,
            texts,
            page,
            seen_request: None,
            announced: false,
            log: String::new(),
            tail: None,
            last_poll: Instant::now() - TICK,
            alerts_only: false,
            hz: None,
            exiting: false,
            closing: false,
        }
    }

    fn s(&self, key: &str) -> String {
        self.texts.s(key)
    }

    fn follow_requests(&mut self, ctx: &egui::Context, snap: &Snapshot) {
        let req = &snap.0["window_request"];
        let seq = req["seq"].as_u64().unwrap_or(0);
        match self.seen_request {
            None => {
                self.seen_request = Some(seq);
                if self.log.is_empty() {
                    self.log = snap.0["config"]["logs_default"]
                        .as_str()
                        .filter(|l| LOGS.contains(l))
                        .unwrap_or("recorder")
                        .to_string();
                }
            }
            Some(seen) if seq > seen => {
                self.seen_request = Some(seq);
                match req["action"].as_str() {
                    Some("logs") => self.page = Page::Logs,
                    Some("hide") => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                    _ => {}
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(_) => {}
        }
    }

    fn nav(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.add_space(6.0);
        for (page, key) in [
            (Page::Services, "page.services"),
            (Page::Options, "page.options"),
            (Page::Logs, "page.logs"),
        ] {
            let label = self.s(key);
            if ui.selectable_label(self.page == page, label).clicked() {
                self.page = page;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(6.0);
            if !snap.managed() {
                let text = if self.exiting {
                    self.s("status.stopping")
                } else {
                    self.s("exit.button")
                };
                let button = ui
                    .add_enabled(!self.exiting, egui::Button::new(text))
                    .on_hover_text(self.s("exit.tip"));
                if button.clicked() {
                    self.exiting = true;
                    self.link.send(Command::Exit {
                        origin: "window".into(),
                    });
                }
            }
        });
    }

    fn services(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        if snap.0["exiting"] == true {
            ui.label(self.s("services.stopping"));
        }
        let services = snap.0["services"].as_array().cloned().unwrap_or_default();
        for svc in &services {
            let id = svc["id"].as_str().unwrap_or_default();
            let name = svc["name"].as_str().unwrap_or(id);
            let mut on = svc["running"].as_bool().unwrap_or(false);
            let busy = snap.0["exiting"] == true || svc["state"] == "stopping";
            ui.horizontal(|ui| {
                let [r, g, b] = level_rgb(svc["level"].as_str().unwrap_or("off"));
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 5.0, egui::Color32::from_rgb(r, g, b));
                if ui
                    .add_enabled(!busy, egui::Checkbox::new(&mut on, name))
                    .changed()
                {
                    self.link.send(Command::Switch {
                        service: id.to_string(),
                        on,
                    });
                }
            });
            ui.indent(id, |ui| {
                ui.label(svc["label"].as_str().unwrap_or_default());
                for line in self.detail_lines(snap, svc) {
                    ui.weak(line);
                }
            });
            ui.add_space(8.0);
        }
        ui.separator();
        let folder = snap.0["data_dir"].as_str().unwrap_or_default();
        ui.label(
            self.texts
                .t("services.data_folder", &[("folder", folder.into())]),
        );
        if ui.button(self.s("services.open_data_folder")).clicked() {
            self.link.send(Command::OpenDataFolder);
        }
    }

    fn detail_lines(&self, snap: &Snapshot, svc: &Value) -> Vec<String> {
        let mut out = Vec::new();
        if !svc["running"].as_bool().unwrap_or(false) {
            return out;
        }
        out.push(self.s(if snap.0["game_running"] == true {
            "detail.game_running"
        } else {
            "detail.game_not_running"
        }));
        out.push(self.s(match snap.0["pitwall"].as_str() {
            Some("connected") => "status.pitwall.connected",
            Some("refused") => "status.pitwall.refused",
            _ => "status.pitwall.absent",
        }));
        let outbox = &snap.0["outbox"];
        if let Some(n) = outbox["pending"].as_i64().filter(|n| *n > 0) {
            out.push(self.texts.t("detail.outbox", &[("n", Arg::Int(n))]));
        }
        if let Some(n) = outbox["rejected"].as_i64().filter(|n| *n > 0) {
            out.push(self.texts.t("detail.rejected", &[("n", Arg::Int(n))]));
        }
        if let Some(pid) = svc["pid"].as_u64() {
            out.push(
                self.texts
                    .t("detail.pid", &[("pid", pid.to_string().into())]),
            );
        }
        out
    }

    fn options(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        let cfg = &snap.0["config"];
        if !snap.managed() {
            let mut on = snap.0["autostart"].as_bool().unwrap_or(false);
            if ui.checkbox(&mut on, self.s("options.autostart")).changed() {
                self.link.send(Command::SetAutostart(on));
            }
            ui.weak(self.s("options.autostart.note"));
            ui.add_space(10.0);
        }
        let mut minimized = cfg["start_minimized"].as_bool().unwrap_or(true);
        if ui
            .checkbox(&mut minimized, self.s("options.minimized"))
            .changed()
        {
            self.link.send(Command::SetOptions(OptionsChange {
                start_minimized: Some(minimized),
                ..Default::default()
            }));
        }
        ui.weak(self.s("options.minimized.note"));
        ui.add_space(10.0);
        ui.label(self.s("options.hz"));
        let mut hz = self
            .hz
            .unwrap_or_else(|| cfg["recorder_hz"].as_f64().unwrap_or(50.0));
        let response = ui.add(
            egui::DragValue::new(&mut hz)
                .range(HZ_MIN..=HZ_MAX)
                .suffix(self.s("options.hz.suffix")),
        );
        if response.dragged() || response.has_focus() {
            self.hz = Some(hz);
        }
        if response.drag_stopped() || response.lost_focus() {
            self.hz = None;
            self.link.send(Command::SetOptions(OptionsChange {
                recorder_hz: Some(hz.round()),
                ..Default::default()
            }));
        }
        ui.weak(self.s("options.hz.note"));
        ui.add_space(10.0);
        for service in ams2_connector_ui::SERVICES {
            let on = cfg["start_with_app"][service]
                .as_bool()
                .unwrap_or(service == "recorder");
            let name = ams2_connector_ui::status::display_name(&self.texts, service);
            let mut shown = on;
            ui.add_enabled(
                false,
                egui::Checkbox::new(
                    &mut shown,
                    self.texts.t("options.start", &[("service", name.into())]),
                ),
            );
        }
        ui.weak(self.s("options.start.note"));
    }

    fn logs(&mut self, ui: &mut egui::Ui, snap: &Snapshot) {
        ui.horizontal(|ui| {
            ui.label(self.s("logs.selector"));
            for name in LOGS {
                let label = if name == "ui" {
                    self.s("logs.ui")
                } else {
                    self.s("service.recorder")
                };
                if ui.selectable_label(self.log == name, label).clicked() {
                    self.log = name.to_string();
                    self.tail = None;
                }
            }
            let alerts = self.s("logs.alerts_only");
            ui.checkbox(&mut self.alerts_only, alerts);
        });
        let dir = PathBuf::from(snap.0["data_dir"].as_str().unwrap_or_default());
        let path = dir.join(format!("{}.log", self.log));
        if self.tail.as_ref().is_none_or(|(p, _)| *p != path) {
            let mut tail = LogTail::new(&path);
            tail.open();
            self.tail = Some((path.clone(), tail));
        } else if self.last_poll.elapsed() >= TICK {
            if let Some((_, tail)) = &mut self.tail {
                tail.poll();
            }
            self.last_poll = Instant::now();
        }
        let Some((_, tail)) = &self.tail else { return };
        let mut filter = LevelFilter::default();
        let shown: Vec<&str> = if self.alerts_only {
            tail.lines
                .iter()
                .map(String::as_str)
                .filter(|l| filter.accept(l))
                .collect()
        } else {
            tail.lines.iter().map(String::as_str).collect()
        };
        ui.horizontal(|ui| {
            if ui
                .button(
                    self.texts
                        .t("logs.copy", &[("n", Arg::Int(COPY_LINES as i64))]),
                )
                .clicked()
            {
                let from = shown.len().saturating_sub(COPY_LINES);
                ui.ctx().copy_text(shown[from..].join("\n"));
            }
            if ui.button(self.s("logs.open")).clicked() {
                let _ = std::process::Command::new("explorer.exe")
                    .arg(&path)
                    .spawn();
            }
        });
        if !tail.exists {
            ui.weak(self.texts.t(
                "logs.none_yet",
                &[("file", path.display().to_string().into())],
            ));
        }
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &shown {
                    ui.add(egui::Label::new(egui::RichText::new(*line).monospace()).extend());
                }
            });
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(TICK);
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            self.closing = true;
            if self.announced {
                let _ = self
                    .link
                    .send_and_wait(Command::WindowVisible(false), Duration::from_secs(1));
            }
            self.link.close();
        }
        let Some(snap) = self.link.snapshot() else {
            let key = if self.link.connected() {
                "status.starting"
            } else {
                "status.pitwall.absent"
            };
            let _ = key;
            ui.label(self.s("status.starting"));
            return;
        };
        if !self.announced {
            self.announced = true;
            self.link.send(Command::WindowVisible(true));
        }
        self.follow_requests(&ctx, &snap);
        if let Some(error) = self.link.take_error() {
            ui.colored_label(egui::Color32::from_rgb(0xd1, 0x34, 0x38), error);
        }
        egui::Panel::left("nav")
            .resizable(false)
            .show(ui, |ui| self.nav(ui, &snap));
        egui::CentralPanel::default().show(ui, |ui| match self.page {
            Page::Services => self.services(ui, &snap),
            Page::Options => self.options(ui, &snap),
            Page::Logs => self.logs(ui, &snap),
        });
    }
}
