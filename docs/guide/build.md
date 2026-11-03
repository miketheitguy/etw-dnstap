# Build the executable

> **Status:** Current · **Describes:** the MSI work at commit `1aad4d0` · **Reviewed:** 2026-11-03

Use this guide to build `etw-dnstap.exe` from source and check its hardening.
It is for the person who prepares the executable. To build the MSI installer, see
[Build the MSI](#build-the-msi). To install the result on a DNS Server, see
[Install and provision](install.md).

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
- **Check the version.** The executable carries a version resource that matches
  `Cargo.toml`. You can read it without running the program:

  ```powershell
  (Get-Item target\x86_64-pc-windows-msvc\release\etw-dnstap.exe).VersionInfo
  ```

  `FileVersion` is `X.Y.Z`. `ProductVersion` is the full version, which includes
  any pre-release or build metadata. `ProductName` is `EtwDnstap`. The binary has
  no `--version` flag and prints nothing. At run time, the `version` field of the
  `capture started` event shows the same value. See
  [install.md](install.md#7-start-capture-and-check-that-it-is-running).

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


## Build the MSI

Use this section to build the MSI installer from the release executable. The
build script runs the Rust release build and the CI checks on it, then the WiX
build. The script runs on Windows only. To install the MSI, see
[Install with the MSI](install.md#install-with-the-msi).

Lab validation: pending, see [docs/plans/msi-installer-plan.md](../plans/msi-installer-plan.md).

### Prerequisites

- **PowerShell 7** (`pwsh`). The script requires version 7.0 or later.
- **.NET SDK 8 or later.** The maintainer's build used the 10.0.403 SDK. The SDK
  builds the WiX project.
- **WiX v7 from NuGet.** The project pins `WixToolset.Sdk` 7.0.1 and
  `WixToolset.Util.wixext` 7.0.1. NuGet restores them for the build. Do not install
  WiX globally.
- **The Rust toolchain and the Visual Studio C++ Build Tools**, as in the
  sections above.

### The WiX OSMF EULA

WiX v7 requires acceptance of the Open Source Maintenance Fee (OSMF) EULA. The
maintainer of this repository accepted it. The project file
[EtwDnstap.wixproj](../../packaging/msi/EtwDnstap.wixproj) sets `AcceptEula` to
`wix7`, which is the direct acceptance that the build uses. Read the terms at
<https://docs.firegiant.com/wix/osmf/> before you build.

Whoever builds the MSI must have the right to accept the EULA for themselves or
for their organization. A fee can apply to an organization that earns revenue.
If you do not have that right, do not build the MSI.

### Run the build

Run this from the repository root:

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1
```

The script runs these steps in order. It stops at the first failure with a
non-zero exit code.

1. **Version.** Reads the version of the `etw-dnstap` package with
   `cargo metadata`, and checks it against SemVer 2.0.0. It refuses a version
   with a pre-release part, such as `0.2.0-rc.1`, and a version with build
   metadata. An MSI cannot carry either one. It also refuses a major or minor
   version above 255, or a patch version above 65535.
2. **Release build.** Runs `cargo clean --release -p etw-dnstap` and then
   `cargo build --locked --release`. The build log goes to
   `target\msi\release-build.log`. The clean step makes the log include the
   compile and link of `etw-dnstap`, which the next check reads.
3. **CI checks.** Runs `scripts/ci/verify-link-exceptions.ps1` on the log, and
   `scripts/ci/verify-pe.ps1` on the executable. See
   [Build the executable](#build-the-executable).
4. **Version resource.** Checks the executable's binary `FileVersion`
   (`X.Y.Z.0`), its `FileVersion` string (`X.Y.Z`), and its `ProductVersion`
   string (the full version).
5. **WiX build.** Builds `packaging/msi/EtwDnstap.wixproj` in Release.
6. **Property table.** Reads the MSI `Property` table through Windows Installer.
   It checks `ProductVersion`, `UpgradeCode`, `ProductName`, and `Manufacturer`.
7. **Hash.** Writes the SHA-256 hash file next to the MSI, and prints the
   version, the paths, and the hash.

The output is in `target\msi\`:

| File | Content |
| --- | --- |
| `etw-dnstap-X.Y.Z-x64.msi` | The installer, for the version in `Cargo.toml`. |
| `etw-dnstap-X.Y.Z-x64.msi.sha256` | One line: the SHA-256 hash, two spaces, and the file name. |

`target\` is ignored by Git. The script writes nothing outside it. To check the
MSI, compare its hash with the `.sha256` file:

```powershell
Get-FileHash target\msi\etw-dnstap-<version>-x64.msi -Algorithm SHA256
```

### Build from an existing executable

`-SkipCargoBuild` skips step 2. The script then reuses
`target\x86_64-pc-windows-msvc\release\etw-dnstap.exe` and
`target\msi\release-build.log`. It stops if either file is missing. The log must
come from the build of that executable, because step 3 reads it.

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1 -SkipCargoBuild
```

## Next steps

- [Install and provision the service](install.md)
- [Contributing](../../CONTRIBUTING.md#build-and-check) for the full check list
