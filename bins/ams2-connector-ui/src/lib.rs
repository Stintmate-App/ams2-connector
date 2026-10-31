//! The AMS2 connector's control app, without a UI.
//!
//! * [`app`]: [`app::ControlApp`], the heart: what the tray and the window will show and do,
//!   behind a small in-process API ([`api`]: commands in, snapshots and events out), so the
//!   UI can live in this process or in a separate window process later;
//! * [`endpoint`]: the control endpoint `connector-ams2` (control interface, `control.state`,
//!   `control.activity`, `control.notices`, `control.services.desired`, the services'
//!   `service.status`);
//! * [`services`]: the ServiceManager (spawn suspended into a kill-on-close job, adopt, clean
//!   and forced stop, restart with backoff) and [`status`], the states and labels;
//! * [`config`], [`descriptor`], [`autostart`], [`instance`] (single instance and the second
//!   launch's call), [`managed`] (parent watch), [`logtail`], [`timefmt`], [`texts`].

pub mod api;
pub mod app;
pub mod autostart;
pub mod cli;
pub mod config;
pub mod descriptor;
#[cfg(windows)]
pub mod dialog;
pub mod endpoint;
pub mod hold;
pub mod icons;
pub mod instance;
pub mod logtail;
pub mod managed;
pub mod resume_watch;
pub mod services;
pub mod shortcut;
pub mod status;
pub mod texts;
pub mod timefmt;
#[cfg(windows)]
pub mod tray;
pub mod tray_model;
pub mod window_host;

/// The control app's id in `control.state.app` and the descriptor's `written_by`.
pub const APP_ID: &str = "ams2-connector-ui";
/// The connector's endpoint id (`peer.app` of both processes).
pub const ENDPOINT_ID: &str = "connector-ams2";
/// The connector's id (folder name under `connectors\`).
pub const CONNECTOR_ID: &str = "ams2";
/// The simulator's id.
pub const SIM: &str = "ams2";
/// The simulator's display name.
pub const SIM_NAME: &str = "Automobilista 2";
/// The connector's display name.
pub const CONNECTOR_NAME: &str = "AMS2 connector";
/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The services `control.service.start|stop` take.
pub const SERVICES: [&str; 1] = ["recorder"];
