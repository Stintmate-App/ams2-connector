//! Windows resources of `ams2-connector-ui.exe`: the icon (`assets/icon.ico` of the control app) and the
//! version information.

use std::path::PathBuf;

/// The icon, relative to this package.
const ICO: &str = "assets/icon.ico";

fn main() {
    println!("cargo:rerun-if-changed={ICO}");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let ico = manifest.join(ICO).canonicalize().expect("the icon");
    let version = std::env::var("CARGO_PKG_VERSION").expect("version");
    let mut n = version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse::<u16>().ok())
        .chain(std::iter::repeat(0));
    let (a, b, c) = (n.next().unwrap(), n.next().unwrap(), n.next().unwrap());
    let rc = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},0
PRODUCTVERSION {a},{b},{c},0
FILEOS 0x40004
FILETYPE 1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Stintmate\0"
      VALUE "FileDescription", "AMS2 connector control app\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "ams2-connector-ui\0"
      VALUE "LegalCopyright", "Apache-2.0\0"
      VALUE "OriginalFilename", "ams2-connector-ui.exe\0"
      VALUE "ProductName", "Stintmate AMS2 connector\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#,
        ico = ico.display().to_string().replace('\\', "/")
    );
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).expect("write app.rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile the resources");
}
