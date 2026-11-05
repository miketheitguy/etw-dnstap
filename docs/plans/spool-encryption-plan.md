# Spool encryption plan

> **Status:** Approved 2026-11-04; Wave 1 (K, C, I), Wave 2 (G), and task R complete, Wave 3 task L open · **Describes:** `main` at commit `64b5e91` · **Evidence dated:** not applicable (design from Microsoft Learn and the source; no lab run yet) · **Reviewed:** 2026-11-04

The spool writes dnstap frames to disk while the receiver is down or slow. Today they are
plaintext Frame Streams files. This plan encrypts what the spool writes, and only that:
frames in memory, in the send queue, and on the wire are not changed. Replay decrypts a
segment before its frames go back to the router.

## Decisions (user, 2026-11-04)

| Topic | Decision |
| --- | --- |
| Cipher | AES-128-GCM, one record per spool append. GCM is CTR mode with an authentication tag, so a changed or torn record is detected and counted, not replayed. |
| Key protection | A persisted, non-exportable RSA-3072 machine key in the Microsoft Software Key Storage Provider, created by the installer. Its DACL grants use to `NT SERVICE\EtwDnstap`. The AES key is wrapped with RSA-OAEP (SHA-256) and stored as `REG_BINARY` in the value `WrappedKey` of `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, which has its own DACL and lies outside the service key, so a major upgrade keeps it (user, 2026-11-04). Nothing under `Services\EtwDnstap\Parameters` is used. |
| Disk I/O | Keep cached file I/O. No `FILE_FLAG_NO_BUFFERING`. |
| Old segments | At startup, delete every segment the service cannot read (plaintext segments from earlier versions, and segments made with another key) and count them as dropped. |
| Key in memory | Unwrapped once per start; the expanded key is held in one VirtualLock'ed page, excluded from WER dumps, zeroed on drop; never copied (user, 2026-11-04). |

## Research summary

- **DPAPI-NG cannot name a local service SID.** Its documented principals are "a group in an
  Active Directory forest" and web credentials; the domain controller distributes the keys
  for `SID=` and `SDDL=` rules. The local rules are `LOCAL=logon`, `LOCAL=user` (the account
  the service runs as, today LocalService, which many services share), and `LOCAL=machine`
  ("All users on the local computer can decrypt"). `CryptProtectData` with
  `CRYPTPROTECT_LOCAL_MACHINE` has the same scope. Sources: *CNG DPAPI*, *Protection
  Descriptors*, *CNG DPAPI Constants*, *CryptProtectData* on Microsoft Learn.
- **A CNG key DACL can.** The Microsoft Software KSP keeps private keys in files (machine
  keys under `%ALLUSERSPROFILE%\Microsoft\Crypto\Keys`). The private key is held in the LSA
  key isolation process, never in the application process, and every use goes through the
  key storage router. Each persisted key has a security descriptor
  (`NCRYPT_SECURITY_DESCR_PROPERTY`) that the KSP checks against the caller's token. A DACL
  entry for `S-1-5-80-...` (the service SID) works on a workgroup server. Sources: *Key
  Storage and Retrieval*, *Key Storage Property Identifiers*, *NCryptSetProperty*.
- **CNG has no CTR mode** (ECB, CBC, CFB, CCM, GCM, XTS). GCM comes from `ring`, which is
  already in the dependency tree through rustls; `seal_in_place_separate_tag` encrypts in
  place without allocating.
- **Random keys:** `BCryptGenRandom` with `BCRYPT_USE_SYSTEM_PREFERRED_RNG` (Microsoft SDL
  cryptographic recommendations).
- **Caveat:** the Microsoft Learn note says a service must not call CNG key functions from
  its start routine, because the call can deadlock with the key isolation service. The note
  applies to `NCryptOpenKey`, `NCryptEncrypt`, `NCryptDecrypt`, and others, not only to
  `NCryptCreatePersistedKey` and `NCryptSetProperty`. Only the installer creates the key and
  sets its DACL, but the service still opens the key and decrypts at start. Mitigation (review
  finding F2): the service depends on `KeyIso` (installer), and the code loads the key on a
  helper thread with a 3-second wait, so a hung key store never holds `START_PENDING` and
  never stops the service. See "Review R findings".

## Threat model

Protects spool files and the wrapped key at rest against: a copy or backup of the spool
folder alone; local accounts other than the service, SYSTEM, and Administrators; and a disk,
image, or backup of a volume that BitLocker protects, with a TPM. The copied-disk case needs
BitLocker.

Does not protect against:

- An offline copy of a system volume without BitLocker. The key file of the Software KSP
  machine key is protected by machine DPAPI. The DPAPI_SYSTEM LSA secret is in the SECURITY
  and SYSTEM hives on the same volume, and `WrappedKey` is in the SOFTWARE hive. Standard offline
  DPAPI tools recover the RSA key, and with it every segment.
- SYSTEM or Administrators (they can always use or re-ACL the key), and a process running as
  the service.
- Memory inspection of the running service. The locked page is in physical memory, so a VM
  saved state or memory checkpoint, `hiberfil.sys`, and a complete kernel memory dump contain
  it. `WerRegisterExcludedMemoryBlock` covers WER crash dumps only.

The AES key is zeroed when the spool closes; `ring`'s expanded key schedule is not zeroed by
`ring`.

Key in memory (task G): the service unwraps the key once for each start and builds the
`ring` key schedule once. The raw key bytes are zeroed as soon as the schedule is built. The
schedule lives in one page of its own (`src/spool/locked.rs`), locked with `VirtualLock` so it
is never written to the pagefile, registered with `WerRegisterExcludedMemoryBlock` so WER
leaves it out of WER crash dumps, shared by every writer and reader through an `Arc`, and zeroed
before the page is freed. Limits: `ring` builds the schedule on the stack during
construction, and it is moved into the page, so stack residue of it may remain; SYSTEM and
Administrators can read the memory of the process; and the key is in the clear in that
locked page while the service runs. A failed `VirtualLock` is a key error: the spool is
unavailable, and the key is never kept in pageable memory.

## Design

### Key lifecycle

1. **Install** (MSI custom action, runs as SYSTEM): create the RSA-3072 key named
   `EtwDnstap Spool Key` in the Microsoft Software KSP with `NCRYPT_MACHINE_KEY_FLAG`,
   export policy 0 (not exportable), and the DACL `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;<service SID>)`
   (read is what `NCryptDecrypt` needs; the agent confirms the minimum right in a test).
   If the key already exists, keep it and reapply the DACL. A major upgrade keeps it
   (`UPGRADINGPRODUCTCODE`, as for the spool). Uninstall deletes the key, the `Protect`
   registry key, and the `ETW-DNSTapService` key when it is empty.
2. **First start** (no `WrappedKey` value): `BCryptGenRandom` makes 16 key bytes and an 8-byte
   key id. `NCryptOpenKey` + `NCryptEncrypt` (OAEP, SHA-256) wrap the key. The service writes
   `WrappedKey` and deletes any segments it finds (they cannot be read with a new key).
3. **Every start:** read `WrappedKey`, `NCryptDecrypt` unwraps it inside key isolation, the
   KSP checks the service token against the key DACL. The key lives in a type that zeroes
   it on drop. The load runs on a helper thread and the start waits at most 3 seconds for it;
   a timeout takes the failure path below.
4. **Failure:** if the KSP key is missing or unusable, the registry cannot be read or
   written, or an existing `WrappedKey` does not unwrap, the spool is disabled with one
   warning that names the cause (the existing spool-unavailable path). The service keeps
   running, and the spool never writes plaintext. An existing `WrappedKey` is never replaced
   automatically; an administrator deletes the value to force a new key.

`WrappedKey` layout (`REG_BINARY`, little-endian): version `u8` = 1, key id `[u8; 8]`,
wrapping algorithm `u8` = 1 (RSA-OAEP-SHA256, with the key id as the OAEP label, so a
changed key id fails the unwrap), wrapped length `u16`, wrapped key bytes.

Registry (user decision, 2026-11-04: store `WrappedKey` permanently in
`HKLM\SOFTWARE\ETW-DNSTapService\Protect` with its own DACL; no upgrade copy): the value
`WrappedKey` lives in the key `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, outside the service
key. Deleting a service deletes its own registry key, but not this one, so a major upgrade
keeps the wrapped key with no copy step. The installer creates `Protect` with a protected
DACL, `D:P(A;;KA;;;SY)(A;;KA;;;BA)(A;;0x20003;;;<service SID>)`: SYSTEM and Administrators
full control, and the service SID query value, set value, and read control on that key only.
The service runs with a restricted service SID, so its write needs this allow entry for the
service SID. The service never creates the key. Uninstall deletes `Protect`, and the
`ETW-DNSTapService` key when it is empty. Nothing under `Services\EtwDnstap\Parameters` is
used.

