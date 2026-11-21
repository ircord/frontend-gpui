//! Embeds the Windows icon and version resource in `ircord.exe`. Icon ID 1 is
//! the one GPUI's Windows backend loads for its windows, and Explorer shows it
//! for the file. On Linux hosts `embed-resource` compiles the resource with
//! `llvm-rc`, which the build container has.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../../packaging/windows/ircord.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    parts.resize(4, 0);
    let numeric = parts
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // The constants are spelt out so the file needs no Windows SDK headers:
    // VOS_NT_WINDOWS32 is 0x40004 and VFT_APP is 1.
    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3f
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "IRCord"
      VALUE "FileDescription", "IRCord"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "ircord"
      VALUE "OriginalFilename", "ircord.exe"
      VALUE "ProductName", "IRCord"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = icon.display().to_string().replace('\\', "/"),
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("ircord.rc");
    std::fs::write(&out, rc).expect("write ircord.rc");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_optional()
        .expect("compile the Windows resource");
}
