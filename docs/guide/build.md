# Build the executable

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

This guide builds `etw-dnstap.exe` from source and checks its hardening. To build the MSI, see
[Build the MSI](#build-the-msi). To install the result, see [Install and provision](install.md).

You need a clone of this repository and Visual Studio C++ Build Tools with the Windows SDK. You do
not need a Protocol Buffers compiler.

## Install the Rust toolchain

The repository pins a dated nightly in [rust-toolchain.toml](../../rust-toolchain.toml). A stable
toolchain is not enough. The build uses nightly-only Cargo and compiler options.

1. Install rustup from <https://rustup.rs> (the `rustup-init.exe` for Windows x64).
2. From the repository root, run `rustup toolchain install` with no name. It installs the pinned
   nightly with `clippy`, `rustfmt`, `rust-src`, and the `x86_64-pc-windows-msvc` target. CI runs the
   same command with `--profile minimal`.
3. Run `rustup show active-toolchain`. Check that the output names the pinned channel.

**Updating the toolchain.** A pinned channel does not follow `rustup update`. Change `channel` in
`rust-toolchain.toml` in a reviewed commit, then run `rustup toolchain install`. Record the change in the
[CHANGELOG](../../CHANGELOG.md), because build output and measured behavior depend on it. Remove the old
toolchain only after the new one builds.

## Build the executable

Run these from the repository root. Cargo finds `.cargo/config.toml` and the pinned toolchain from the
current directory, so run them inside the repository. Otherwise the hardening flags do not apply:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The release executable is `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. Git ignores `target\`,
so it is a local artifact.

- **The first build is slow.** `.cargo/config.toml` enables `build-std`. Cargo rebuilds the standard
  library and `panic_unwind` with the hardening flags.
- **Warnings fail the build**, including release builds. `Cargo.toml` sets `warnings` to `deny` under `[lints.rust]`.
- **The release profile** enables overflow checks, `panic = "abort"`, fat link-time optimization, and one
  code-generation unit. It is slower to build. `.cargo/config.toml` statically links the MSVC C runtime
  (`+crt-static`), so Server Core needs no Visual C++ Redistributable.
- **Hardening flags** come from `.cargo/config.toml`: Control Flow Guard, the EH continuation guard, ASLR,
  high-entropy VA, NX, a CET-compatible marker, stack protection (`-Z stack-protector=all`), and
  `/DEBUG:NONE` for release. The CET flag is a compatibility marker. It does not prove that the hardware
  enforces shadow stacks. The release profile also sets `trim-paths = "all"`, which keeps build-host paths
  out of the image.

  After each build, and after any toolchain or flag change, run:

  ```powershell
  scripts\ci\verify-pe.ps1 -Path target\x86_64-pc-windows-msvc\release\etw-dnstap.exe
  ```

  The script uses `dumpbin.exe` from `PATH`, or asks `vswhere` for the newest Visual Studio C++ toolset.
  Pass `-DumpBin <path>` to name a copy. It fails when:

  - A hardening flag is missing.
  - The image has a PDB entry.
  - The image imports an ASan runtime, or the dynamic C or Universal CRT.
  - The image contains an absolute path that starts at a drive's `Users` folder.

  CI runs the same check on the release build.
- **Linker warnings.** The release link prints one `LNK2047` warning for each prebuilt `ring` assembly
  object. `.cargo/config.toml` allows them with `/FORCE:GUARDEHCONT`. CI runs
  `scripts\ci\verify-link-exceptions.ps1 -LogPath <build log>` and fails when the objects differ from
  [scripts/ci/link-exceptions.txt](../../scripts/ci/link-exceptions.txt).
- **Check the version** without running the program:

  ```powershell
  (Get-Item target\x86_64-pc-windows-msvc\release\etw-dnstap.exe).VersionInfo
  ```

  `FileVersion` is `X.Y.Z`, and `ProductVersion` is the full version, with any pre-release or build
  metadata. The binary has no `--version` flag. The `capture started` event carries the same version at
  run time. See [step 7 of the install guide](install.md#7-start-capture-and-check-that-it-is-running).

### Optional: diagnostic build

The `diag-timing` feature adds parser wait and socket-write timing to the stop report, and it enables two
diagnostic flags. Do not deploy this build as the production binary:

```powershell
cargo build --locked --release --features diag-timing
```

See [configuration.md](configuration.md#diagnostic-flags-diag-timing-builds-only).

### Optional: address-sanitizer builds

`cargo asan` runs the test suite with AddressSanitizer and writes its output to `target\asan`:

```powershell
cargo asan
```

On MSVC, add the AddressSanitizer component (the MSVC x64 ASan runtime) in the Visual Studio Installer.
Check its exact name in your installer version. Then put the directory that contains
`clang_rt.asan_dynamic-x86_64.dll` on `PATH`. The DLL is not in `System32`.

The alias instruments the Rust test graph and the standard library. Windows system DLLs and prebuilt CRT
code are not instrumented. So an ASan result covers only Rust-compiled code.

## Build the MSI

The MSI build script runs the Rust release build and the CI checks, then the WiX build. It runs on
Windows only. CI does not build the MSI. To install the MSI, see [Install with the MSI](install.md#install-with-the-msi).

### Prerequisites

- **PowerShell 7** (`pwsh`), version 7.0 or later.
- **.NET SDK 8 or later.**
- **WiX v7 from NuGet.** The project pins `WixToolset.Sdk` 7.0.1 and `WixToolset.Util.wixext` 7.0.1, which
  NuGet restores. Do not install WiX globally.
- **The Rust toolchain and the Visual Studio C++ Build Tools**, as above.

### The WiX OSMF EULA

WiX v7 requires acceptance of the Open Source Maintenance Fee (OSMF) EULA. The project file
[EtwDnstap.wixproj](../../packaging/msi/EtwDnstap.wixproj) sets `AcceptEula` to `wix7`. That is the direct
acceptance that the build uses. Read the terms at <https://docs.firegiant.com/wix/osmf/> before you build.

You must have the right to accept the EULA for yourself or your organization. A fee can apply to an
organization that earns revenue. If you do not have that right, do not build the MSI.

### Run the build

Run this from the repository root:

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1
```

The script runs these steps in order. It stops at the first failure with a non-zero exit code.

1. **Version.** Reads the version with `cargo metadata` and checks it against SemVer 2.0.0. It refuses a
   pre-release version (such as `0.2.0-rc.1`) and build metadata, which an MSI cannot carry. It also refuses
   a major or minor version above 255, or a patch version above 65535.
2. **Release build.** Runs `cargo clean --release -p etw-dnstap`, then `cargo build --locked --release`, with
   the log in `target\msi\release-build.log`. The clean step makes the log include the compile and link that
   step 3 reads.
3. **CI checks.** Runs `scripts/ci/verify-link-exceptions.ps1` on the log, and `scripts/ci/verify-pe.ps1` on
   the executable.
4. **Version resource.** Checks the binary `FileVersion` (`X.Y.Z.0`), the `FileVersion` string (`X.Y.Z`), and
   the `ProductVersion` string (the full version).
5. **WiX build.** Builds `packaging/msi/EtwDnstap.wixproj` in Release. It reports warning `WIX1076` (ICE61),
   because `MajorUpgrade` sets `AllowSameVersionUpgrades="yes"`. The script neither suppresses that warning nor
   stops on it. Every build gets a new `ProductCode`. See [Downgrade is refused](upgrade.md#downgrade-is-refused).
6. **Property table.** Reads the MSI `Property` table through Windows Installer. Checks `ProductVersion`,
   `UpgradeCode`, `ProductName`, and `Manufacturer`.
7. **Hash.** Writes the SHA-256 hash file next to the MSI. Prints the version, the paths, and the hash.

The output goes to `target\msi\`:

| File | Content |
| --- | --- |
| `etw-dnstap-X.Y.Z-x64.msi` | The installer, for the version in `Cargo.toml`. |
| `etw-dnstap-X.Y.Z-x64.msi.sha256` | One line: the SHA-256 hash, two spaces, and the file name. |
| `etw-dnstap-X.Y.Z-x64.wixpdb` | WiX debug data that the build writes next to the MSI. |
| `release-build.log` | The log of the step 2 release build. Step 3 reads it. |

The folder also holds `obj\`, the WiX intermediate files. The script writes nothing outside `target\`, which
Git ignores. To check the MSI, compare its hash with the `.sha256` file:

```powershell
Get-FileHash target\msi\etw-dnstap-<version>-x64.msi -Algorithm SHA256
```

### Build from an existing executable

`-SkipCargoBuild` skips step 2. It reuses `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe` and
`target\msi\release-build.log`. It stops if either file is missing. The log must come from the build of
that executable, because step 3 reads it.

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1 -SkipCargoBuild
```

## Next steps

- [Install and provision the service](install.md)
- [Contributing](../../CONTRIBUTING.md#build-and-check), for the full check list