**Wave 1 findings (2026-11-04).**

- The minimum right on the CNG key for `NCryptOpenKey` and `NCryptDecrypt` is
  `GENERIC_READ` (stored as `0x80120089`); every narrower mask failed at `NCryptOpenKey`.
  A caller with no access gets `NTE_BAD_KEYSET`, the same code as a missing key. With read
  only, the service cannot change the key's DACL or delete it. SYSTEM and Administrators
  need `GENERIC_ALL` to reapply the DACL and to delete the key.
- The service opens `Protect` with exactly `KEY_QUERY_VALUE | KEY_SET_VALUE`;
  with query value alone the open fails (error 5). On first start it also unwraps the new
  key once before writing it, so a key it could not read back is never stored.
- Deleting a service deletes its whole registry key, so a value under the service key would
  not survive a major upgrade (which removes the old product first). The first design copied
  the value during the upgrade. The user decision (2026-11-04) replaced it: store `WrappedKey` permanently in
  `HKLM\SOFTWARE\ETW-DNSTapService\Protect` with its own DACL; no upgrade copy. The CNG key
  lies outside the service key and is not affected. To be confirmed in the lab.

### Segment format, version 1

| Part | Bytes | Content |
| --- | --- | --- |
| File header | 32 | magic `EDSE`, version `u16` = 1, flags `u16` = 0, key id `[8]`, nonce prefix `[8]` (random per segment), segment sequence `u64` |
| Record header | 12 | plaintext length `u32`, record index `u32` (0, 1, 2, ...), kind `u8` (1 data, 2 end), 3 reserved bytes = 0 |
| Record body | length + 16 | ciphertext, then the 16-byte GCM tag |

