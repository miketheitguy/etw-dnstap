# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it is tagged.

Dates are commit dates in America/Los_Angeles, reconstructed from `git log`. The
repository has no tags yet, so `0.1.0` lists the commits on `main` in order.
Each entry is summarized from its commit subject. See the commit for details.

## [Unreleased]

Placeholder. Entries will be filled from code changes. Do not remove the
headings.

### Added

- Reconnect without resend: _pending_
- DNS PID event filter for the DNS Server provider: _pending_
- Virtual-account identity for the service: _pending_
- Periodic counter logging: _pending_

### Changed

- _pending_

### Fixed

- _pending_

### Security

- _pending_

## [0.1.0] - not tagged

Commits on `main`, oldest first. Entries reflect commit subjects. Per-commit
contents were not reviewed for this file.

### Added

- `08254d4` (2026-10-29), Initial Buildout.
- `2c62a9f` (2026-10-29), Document pooled ETW performance results and add WPR
  graphs. Adds the pooled-intake performance records and generated graphs.
- `e59425f` (2026-10-31), Add single-pass decoding, ETW flush control, and
  low-rate defaults. Adds the `--etw-flush-ms` option and single-pass decoding.
- `9172f2d` (2026-11-01), Clarify low-rate default warnings and high-rate
  guidance. Documentation only.

### Changed

- `3d401cc` (2026-10-29), Optimize DNS ETW intake and restrict capture to client
  events. The service accepts only DNS Server event IDs 256 (query received) and
  257 (response success).
- `5fa2152` (2026-10-29), Preserve capture filter metadata in receiver end
  markers.

### Removed

- The `--output`/`-o`, `--max-output-bytes`, and `--metrics` options and the
  service JSONL metrics writer. The removing commit is not identified in this
  file. Removed arguments now fail command-line parsing. See the
  [instrumentation compatibility note](docs/README.md#instrumentation-compatibility-note).
