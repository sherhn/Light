//! Embeds the application icon and version info into the Windows executable.
//! The version comes from the workspace `Cargo.toml`, so a release changes it in one place.

fn main() {
    // embed-resource is a build dependency on Windows hosts only
    #[cfg(windows)]
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    use std::{env, fs, path::PathBuf};

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../assets/icon/light.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let part = |name: &str| env::var(name).unwrap_or_else(|_| "0".into());
    let (major, minor, patch) = (part("CARGO_PKG_VERSION_MAJOR"), part("CARGO_PKG_VERSION_MINOR"), part("CARGO_PKG_VERSION_PATCH"));
    let rc = format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "{description}"
      VALUE "ProductName", "Light"
      VALUE "InternalName", "light"
      VALUE "OriginalFilename", "light.exe"
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
        version = env::var("CARGO_PKG_VERSION").unwrap_or_default(),
        description = env::var("CARGO_PKG_DESCRIPTION").unwrap(),
    );
    let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("light.rc");
    fs::write(&path, rc).expect("write the resource script");
    let _ = embed_resource::compile(path, embed_resource::NONE);
}