- Nonce (96 bits) = nonce prefix (8 bytes) followed by the record index (4 bytes, big-endian).
- Additional authenticated data = the 32-byte file header followed by the 12-byte record
  header. A record cannot move to another index, segment, or key.
- A data record's plaintext is one spool append: complete Frame Streams data frames
  (4-byte length, payload). The end record has an empty plaintext and replaces STOP; the
  file header replaces START.
- Limits: plaintext length at most 1 MiB (larger is corrupt); at most `u32::MAX` records.
- The writer builds header, plaintext copy, and tag in one reused buffer, encrypts in place,
  and writes it with one call. No allocation per append after the first.

Reader outcomes map to today's `EndReason`: end record is `Stop`; a record that is not yet
complete is `WriterPosition` (active segment) or `CutOff` (leftover); a tag failure, a bad
length, a bad kind, an index out of order, or a bad header is `Corrupt`. The rest of a
corrupt segment is dropped and counted, as today.

### Cursor and replay

`cursor.bin` keeps its format. Its offset stays a logical position: the byte offset in the
segment's plaintext frame stream. To resume, the reader walks the record headers (it skips
ciphertext without decrypting), decrypts the record that holds the offset, and starts at
the frame boundary inside it. Rewind (B2) keeps its meaning. Spool size accounting stays on
file sizes, which now include 32 bytes a segment and 28 bytes a record.

**Task G notes (2026-11-04).**

- `open_spool` checks the folder, then calls the key source once (production:
  `key::load_or_create(&KeyLocation::PRODUCTION)`), builds the locked `SegmentKey`, and drops
  the `LoadedKey`. A key or lock error takes the spool-unavailable path: one warning,
  `spool key is not usable`, Event ID 26 (`RISK_SPOOL_DISABLED`), with `reason` set to the
  error text. Tests inject the key source and never touch HKLM or CNG.
- The startup check reads each leftover's header. `NotThisFormat` and `OtherKey` segments
  (all segments when the key was just created) are deleted, their file bytes are added to
  `spool_dropped_bytes`, and one warning per start, `dnstap spool deleted segments it cannot
  read at startup`, carries the counts with Event ID 14 (`LOSS_SPOOL`, the ID of the other
  spool loss lines). A header that is only short (`TooShort`) is left to the reader, which
  counts it as unreadable data, as a bad START was before.
- The reader also refuses a header whose sequence differs from the file name.
- A cursor offset that is not a frame boundary is corrupt from the frame that holds it. A
  cut-off final record of a leftover counts one frame in `spool_dropped`, as a cut-off
  final frame did.
- A rewind inside the record the reader holds costs nothing. Any other rewind walks the
  segment again from its first record: it reads the skipped records but does not decrypt
  them.
- `unread_bytes` and the byte counts of an eviction use the reader's file position. After a
  resume from `cursor.bin`, that position is an estimate (header plus offset) until the
  reader opens the segment.
- A block over 1 MiB, which the router never sends, is split at frame boundaries into
  several records in one write.

## Tasks

