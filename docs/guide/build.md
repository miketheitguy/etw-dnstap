# Build the executable

> **Status:** Current · **Describes:** `main` at commit `0651f9b` · **Reviewed:** 2026-11-02

Use this guide to build `etw-dnstap.exe` from source and check its hardening.
It is for the person who prepares the executable. To install the result on a DNS
Server, see [Install and provision](install.md).

Before you start, you need a clone of this repository and Visual Studio C++
Build Tools with the Windows SDK. The tools supply the MSVC linker and the
resource compiler. You do not need a Protocol Buffers compiler.

## Install the Rust toolchain

The repository pins its toolchain in [rust-toolchain.toml](../../rust-toolchain.toml).
It is a dated nightly, so a stable toolchain is not enough.

1. Install rustup from <https://rustup.rs> (the `rustup-init.exe` for Windows x64).
2. From the repository root, run `rustup toolchain install` with no toolchain
   name. rustup reads `rust-toolchain.toml` and installs the pinned nightly with
   `clippy`, `rustfmt`, `rust-src`, and the `x86_64-pc-windows-msvc` target. CI
   runs the same command with `--profile minimal`. Current rustup versions do
   not install a toolchain from `rustup show`.
3. Run `rustup show active-toolchain`. Check that the output names the pinned
   channel.

**Updating the toolchain.** A pinned channel does not follow `rustup update`.
Change `channel` in `rust-toolchain.toml` in a reviewed commit, then run `rustup
toolchain install` again. Record the change in the
[CHANGELOG](../../CHANGELOG.md), because the build output and the measured
behavior depend on it. Remove an old toolchain with `rustup toolchain uninstall
<name>` only after the new one builds.

## Build the executable

Run these from the repository root:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The release executable is
`target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. Git ignores `target\`.
It is a local build artifact, not part of the clone.

Points to know before you build:

- **The first build is slow.** `.cargo/config.toml` enables `build-std`, so
  Cargo rebuilds the standard library and `panic_unwind` with the target
  hardening flags. Later builds reuse that output.
- **Warnings fail the build.** Under `[lints.rust]`, `Cargo.toml` sets
  `warnings` to `deny`. Any compiler warning is an error, including in a
  release build.
- **The release profile** enables overflow checks and `panic = "abort"`.
  `.cargo/config.toml` statically links the MSVC C runtime (`+crt-static`) for
  every build of the target. The clean Server Core deployment does not need the
  Visual C++ Redistributable.
- **Hardening flags** (CFG, EH continuation guard, ASLR, high-entropy VA, NX,
  CET-compatible marker, stack protection (`-Z stack-protector=all`), and
  `/DEBUG:NONE` for release) come from
  `.cargo/config.toml`. The release profile also sets `trim-paths = "all"`,
  which keeps build-host paths out of the image. The CET flag is a compatibility
  marker. It does not prove that the hardware enforces shadow stacks. After each
  build, and after any toolchain or flag change, check the release executable:

  ```powershell
  scripts\ci\verify-pe.ps1 -Path target\x86_64-pc-windows-msvc\release\etw-dnstap.exe
  ```

  The script fails in each of these conditions:

  - A hardening flag is missing.
  - The image has a PDB entry.
  - The image imports an ASan runtime, or the dynamic C or Universal CRT.
  - The image contains an absolute path that starts at a drive's `Users` folder.

  CI runs the same check on the release build.
- **Linker warnings.** The release link prints one `LNK2047` warning for each
  prebuilt `ring` assembly object. `.cargo/config.toml` allows them with
  `/FORCE:GUARDEHCONT`. CI runs `scripts\ci\verify-link-exceptions.ps1` on the
  build log. It fails when the objects differ from `scripts/ci/link-exceptions.txt`.
- **Check the binary.** `etw-dnstap.exe --version` prints the package version.
  `--help` prints the options without starting capture.

### Optional: diagnostic build

The `diag-timing` Cargo feature adds parser wait and socket-write timing to the
stop report, and enables two diagnostic flags. Build it separately and do not
deploy it as the production binary:

```powershell
cargo build --locked --release --features diag-timing
```

See [configuration.md](configuration.md#diagnostic-flags-diag-timing-builds-only).

### Optional: address-sanitizer builds

`cargo asan` runs the test suite with AddressSanitizer in `target\asan`:

```powershell
cargo asan
```

This needs more than the Rust nightly. On MSVC it needs the AddressSanitizer
runtime from Visual Studio:

1. In the Visual Studio Installer, add the individual component for C++
   AddressSanitizer (the MSVC x64 ASan runtime). Check its exact name in your
   installer version.
2. Put the directory that contains `clang_rt.asan_dynamic-x86_64.dll` on `PATH`.
   The DLL is not installed in `System32`, so instrumented test binaries cannot
   find it otherwise.

The alias instruments the Rust test graph and standard library. Windows system
DLLs and prebuilt CRT code are not instrumented. Reading an ASan result is
therefore a statement about Rust-compiled code only.


## Next steps

- [Install and provision the service](install.md)
- [Contributing](../../CONTRIBUTING.md#build-and-check) for the full check list
