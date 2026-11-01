//! Embed the Windows resources into the `rustshot` executables: the application icon, the
//! manifest (Per-Monitor-V2 DPI awareness, `asInvoker`, Common Controls v6) and a version
//! block, so Explorer's Properties → Details shows the product name and version. The resource
//! script is generated here because the version comes from Cargo.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=app.manifest");
        println!("cargo:rerun-if-changed=resources/rustshot.ico");

        // Optional for debug builds (the exe still runs without an icon/manifest), but a release
        // build without them would ship DPI-unaware and iconless, so it fails instead.
        if let Err(e) = embed() {
            if std::env::var("PROFILE").as_deref() == Ok("release") {
                panic!("could not embed app resources (icon + manifest) in a release build: {e}");
            }
            println!("cargo:warning=could not embed app resources: {e}");
        }
    }
}

/// Write the resource script to OUT_DIR and compile it into the executables.
#[cfg(windows)]
fn embed() -> Result<(), String> {
    let out = std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?;
    let rc = std::path::Path::new(&out).join("rustshot.rc");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(&rc, resource_script(dir, env!("CARGO_PKG_VERSION")))
        .map_err(|e| format!("writing {}: {e}", rc.display()))?;
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .map_err(|e| e.to_string())
}

/// The resource script: icon 1, manifest 1 (type 24 = RT_MANIFEST) and VERSIONINFO 1.
#[cfg(windows)]
fn resource_script(dir: &std::path::Path, version: &str) -> String {
    // rc.exe string literals use C escapes, so backslashes in the paths are doubled.
    let path = |p: &str| dir.join(p).display().to_string().replace('\\', "\\\\");
    // FILEVERSION wants four numbers; a pre-release suffix ("0.2.0-beta.1") is dropped there
    // but kept in the version strings.
    let mut nums = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|n| n.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    nums.resize(4, 0);
    let numeric = nums
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3F
FILEFLAGS 0x0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0x0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "RustShot contributors"
      VALUE "FileDescription", "RustShot"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "rustshot"
      VALUE "LegalCopyright", "MIT License"
      VALUE "ProductName", "RustShot"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = path("resources/rustshot.ico"),
        manifest = path("app.manifest"),
    )
}
