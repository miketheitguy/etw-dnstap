# TLS 1.2 and 1.3 for the dnstap receiver connection

> **Status:** Current (implemented 2026-11-01; see section 6 for the decisions taken during implementation) · **Evidence dated:** 2026-11-01 · **Commit:** written at `dc28e45` · **Reviewed:** 2026-11-01

## 1. Goal

Add an optional TLS transport to the Frame Streams TCP sender in
`src/network.rs`. The sender must:

- Negotiate TLS 1.2 or TLS 1.3 only. No other version.
- Use rustls. No Schannel, no OpenSSL.
- Verify the receiver certificate with the Windows system certificate store
  (the machine `Trusted Root Certification Authorities` and intermediates), with
  revocation checks, through `rustls-platform-verifier`.
- Make TLS the default. `--plaintext` turns it off and keeps the existing
  unencrypted path unchanged. An existing deployment that points at a
  plaintext receiver fails at startup until the operator adds `--plaintext`
  or gives the receiver a certificate. Under the CHANGELOG local policy this
  is a contract change and increases `y`.

Out of scope: client certificates (mutual TLS), hostnames in `--receiver`, a
custom CA file, TLS on any other socket.

## 2. Decisions

| Decision | Choice | Reason |
| --- | --- | --- |
| Crypto provider | `ring` (`rustls` with `default-features = false`, features `ring`, `std`, `tls12`) | `aws-lc-rs` needs CMake and NASM on Windows MSVC. Neither is on the build host or in CI. `ring 0.17.14` builds here in seconds with the pinned nightly, `build-std`, and the target hardening flags. |
| Verifier | `rustls-platform-verifier 0.7` | On Windows it calls CryptoAPI (`CertGetCertificateChain`, `CertVerifyCertificateChainPolicy`). That is the system store, including enterprise roots pushed by Group Policy, and revocation. `rustls-native-certs` copies only the root list and does no revocation. It already depends on `windows-sys 0.61`, the version this crate uses, so no duplicate. |
| Versions | `ClientConfig::builder_with_protocol_versions(&[&TLS12, &TLS13])` | Explicit, so a future rustls default change cannot widen it. |
| Disable flag | `--plaintext` (boolean, default off; TLS is on by default) | Encrypted by default. The flag name states what the operator accepts. |
| Server name | `--tls-server-name NAME`, optional, conflicts with `--plaintext` | `--receiver` stays a numeric `IP:PORT`. By default the certificate is verified against the receiver IP (an `iPAddress` SAN). Operators whose receiver certificate carries only a DNS name set this flag. The value is also the SNI. Must parse as `rustls_pki_types::ServerName`. |
| Transport type | `enum Transport { Plain(DeadlineTcp), Tls(rustls::StreamOwned<ClientConnection, DeadlineTcp>) }` with `Read + Write` | Keeps `DeadlineTcp` as the single place that owns the I/O deadline and the write timing stats. `StreamOwned.sock` gives access to it for `rearm` and `attach_stats`. No generics leak into `NetworkSink`. |
| Handshake timing | Run `complete_io` until the handshake is done right after TCP connect, before READY | The TLS handshake shares the 2-second connect/handshake deadline with READY and ACCEPT. Running it explicitly gives the failure a clear context (`TLS handshake`) instead of surfacing inside the READY write. |
| Failure class | rustls maps TLS errors to `io::ErrorKind::InvalidData` | The sink already treats `InvalidData` on the first connection as fatal (configuration error) and on reconnects as a counted failure. A certificate that does not verify therefore stops the service at startup, and is retried after. Document this. No new error kinds. |
| close_notify | Not sent | FINISH ends the Frame Streams session; the TCP close follows. Sending close_notify adds a write under the same 2-second deadline for no protocol benefit. |
| Test certificates | `rcgen 0.14` (dev-dependency, `ring` feature) generates a CA and a server certificate at test time; the test `ClientConfig` trusts that CA through a `RootCertStore` | The platform verifier cannot be unit-tested without writing to the machine store. The code under test is the transport, the deadline, the reconnect, and the handshake, which are the same with either verifier. |
| Licenses | Add `ISC` and `BSD-3-Clause` to `deny.toml` `[licenses].allow` | `ring` is `Apache-2.0 AND ISC`; `subtle` (a rustls dependency) is `BSD-3-Clause`. Both are permissive. Everything else is MIT or Apache-2.0 dual. `webpki-roots` (CDLA) is not used. |

