//! `ams2-connector-ui.exe`: the AMS2 connector's control app.
#![windows_subsystem = "windows"]

fn main() {
    std::process::exit(ams2_connector_ui::cli::main(
        std::env::args().skip(1).collect(),
    ));
}
