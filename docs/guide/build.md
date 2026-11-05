# Build the executable

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

### Quick Start

From the repository root, with Visual Studio C++ Build Tools and the Windows SDK:

```powershell
rustup toolchain install
cargo build --locked --release
scripts\ci\verify-pe.ps1 -Path target\x86_64-pc-windows-msvc\release\etw-dnstap.exe
```

Output: `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. For the MSI, run
`pwsh -NoProfile -File scripts/build/Build-Msi.ps1` ([Build the MSI](#build-the-msi)).

### Steps / Configuration

#### Install the Rust toolchain

[rust-toolchain.toml](../../rust-toolchain.toml) pins a dated nightly (the build needs nightly-only options).

1. Install rustup from <https://rustup.rs>.
2. Run `rustup toolchain install` with no name in the repository root. It installs the pinned nightly with
   `clippy`, `rustfmt`, `rust-src`, and the `x86_64-pc-windows-msvc` target (CI adds `--profile minimal`).
3. Run `rustup show active-toolchain`. Expect the pinned channel.

To update, change `channel` in a reviewed commit, install, record it in the [CHANGELOG](../../CHANGELOG.md),
and remove the old toolchain once the new one builds.

#### Check and build

Run in the repository root, where `.cargo/config.toml` applies the hardening flags.

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The release profile enables overflow checks, `panic = "abort"`, fat LTO, and one code-generation unit.
`+crt-static` links the MSVC runtime statically, so Server Core needs no Visual C++ Redistributable.
`.cargo/config.toml` sets Control Flow Guard, the EH continuation guard, ASLR, high-entropy VA, NX, a
CET-compatible marker, stack protection (`-Z stack-protector=all`), `/DEBUG:NONE` for release, and
`trim-paths = "all"`.

Run `verify-pe.ps1` after each build and any toolchain or flag change. It finds `dumpbin.exe` on `PATH` or
through `vswhere` (`-DumpBin <path>` names one), and CI runs it too. It fails on a missing hardening flag, a
PDB entry, an ASan runtime or dynamic C or Universal CRT import, or an absolute path that starts at a
drive's `Users` folder.

The binary has no `--version` flag. Read the version from the file; the `capture started` event carries it
too ([step 7](install.md#7-start-capture-and-check-that-it-is-running)):

```powershell
(Get-Item target\x86_64-pc-windows-msvc\release\etw-dnstap.exe).VersionInfo
```

Expect `FileVersion` `X.Y.Z` and `ProductVersion` as the full version, with any pre-release or build
metadata.

#### Optional: diagnostic build

`diag-timing` adds parser wait and socket-write timing to the stop report and two diagnostic flags
([configuration.md](configuration.md#diagnostic-flags-diag-timing-builds-only)).

```powershell
cargo build --locked --release --features diag-timing
```

#### Optional: address-sanitizer builds

`cargo asan` runs the tests under AddressSanitizer, writing to `target\asan`. On MSVC, add the
AddressSanitizer component (MSVC x64 ASan runtime) in the Visual Studio Installer, and put the directory
that holds `clang_rt.asan_dynamic-x86_64.dll` on `PATH`.

```powershell
cargo asan
```

#### Build the MSI

Windows only. Needs PowerShell 7.0 or later (`pwsh`), .NET SDK 8 or later, the Rust toolchain, and the C++
Build Tools. NuGet restores WiX v7 (`WixToolset.Sdk` and `WixToolset.Util.wixext` 7.0.1). Don't install WiX
globally. WiX v7 requires you to accept the Open Source Maintenance Fee (OSMF) EULA
([terms](https://docs.firegiant.com/wix/osmf/)); [EtwDnstap.wixproj](../../packaging/msi/EtwDnstap.wixproj)
sets `AcceptEula` to `wix7`. Build only if you may accept it for yourself or your organization.

```powershell
pwsh -NoProfile -File scripts/build/Build-Msi.ps1
```

It stops at the first failure with a non-zero exit code. In order:

1. **Version.** `cargo metadata`, SemVer 2.0.0. Refuses a pre-release version (such as `0.2.0-rc.1`), build
   metadata, a major or minor above 255, and a patch above 65535.
2. **Release build.** `cargo clean --release -p etw-dnstap`, then `cargo build --locked --release`, logged
   to `target\msi\release-build.log` (the clean puts the compile and link in the log).
3. **CI checks.** `scripts/ci/verify-link-exceptions.ps1` on the log, `scripts/ci/verify-pe.ps1` on the
   executable.
4. **Version resource.** `FileVersion` `X.Y.Z.0` (binary), `X.Y.Z` (string), full `ProductVersion`.
5. **WiX build.** `packaging/msi/EtwDnstap.wixproj`, Release. Each build gets a new `ProductCode`
   ([Downgrade is refused](upgrade.md#downgrade-is-refused)).
6. **Property table.** Checks `ProductVersion`, `UpgradeCode`, `ProductName`, `Manufacturer`.
7. **Hash.** Writes the SHA-256 file and prints the version, paths, and hash.

Output in `target\msi\`: `etw-dnstap-X.Y.Z-x64.msi`, `etw-dnstap-X.Y.Z-x64.msi.sha256` (hash, two spaces,
file name), `etw-dnstap-X.Y.Z-x64.wixpdb` (WiX debug data), `release-build.log`, and `obj\` (WiX
intermediates).

```powershell
Get-FileHash target\msi\etw-dnstap-<version>-x64.msi -Algorithm SHA256
pwsh -NoProfile -File scripts/build/Build-Msi.ps1 -SkipCargoBuild
```

The first line checks the hash against the `.sha256` file. `-SkipCargoBuild` skips step 2 and needs the
release executable and the `release-build.log` from that same build.

### Common Gotchas

- The first build is slow: `build-std` rebuilds the standard library and `panic_unwind` with the hardening
  flags.
- Warnings fail the build, release included (`warnings` is `deny` under `[lints.rust]`).
- The release link prints one `LNK2047` warning per prebuilt `ring` assembly object, allowed by
  `/FORCE:GUARDEHCONT`. CI runs `scripts\ci\verify-link-exceptions.ps1 -LogPath <build log>` and fails when
  the objects differ from [scripts/ci/link-exceptions.txt](../../scripts/ci/link-exceptions.txt).
- The CET flag is a compatibility marker only.
- Never deploy a `diag-timing` build as production.
- The ASan DLL isn't in `System32`.
- The WiX build reports `WIX1076` (ICE61), because `MajorUpgrade` sets `AllowSameVersionUpgrades="yes"`. The
  script continues.
- The OSMF fee can apply to an organization that earns revenue. CI doesn't build the MSI.