| ID | Wave | Task | Agent (model) | Effort | Output |
| --- | --- | --- | --- | --- | --- |
| K | 1 | Key module `src/spool/key.rs`: random key and key id, wrap and unwrap with a named CNG key (`NCryptOpenStorageProvider`, `NCryptOpenKey`, `NCryptEncrypt`/`NCryptDecrypt` with OAEP SHA-256), `WrappedKey` encode and decode, registry read and write in the `Protect` key, zero on drop. Tests use a per-user test key that the test creates with a DACL for the current user, and delete it. | `voltagent-lang:rust-engineer` (Opus) | high | Code and tests. |
| C | 1 | Record layer `src/spool/crypt.rs`: the segment format above with `ring` AES-128-GCM, writer and incremental reader over byte slices, no I/O. Known-answer tests, tamper tests for every field, and property tests (round trip, any truncation is `WriterPosition`/`CutOff`, any bit flip is `Corrupt`). | `voltagent-lang:rust-engineer` (Sonnet) | high | Code and tests. |
| I | 1 | Installer: create the KSP key and its DACL, create `HKLM\SOFTWARE\ETW-DNSTapService\Protect` with its DACL, keep both on major upgrade (no upgrade copy), delete both on uninstall. Update `docs/guide/install.md`. | `voltagent-lang:powershell-5.1-expert` (Sonnet) | medium | Installer and provisioning changes. |
| G | 2 | Integration: `SegmentWriter`/`SegmentReader` use C, the spool opens with K, failures take the spool-unavailable path, unreadable segments are deleted and counted at startup, cursor resume by logical offset. Update the guides, the design notes, and the CHANGELOG. | `voltagent-lang:rust-engineer` (Opus) | high | Done 2026-11-04: code, tests, docs. |
| R | 3 | Security and correctness review of K, C, I, and G. | `voltagent-qa-sec:code-reviewer` (Opus) | high | Done 2026-11-04: findings F1 to F7, fixes in code, installer, and docs (see "Review R findings"). |
| L | 3 | Lab: first start creates `WrappedKey`; restart and major upgrade keep it; an outage spills and replays with no loss; a changed byte in a segment is counted as corrupt; another local account cannot unwrap; spool throughput at 50,000 and 200,000 QPS. Needs explicit authorization. | `general-purpose` (Sonnet) | high | History section. |

Wave 1 tasks touch separate files. `Cargo.toml` (the `ring` dependency and the
`Win32_System_Registry` feature) and the module declarations in `src/spool.rs` are set
up before Wave 1 starts, so the agents do not edit the same file.

## Review R findings (2026-11-04)

| ID | Severity | Finding | Disposition |
| --- | --- | --- | --- |
| F1 | High | The docs said a copied disk, image, or backup cannot read the frames. That is wrong for a system volume without BitLocker: the machine DPAPI secret that protects the Software KSP key file is in the SECURITY and SYSTEM hives on the same volume, and `WrappedKey` is in the SOFTWARE hive. Offline DPAPI tools recover the RSA key and so every segment. The locked page also appears in VM saved states and checkpoints, `hiberfil.sys`, and complete kernel memory dumps. | Fixed in the docs. SECURITY.md, the Threat model above, `docs/guide/configuration.md#encryption`, the CHANGELOG, `docs/design/data-path.md`, and the comments in `src/spool/locked.rs` and `src/spool/crypt.rs` now say what is protected (a copy of the spool folder alone, other local accounts, a BitLocker volume) and what is not (an offline copy of an unencrypted system volume, SYSTEM, Administrators, VM saved state, the hibernation file, full memory dumps). They say "WER crash dumps" for the exclusion. |
| F2 | Medium-High | The CNG calls ran in `connect` during `START_PENDING`. Microsoft Learn warns that a service must not call CNG key functions from its start routine (possible deadlock). The service-level startup watchdog (`service.rs`, armed with the full startup budget before `run_capture`) covered the call, but its expiry aborts the process, so a hung key store would have stopped the service. | Fixed in code and installer. Installer: the service depends on `KeyIso` (another agent). Code: `load_key_bounded` in `src/pipeline/supervisor.rs` runs the key source on the `spool-key-load` helper thread and waits at most `KEY_LOAD_TIMEOUT` (3 s, `src/pipeline/watchdog.rs`). A timeout, a panic, or a thread that cannot start is a key error: the one `spool key is not usable` warning and a run without a spool. The helper is not joined; a late answer is dropped, which zeroes it. The startup deadline formula (10 s plus one second per 16,384 slots) and the watchdog are unchanged. The wait is a third of the base budget, so a slow receiver connect, which has its own 2-second I/O timeouts, still fits the budget, and an ordinary key load takes milliseconds. Tests: a hung source, a panicking source, a timely source, and the timeout against the budget. |
| F3 | Medium | The upgrade copy of `WrappedKey` was deleted while later install steps could still fail; a rollback then left no `WrappedKey` anywhere, and the next start made a new key and deleted every segment. | Superseded by the new location of `WrappedKey` (`HKLM\SOFTWARE\ETW-DNSTapService\Protect`, see Registry above). |
| F4 | Low-Medium | Recreating the CNG key leaves an existing `WrappedKey` that the new key cannot unwrap. | Docs part: the troubleshooting row for `0x80090016` now says that after a repair that made a new key the administrator must also delete `WrappedKey`, which forces a new spool key; segments are then deleted and counted. The installer part is fixed by another agent. |
| F5 | Low | The upgrade copy key inherited the HKLM\SOFTWARE ACL (Users read) until its DACL was set, and stayed readable if setting the DACL failed. | Superseded by the new location of `WrappedKey` (`HKLM\SOFTWARE\ETW-DNSTapService\Protect`, see Registry above). |
| F6 | Low | A failed CNG key delete stopped the uninstall, unlike the other cleanup steps, which only warn. | Fixed in the installer by another agent. |
| F7 | Informational | Two behaviors of the record layer. | Nothing exploitable. Removing the end record of a sealed segment reads as `EndOfFile`, the same as a crash, so the frames before it are replayed. `skip_to` walks the record headers, which are not authenticated until the record is decrypted, so a changed header can only move the resume point inside data that was already delivered. No change. |