## 3. Changes by file

### Cargo.toml, Cargo.lock, fuzz/Cargo.lock, deny.toml

- Add `rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }`.
- Add `rustls-platform-verifier = "0.7"`.
- Add dev-dependency `rcgen = { version = "0.14", default-features = false, features = ["ring", "pem"] }` (check which features it needs; `pem` only if the tests use PEM).
- Refresh `fuzz/Cargo.lock` without bumping existing crates (`cargo metadata` in `fuzz/`), because the fuzz crate depends on this crate by path and CI runs `cargo deny --locked` on it.
- `deny.toml`: add `"ISC"` to the allow list with a one-line comment naming `ring`.

### src/network.rs

- `pub(crate) struct ReceiverConfig { address: SocketAddr, tls: Option<TlsConfig> }` where `TlsConfig { client: Arc<rustls::ClientConfig>, server_name: ServerName<'static> }`. Build the production `ClientConfig` in one function, `TlsConfig::system(server_name)`, with `rustls_platform_verifier::ConfigVerifierExt` or `BuilderVerifierExt` and the `ring` provider.
- `Connection::connect(&ReceiverConfig)`: TCP connect, `set_nodelay`, `SO_SNDBUF` as today; then, when `tls` is set, build `ClientConnection::new(client, server_name)`, wrap in `StreamOwned`, loop `complete_io` while `is_handshaking()`, log the negotiated `protocol_version` at `debug`. Then READY, ACCEPT, START as today over `Transport`.
- `NetworkSink` stores `ReceiverConfig` instead of `SocketAddr`. The reconnect thread clones it (`Arc<ClientConfig>` is cheap). Log lines keep `receiver = %address`.
- `DeadlineTcp` is unchanged. `rearm` and `attach_stats` reach it through `Transport::deadline_mut()`.
- Keep `finish_stream` generic over `Read + Write`; `Transport` satisfies it.

### src/pipeline.rs

- `CaptureArgs`: add `#[arg(long)] plaintext: bool` and `#[arg(long, conflicts_with = "plaintext", value_parser = parse_server_name)] tls_server_name: Option<String>`. Update the `--receiver` doc comment (it says "Plaintext").
- `PipelineConfig`: carry `ReceiverConfig`. `run_pipeline`'s `connect` closure takes `&ReceiverConfig` (or the config by value) instead of `SocketAddr`. Update the two test call sites that pass `Connection::connect`.
- `capture started` log: add `tls = true|false` (false only with `--plaintext`) and `tls_server_name` when set.
- CLI tests in `src/pipeline/cli_tests.rs`: `--tls-server-name` with `--plaintext` is rejected; a bad server name is rejected; TLS is on by default and off with `--plaintext`.

### Tests (src/network/*.rs)

New module `src/network/tls_tests.rs`, following the existing `#[path]` pattern:

- Helper: generate a CA and a server certificate for `127.0.0.1` (IP SAN) and for a DNS name; build a `rustls::ServerConfig`; a receiver thread that accepts TCP, wraps it in `StreamOwned<ServerConnection, TcpStream>`, and runs the existing `handshake` logic over it (generalize `test_support::handshake` over `Read + Write`, or add a TLS variant).
- `tls_handshake_frames_and_finish_over_tls13`: full session with STOP/FINISH; assert negotiated version is 1.3.
- `tls12_only_receiver_negotiates_tls12`: server restricted to TLS 1.2; the client connects; assert 1.2.
- `tls_server_name_mismatch_is_invalid_data_and_fatal_at_startup`: certificate for another name; `NetworkSink::start` returns `InvalidData`.
- `plaintext_receiver_with_default_tls_is_invalid_data`: receiver answers ACCEPT in plaintext; the client sees a corrupt TLS record; `InvalidData`.
- `tls_reconnects_with_a_fresh_handshake`: mirror `reconnects_with_a_fresh_handshake_and_never_resends` over TLS.
- `tls_handshake_respects_the_deadline`: a receiver that accepts TCP and sends nothing; connect fails within about `IO_TIMEOUT` with `TimedOut`.

Existing tests keep passing unchanged except for the `connect` signature.

### Docs

