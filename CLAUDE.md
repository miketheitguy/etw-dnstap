# Project rules for Claude Code

## No host directory paths

Do not write a host directory path into anything that goes into the
repository: source code, comments, tests, scripts, documentation, data files,
plan or history documents, and git commit messages or PR descriptions.

A host directory path is any absolute or home-relative path on a build or lab
machine. Examples of what must not appear:

- A Windows profile path such as `C:\Users\<name>\...` or `%USERPROFILE%\...`.
- A clone location such as `C:\Users\<name>\Git\etw-dnstap` or `D:\src\...`.
- A home-relative path such as `~/.cargo`, `~/.claude`, or `$HOME/...`.
- A toolchain path such as `<profile>\.rustup\toolchains\...`.
- A temp or scratchpad path.

Write these instead:

- Repository-relative paths: `src/network.rs`, `docs/guide/install.md`.
- Placeholders when a path is needed: `<repo>`, `<profile>`, `<install-dir>`,
  `C:\Program Files\EtwDnstap\` for the documented install location.
- The generic form `C:\Users\` only where the text describes the trim-paths
  check itself (`scripts/ci/verify-pe.ps1`, `docs/guide/install.md`).

Before a commit, check the staged changes and the commit message:

```powershell
git diff --cached | Select-String -Pattern 'C:\\Users|/c/Users|C:/Users|~/|\$HOME|%USERPROFILE%'
```

Any match is a defect to fix before committing. Build logs that contain such
paths belong under an ignored pattern (`*.log` is ignored), never in a tracked
file.

## Why

The shipped binary is built with `trim-paths = "all"` so that no build-host
path reaches a user. The same rule applies to the repository: a profile path
names a person and a machine, and it is noise that differs for every
contributor.
