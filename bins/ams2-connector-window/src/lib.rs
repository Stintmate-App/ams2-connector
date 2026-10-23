//! The AMS2 connector's window (PRY-057 U1, decision D11): a separate, short-lived process
//! started by the control app when the user opens the window and gone when it closes. It
//! draws the control app's `ui.snapshot` and sends `ui.command` over the connector's endpoint;
//! the tray process never loads egui or a graphics context.
//!
//! * [`link`]: the endpoint client (the snapshot in, commands out);
//! * [`gui`]: the egui application (Services, Options and Logs panes).

pub mod gui;
pub mod link;