Minor: `src/spool/key.rs` now treats any `SECURITY_STATUS` other than `ERROR_SUCCESS` (0) as a
failure, not only negative values. No documented success code other than 0 applies to the
`NCrypt` calls used.

## Option A: the AES key protected in RAM between uses (user decision, 2026-11-04)

The user chose to keep the AES key encrypted in RAM with `CryptProtectMemory`
(`CRYPTPROTECTMEMORY_SAME_PROCESS`) and to build the `ring` key only for the duration of
each record operation. `ring` stays; its key schedule is zeroed by hand.

Measured on the build host (release build): `CryptUnprotectMemory` plus `CryptProtectMemory`
on 16 bytes 2.25 µs, `ring` AES-128-GCM key setup 0.09 µs, sealing a 64 KiB record 4.3 µs.
At 10,000 queries per second the spool writes about 30 records a second, so the added cost
is about 70 µs of CPU a second.

### Design

- **Load.** `NCryptDecrypt` writes the 16 key bytes directly into the locked page that will
  hold them (no other buffer holds the raw key), and `CryptProtectMemory(SAME_PROCESS)`
  encrypts them in place before anything else runs. The first-start path does the same with
  the random key from `BCryptGenRandom`: generate into the page, wrap from the page, protect.
  The check unwrap on first start writes into a second locked buffer that is compared and
  zeroed.
- **Use.** `SegmentKey::with_aead(|key: &LessSafeKey| ...)` is the only way to use the key.
  Under a mutex (the scratch area is shared), it calls `CryptUnprotectMemory`, builds the
  `LessSafeKey` in place in a scratch area of the same locked page, calls
  `CryptProtectMemory` on the raw bytes at once, runs the closure (one seal or one open),
  then drops the schedule in place and zeroes the scratch area with volatile writes. A
  guard does the re-protect and the zeroing in `Drop`, so a panic in the closure cannot
  leave plaintext. Plaintext exists for about 5 µs per record.
- **Failure.** If `CryptUnprotectMemory` or `CryptProtectMemory` fails, the operation fails
  and the raw bytes are zeroed (the key is then unusable): a seal failure is a spool append
  error and an open failure reads as a corrupt record, both counted as today. The service
  never stops for it.
- **Unchanged.** One unwrap per start, one locked page excluded from WER dumps, shared by
  `Arc<SegmentKey>`, never copied; the record format and the cursor.

### Tasks

| ID | Task | Agent (model) | Effort |
| --- | --- | --- | --- |
| P | Implement the design: `SegmentKey` internals (new module `src/spool/protected.rs`), `with_aead` at the two call sites in `crypt.rs`, key generation and unwrap into the locked page in `key.rs` and `supervisor.rs`, tests. | `voltagent-lang:rust-engineer` (Opus) | high |
| PD | Docs: this plan's threat model and decisions, SECURITY.md, `docs/design/data-path.md`, `docs/guide/configuration.md#encryption`, CHANGELOG. | `voltagent-dev-exp:documentation-engineer` (Sonnet) | medium |
| PR | Security review of P and PD. | `voltagent-qa-sec:code-reviewer` (Opus) | high |
