//! `ams2-recorder.exe --version`: prints `<exe name> <version>` and exits 0.

#[test]
fn version_prints_the_name_and_the_version() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_ams2-recorder"))
        .arg("--version")
        .output()
        .expect("run ams2-recorder");
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("ams2-recorder {}", env!("CARGO_PKG_VERSION"))
    );
}