- `docs/guide/configuration.md`: two new rows in Options (`--plaintext`, `--tls-server-name`); `--receiver` row says TLS by default, plaintext with `--plaintext`; a **TLS** subsection under *Receiver connection* (verification uses the Windows machine store and revocation; failure classes; which versions; SNI); update *Message and transport limits* (deadline covers the TLS handshake). Update the status line commit.
- `docs/guide/install.md`: a short *Receiver certificate* step: the receiver certificate must chain to a CA in `Cert:\LocalMachine\Root` (LocalService cannot see a user store), must carry the receiver IP as an `iPAddress` SAN or the name given in `--tls-server-name`, and revocation endpoints must be reachable or the CRL cached. The example `binPath` needs no flag; show `--plaintext` as the opt-out for a trusted network.
- `README.md`: rewrite the **Security first** paragraph: TLS 1.2/1.3 with system-store verification is the default; `--plaintext` turns it off.
- `SECURITY.md`: mention the TLS option if it describes the transport.
- `CHANGELOG.md` `[Unreleased] / Added` and `/ Changed`: TLS is on by default; existing plaintext deployments must add `--plaintext`. Under the local policy this increases `y`.
- `CONTRIBUTING.md`: update the test counts after the run.
- Write docs in the project's plain STE style (use the `asd-ste100` skill).

## 4. Verification

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diag-timing -- -D warnings
cargo clippy --locked --all-targets --features fuzzing -- -D warnings
cargo build --locked --release
cargo deny --locked check          # if cargo-deny is installed
cd fuzz; cargo deny --locked check
```

Manual, later, on the lab VM (not part of this change): a receiver with a
certificate issued by a CA imported into `Cert:\LocalMachine\Root`, and a
receiver with a self-signed certificate (must fail at startup with Event ID 1).

## 5. Risks

- `ring` compiles C and assembly through `cc`. The probe build on this host
  passed with the pinned nightly. CI uses `windows-latest` with MSVC, which has
  the same toolchain. If CI fails, the fallback is to vendor nothing and report;
  do not switch to `aws-lc-rs` without adding CMake and NASM to CI.
- The platform verifier runs CryptoAPI inside the parser's reconnect helper
  thread. Revocation checks can take seconds against an unreachable CRL
  distribution point. The 2-second deadline does not cover time spent inside
  the verifier because that work is not a socket call. Document this; a later
  change can add a verifier timeout if it is observed.
- The LocalService token is write-restricted. Reading the machine store is a
  read; the validation in `localservice-restricted-validation.md` did not cover
  it. Flag it for the next lab run.

## 6. Decisions taken during implementation

- **`ring` does not link under `/GUARD:EHCONT`.** The plan's probe compiled
  `ring` but did not link a binary. `ring 0.17.14` ships its x86_64 assembly as
  prebuilt NASM objects (`aesni`, `vpaes`, `sha256`, `sha512`, `p256`,
  `x86_64-mont`, `x86_64-mont5`, `chacha`, and others) with no EH-continuation
  metadata. With `-Z ehcont-guard=yes` and `/GUARD:EHCONT`, `link.exe` rejects
  them (`LNK2047`, then `LNK1386`). `cargo check` and `cargo clippy` pass
  because they do not link.
- **Alternatives probed under the project's full flags and `build-std`:**
  - `rustls-graviola 0.4` (pure Rust, no assembly objects) links and runs
    with every hardening flag intact. It is new ("exercise due caution" in its
    README) and requires `avx2`, `bmi2`, `adx`, `aes`, and `pclmulqdq`.
  - `ring` with `/FORCE:GUARDEHCONT` links and runs. The linker prints one
    `LNK2047` line per object as a warning on every build.
  - `aws-lc-rs` was not probed: it needs CMake and NASM, and its assembly is
    also NASM.
- **Decision (2026-11-01, approved by the maintainer):** keep `ring` and add
  `-C link-arg=/FORCE:GUARDEHCONT` to `.cargo/config.toml`, with a comment that
  names the reason and the removal condition. The rest of the image keeps its
  EHCONT metadata. The affected routines never unwind, and release builds use
  `panic = "abort"`, so no exception can pass through them. Revisit when `ring`
  carries the metadata or when `graviola` has more history.
- **`deny.toml`** also allows `BSD-3-Clause` for `subtle`.
- **`run_pipeline`'s `connect` closure** keeps its `SocketAddr` signature.
  `run_capture` builds the `ReceiverConfig` inside the closure, so the existing
  pipeline tests are unchanged. `ReceiverConfig` implements `From<SocketAddr>`
  for the plaintext case, so plaintext tests pass a bare address.
- **The TLS requirement changed to default-on** during planning: `--plaintext`
  disables TLS, and `--tls-server-name` conflicts with it.
