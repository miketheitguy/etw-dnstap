//! Links two resources into `etw-dnstap.exe`: the performance counter name and
//! help strings, and the `VERSIONINFO` block.
//!
//! Performance Monitor reads the counter names from the executable that the
//! manifest's `applicationIdentity` names. That `.rc` file is generated from
//! `packaging/perfcounters/etw-dnstap.man` with CTRPP and committed. The
//! `VERSIONINFO` `.rc` file is generated here, into `OUT_DIR`, from the
//! package metadata. The two compile separately: the counter file is UTF-16
//! and must stay unchanged, and the resource types do not overlap.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "Cargo reads build script directives from stdout and shows stderr"
)]

use std::fmt::Write as _;
use std::path::PathBuf;

const RESOURCE: &str = "packaging/perfcounters/etw-dnstap-perf.rc";

const COMPANY: &str = "miketheitguy";
const COPYRIGHT: &str = "Copyright (c) 2026 miketheitguy";
const PRODUCT: &str = "EtwDnstap";
const FILE_NAME: &str = "etw-dnstap.exe";

/// Escapes a value for an `.rc` string literal: backslash and double quote.
fn rc_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\"\"")
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|error| format!("{name}: {error}"))
}

/// Builds the `VERSIONINFO` resource source from the Cargo package variables.
fn version_rc() -> Result<String, String> {
    let major = env("CARGO_PKG_VERSION_MAJOR")?;
    let minor = env("CARGO_PKG_VERSION_MINOR")?;
    let patch = env("CARGO_PKG_VERSION_PATCH")?;
    let full = env("CARGO_PKG_VERSION")?;
    let description = env("CARGO_PKG_DESCRIPTION")?;
    let numeric = format!("{major},{minor},{patch},0");
    let pairs = [
        ("CompanyName", COMPANY.to_owned()),
        ("FileDescription", description),
        ("FileVersion", format!("{major}.{minor}.{patch}")),
        ("InternalName", FILE_NAME.to_owned()),
        ("LegalCopyright", COPYRIGHT.to_owned()),
        ("OriginalFilename", FILE_NAME.to_owned()),
        ("ProductName", PRODUCT.to_owned()),
        ("ProductVersion", full),
    ];
    let mut values = String::new();
    for (key, value) in pairs {
        writeln!(
            values,
            "            VALUE \"{key}\", \"{}\"",
            rc_escape(&value)
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(format!(
        "#include <winver.h>\n\
         \n\
         VS_VERSION_INFO VERSIONINFO\n\
         FILEVERSION {numeric}\n\
         PRODUCTVERSION {numeric}\n\
         FILEFLAGSMASK VS_FFI_FILEFLAGSMASK\n\
         FILEFLAGS 0\n\
         FILEOS VOS_NT_WINDOWS32\n\
         FILETYPE VFT_APP\n\
         FILESUBTYPE VFT2_UNKNOWN\n\
         BEGIN\n\
         \x20   BLOCK \"StringFileInfo\"\n\
         \x20   BEGIN\n\
         \x20       BLOCK \"040904B0\"\n\
         \x20       BEGIN\n\
         {values}\
         \x20       END\n\
         \x20   END\n\
         \x20   BLOCK \"VarFileInfo\"\n\
         \x20   BEGIN\n\
         \x20       VALUE \"Translation\", 0x0409, 1200\n\
         \x20   END\n\
         END\n"
    ))
}

fn run() -> Result<(), String> {
    let out_dir = PathBuf::from(env("OUT_DIR")?);
    let version_path = out_dir.join("etw-dnstap-version.rc");
    std::fs::write(&version_path, version_rc()?)
        .map_err(|error| format!("writing the version resource: {error}"))?;
    embed_resource::compile(&version_path, embed_resource::NONE)
        .manifest_required()
        .map_err(|error| error.to_string())?;
    embed_resource::compile(RESOURCE, embed_resource::NONE)
        .manifest_required()
        .map_err(|error| error.to_string())
}

fn main() {
    println!("cargo:rerun-if-changed={RESOURCE}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
