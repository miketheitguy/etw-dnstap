# Spool encryption plan

> **Status:** Approved 2026-11-04; Wave 1 in progress · **Describes:** `main` at commit `64b5e91` · **Evidence dated:** not applicable (design from Microsoft Learn and the source; no lab run yet) · **Reviewed:** 2026-11-04

The spool writes dnstap frames to disk while the receiver is down or slow. Today they are
plaintext Frame Streams files. This plan encrypts what the spool writes, and only that:
frames in memory, in the send queue, and on the wire are not changed. Replay decrypts a
segment before its frames go back to the router.

## Decisions (user, 2026-11-04)

| Topic | Decision |
| --- | --- |
| Cipher | AES-128-GCM, one record per spool append. GCM is CTR mode with an authentication tag, so a changed or torn record is detected and counted, not replayed. |
| Key protection | A persisted, non-exportable RSA-3072 machine key in the Microsoft Software Key Storage Provider, created by the installer. Its DACL grants use to `NT SERVICE\EtwDnstap`. The AES key is wrapped with RSA-OAEP (SHA-256) and stored as `REG_BINARY` under the service's `Parameters` key. |
| Disk I/O | Keep cached file I/O. No `FILE_FLAG_NO_BUFFERING`. |
| Old segments | At startup, delete every segment the service cannot read (plaintext segments from earlier versions, and segments made with another key) and count them as dropped. |

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
- **Caveat:** the docs say a service must not call `NCryptCreatePersistedKey` or
  `NCryptSetProperty` from its start routine. Only the installer creates the key and sets
  its DACL; the service only opens it, encrypts with the public half, and decrypts.

## Threat model

Protects spool files and the wrapped key at rest against: a copied disk, VM image, or
backup taken off the machine; and local accounts other than the service, SYSTEM, and
Administrators. Does not protect against SYSTEM or Administrators (they can always use or
re-ACL the key), a process running as the service, or memory inspection of the running
service. The AES key is zeroed when the spool closes; `ring`'s expanded key schedule is not
zeroed by `ring`.

## Design

### Key lifecycle

1. **Install** (MSI custom action, runs as SYSTEM): create the RSA-3072 key named
   `EtwDnstap Spool Key` in the Microsoft Software KSP with `NCRYPT_MACHINE_KEY_FLAG`,
   export policy 0 (not exportable), and the DACL `D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GR;;;<service SID>)`
   (read is what `NCryptDecrypt` needs; the agent confirms the minimum right in a test).
   If the key already exists, keep it and reapply the DACL. A major upgrade keeps it
   (`UPGRADINGPRODUCTCODE`, as for the spool). Uninstall deletes the key and the
   `Parameters` key.
2. **First start** (no `SpoolKey` value): `BCryptGenRandom` makes 16 key bytes and an 8-byte
   key id. `NCryptOpenKey` + `NCryptEncrypt` (OAEP, SHA-256) wrap the key. The service writes
   `SpoolKey` and deletes any segments it finds (they cannot be read with a new key).
3. **Every start:** read `SpoolKey`, `NCryptDecrypt` unwraps it inside key isolation, the
   KSP checks the service token against the key DACL. The key lives in a type that zeroes
   it on drop.
4. **Failure:** if the KSP key is missing or unusable, the registry cannot be read or
   written, or an existing `SpoolKey` does not unwrap, the spool is disabled with one
   warning that names the cause (the existing spool-unavailable path). The service keeps
   running, and the spool never writes plaintext. An existing `SpoolKey` is never replaced
   automatically; an administrator deletes the value to force a new key.

`SpoolKey` layout (`REG_BINARY`, little-endian): version `u8` = 1, key id `[u8; 8]`,
wrapping algorithm `u8` = 1 (RSA-OAEP-SHA256), wrapped length `u16`, wrapped key bytes.

Registry: `HKLM\SYSTEM\CurrentControlSet\Services\EtwDnstap\Parameters`. The installer
creates it with the DACL `D:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KR;;;<service SID>)(A;;0x2;;;<service SID>)`
so the service can read the key and set values but not change the DACL. The service runs
with a restricted service SID, so a write needs an allow entry for the service SID.

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

## Tasks

| ID | Wave | Task | Agent (model) | Effort | Output |
| --- | --- | --- | --- | --- | --- |
| K | 1 | Key module `src/spool/key.rs`: random key and key id, wrap and unwrap with a named CNG key (`NCryptOpenStorageProvider`, `NCryptOpenKey`, `NCryptEncrypt`/`NCryptDecrypt` with OAEP SHA-256), `SpoolKey` encode and decode, registry read and write, zero on drop. Tests use a per-user test key that the test creates with a DACL for the current user, and delete it. | `voltagent-lang:rust-engineer` (Opus) | high | Code and tests. |
| C | 1 | Record layer `src/spool/crypt.rs`: the segment format above with `ring` AES-128-GCM, writer and incremental reader over byte slices, no I/O. Known-answer tests, tamper tests for every field, and property tests (round trip, any truncation is `WriterPosition`/`CutOff`, any bit flip is `Corrupt`). | `voltagent-lang:rust-engineer` (Sonnet) | high | Code and tests. |
| I | 1 | Installer: create the KSP key and its DACL, create `Parameters` with its DACL, keep both on major upgrade, delete both on uninstall. Update `docs/guide/install.md`. | `voltagent-lang:powershell-7-expert` (Sonnet) | medium | Installer and provisioning changes. |
| G | 2 | Integration: `SegmentWriter`/`SegmentReader` use C, the spool opens with K, failures take the spool-unavailable path, unreadable segments are deleted and counted at startup, cursor resume by logical offset. Update the guides, the design notes, and the CHANGELOG. | `voltagent-lang:rust-engineer` (Opus) | high | Code, tests, docs. |
| R | 3 | Security and correctness review of K, C, I, and G. | `voltagent-qa-sec:code-reviewer` (Opus) | high | Findings, then fixes. |
| L | 3 | Lab: first start creates `SpoolKey`; restart and major upgrade keep it; an outage spills and replays with no loss; a changed byte in a segment is counted as corrupt; another local account cannot unwrap; spool throughput at 50,000 and 200,000 QPS. Needs explicit authorization. | `general-purpose` (Sonnet) | high | History section. |

Wave 1 tasks touch separate files. `Cargo.toml` (the `ring` dependency and the
`Win32_System_Registry` feature) and the module declarations in `src/spool.rs` are set
up before Wave 1 starts, so the agents do not edit the same file.
