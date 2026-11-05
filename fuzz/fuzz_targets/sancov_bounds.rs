//! Section-bound symbols for SanitizerCoverage on Windows MSVC.
//!
//! On COFF, LLVM puts the 8-bit counters in `.SCOV$CM` and the PC table in
//! `.SCOVP$M`, and refers to `__start___sancov_*` and `__stop___sancov_*`
//! without defining them. link.exe does not synthesize section bounds the way
//! ELF linkers do. compiler-rt defines them in
//! `sanitizer_coverage_win_sections.cpp`, but libfuzzer-sys builds only
//! libFuzzer, so each target includes this module. The linker sorts grouped
//! sections by the text after `$`, so `A` and `Z` bracket `M`.
//!
//! The layout mirrors compiler-rt: LLVM adds 8 to each start symbol, so a
//! start symbol is an 8-byte slot; a stop symbol is one byte with 1-byte
//! alignment, so the linker adds no padding before it and the counter and
//! PC-table lengths stay equal. The symbols are defined in assembly because a
//! Rust static of the same name in an instrumented crate collides with the
//! declaration that the sancov pass adds, which LLVM then renames.
//!
//! The fuzz-smoke job builds with `--sanitizer none`. An ASan runtime that
//! also defines these symbols would conflict with this module.
#[cfg(all(windows, target_env = "msvc"))]
core::arch::global_asm!(
    ".section .SCOV$CA,\"dw\"",
    ".p2align 3",
    ".globl __start___sancov_cntrs",
    "__start___sancov_cntrs:",
    ".quad 0",
    ".section .SCOV$CZ,\"dw\"",
    ".p2align 0",
    ".globl __stop___sancov_cntrs",
    "__stop___sancov_cntrs:",
    ".byte 0",
    ".section .SCOVP$A,\"dr\"",
    ".p2align 3",
    ".globl __start___sancov_pcs",
    "__start___sancov_pcs:",
    ".quad 0",
    ".section .SCOVP$Z,\"dr\"",
    ".p2align 0",
    ".globl __stop___sancov_pcs",
    "__stop___sancov_pcs:",
    ".byte 0",
    ".text",
);
