//! Links the performance counter name and help strings into `etw-dnstap.exe`.
//!
//! Performance Monitor reads the counter names from the executable that the
//! manifest's `applicationIdentity` names. The `.rc` file is generated from
//! `packaging/perfcounters/etw-dnstap.man` with CTRPP and committed.

const RESOURCE: &str = "packaging/perfcounters/etw-dnstap-perf.rc";

fn main() {
    println!("cargo:rerun-if-changed={RESOURCE}");
    println!("cargo:rerun-if-changed=build.rs");
    if let Err(error) = embed_resource::compile(RESOURCE, embed_resource::NONE).manifest_required()
    {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
