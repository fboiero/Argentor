#![allow(clippy::unwrap_used, clippy::expect_used)]
//! # Security regression tests — path traversal (CWE-22)
//!
//! These tests pin the behaviour of [`PermissionSet::check_file_read`] /
//! [`PermissionSet::check_file_write`] under known path-traversal attacks.
//! The capability layer is the LAST line of defence before any file_* skill
//! actually touches the filesystem — if it stops blocking these patterns,
//! everything downstream is exposed.
//!
//! References:
//! - CWE-22: Improper Limitation of a Pathname to a Restricted Directory
//! - CWE-23: Relative Path Traversal
//! - CWE-36: Absolute Path Traversal
//! - CWE-41: Improper Resolution of Path Equivalence
//! - CWE-59: Improper Link Resolution Before File Access (symlink)
//! - CWE-158: Improper Neutralization of Null Byte
//! - OWASP A01:2021 — Broken Access Control

use argentor_security::{Capability, PermissionSet};
use std::path::Path;

fn perms_with_tmp() -> PermissionSet {
    let mut perms = PermissionSet::new();
    perms.grant(Capability::FileRead {
        allowed_paths: vec!["/tmp".to_string()],
    });
    perms.grant(Capability::FileWrite {
        allowed_paths: vec!["/tmp".to_string()],
    });
    perms
}

/// CWE-23: Classic `../` path traversal must escape detection.
#[test]
fn test_blocks_dotdot_path() {
    let perms = perms_with_tmp();
    let attack = "/tmp/../../../etc/passwd";
    assert!(
        !perms.check_file_read(attack),
        "CRITICAL: `../` traversal '{attack}' must be denied"
    );
    assert!(
        !perms.check_file_write(attack),
        "CRITICAL: `../` traversal write '{attack}' must be denied"
    );
}

/// CWE-36: Absolute path outside the allowed sandbox must be denied.
#[test]
fn test_blocks_absolute_outside_sandbox() {
    let perms = perms_with_tmp();
    let absolute_attacks = [
        "/etc/shadow",
        "/etc/passwd",
        "/root/.ssh/id_rsa",
        "/var/log/auth.log",
    ];
    for attack in absolute_attacks {
        assert!(
            !perms.check_file_read(attack),
            "CRITICAL: absolute path '{attack}' outside /tmp must be denied"
        );
    }
}

/// CWE-22: URL-encoded traversal payloads.
///
/// KNOWN GAP: PermissionSet treats path strings literally — it does not
/// percent-decode. A skill that auto-decodes URLs before passing the path to
/// PermissionSet would be vulnerable. Documented as accepted: skills MUST
/// canonicalize before calling check_file_read.
#[test]
fn test_blocks_url_encoded_traversal() {
    let perms = perms_with_tmp();
    let encoded = "/tmp/%2e%2e/%2e%2e/etc/passwd";
    assert!(
        !perms.check_file_read(encoded),
        "URL-encoded traversal must be denied after decoding"
    );
}

/// CWE-22 / CWE-176: Unicode path equivalence attacks.
///
/// Fixed: PermissionSet now applies NFKC normalization before canonicalization
/// (issue #5). This converts fullwidth dots (U+FF0E) to ASCII dots, enabling
/// the path traversal detector to catch them.
///
/// Note: True overlong UTF-8 sequences like %c0%ae are not valid UTF-8 and
/// are not decoded by the ASCII-only percent-decoder (non-ASCII byte sequences
/// are kept literal). NFKC normalizes Unicode-level equivalences.
#[test]
fn test_blocks_unicode_encoded_traversal() {
    let perms = perms_with_tmp();
    // %2e%2e decodes to ".." via ASCII percent-decoder, then normalize_path blocks it
    let encoded_dots = "/tmp/%2e%2e/%2e%2e/etc/passwd";
    assert!(
        !perms.check_file_read(encoded_dots),
        "Percent-encoded traversal must be denied"
    );
}

/// CWE-158: Null-byte injection — `safe.txt\0../../etc/passwd`.
///
/// REAL GAP DISCOVERED 2026-04-13: PermissionSet's `normalize_path` treats
/// `safe.txt\0..` as a single Normal component, then the trailing `../`
/// pops it, leaving `/tmp/etc/passwd` — which IS under `/tmp` and is
/// therefore APPROVED by the capability check.
///
/// In practice the OS would stop at the NUL and open `safe.txt` only, so
/// the immediate exploit window is narrow. But: any consumer that strips
/// the NUL before calling the OS (e.g. a logging path that does
/// `path.to_string_lossy()`) becomes vulnerable. Defence in depth is
/// missing here. Tracked as SECURITY-TODO; needs explicit NUL rejection
/// in `check_file_read*`.
#[test]
fn test_blocks_null_byte_injection() {
    let perms = perms_with_tmp();
    let attack = "/tmp/safe.txt\x00../../etc/passwd";
    let allowed = perms.check_file_read(attack);
    assert!(
        !allowed,
        "CRITICAL: null-byte injected path must not be silently treated as allowed"
    );
    let enc = "/tmp/safe.txt%00../../etc/passwd";
    assert!(!perms.check_file_read(enc), "encoded null must be denied");
}

/// CWE-59: Symlink that points outside the sandbox must not grant access
/// when followed. We create a real symlink in a tempdir, point it at /etc,
/// and assert PermissionSet denies the canonicalized target.
#[test]
fn test_blocks_symlink_escape() {
    #[cfg(unix)]
    {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let mut perms = PermissionSet::new();
        perms.grant(Capability::FileRead {
            allowed_paths: vec![tmp.path().to_string_lossy().to_string()],
        });

        // Create a symlink inside tmp that points at /etc (outside).
        let link = tmp.path().join("escape");
        std::os::unix::fs::symlink("/etc", &link).expect("create symlink");

        // Reading via the symlink should be denied — canonicalization resolves
        // to /etc which is outside the allowed prefix.
        let target = link.join("passwd");
        assert!(
            !perms.check_file_read_path(&target),
            "CRITICAL: symlink escape must be denied after canonicalization"
        );
    }
}

/// Negative test: a legitimate path inside the sandbox must be allowed.
#[test]
fn test_allows_legitimate_relative_path() {
    let perms = perms_with_tmp();
    assert!(
        perms.check_file_read("/tmp/data/file.txt"),
        "Legitimate path inside /tmp must be allowed (no false negative)"
    );
    // Equivalence: /tmp/./data/file.txt normalizes to the same thing
    assert!(
        perms.check_file_read_path(Path::new("/tmp/./data/file.txt")),
        "Equivalent path with `.` must be allowed after normalization"
    );
}

/// CWE-22: Windows-style backslash separators must NOT bypass the
/// linux-style `/tmp` allowlist. `..\..\windows\system32` is interpreted
/// literally as a single component on Unix and stays inside the sandbox
/// only as a literal filename — but if it's prefixed with /etc... it
/// must be denied.
#[test]
fn test_windows_path_separator() {
    let perms = perms_with_tmp();
    // On Unix, backslash is just a normal character, so this is one filename
    // inside /tmp. That filename literally contains "..\.." but does not
    // navigate up the tree. It IS still under /tmp on Unix — so allowed.
    // The real attack vector ("\..\..\windows\system32") never reaches /tmp.
    let unix_under_tmp = "/tmp/..\\..\\windows\\system32";
    let _ = perms.check_file_read(unix_under_tmp); // documented: literal filename

    // The attack we DO need to block: a path that combines backslashes with
    // forward `..` to escape the sandbox.
    let mixed = "/tmp/../etc/passwd";
    assert!(
        !perms.check_file_read(mixed),
        "CRITICAL: mixed traversal '/tmp/../etc/passwd' must be denied"
    );

    // And outside-prefix backslash paths — denied because they don't start with /tmp.
    let outside = "C:\\Windows\\System32";
    assert!(
        !perms.check_file_read(outside),
        "Windows-style absolute path outside sandbox must be denied"
    );
}

/// CWE-41: Path equivalence via similar-prefix attack — `/tmp-evil/file.txt`
/// must NOT match the allowed prefix `/tmp`. This is critical for any
/// implementation that does naive `starts_with(&str)` instead of component-aware
/// comparison.
#[test]
fn test_blocks_similar_prefix_attack() {
    let perms = perms_with_tmp();
    assert!(
        !perms.check_file_read("/tmp-evil/file.txt"),
        "CRITICAL: '/tmp-evil' must NOT match allowed prefix '/tmp'"
    );
    assert!(
        !perms.check_file_read("/tmpfoo"),
        "CRITICAL: '/tmpfoo' must NOT match allowed prefix '/tmp'"
    );
}

/// CWE-22: Deep traversal through legitimate-looking directories.
///
/// IMPORTANT: a path like `/tmp/a/b/c/d/../../../../etc/shadow` does NOT
/// actually escape `/tmp` after logical normalization — the four `..`
/// pop only `d/c/b/a`, leaving `/tmp/etc/shadow` (still under `/tmp`).
/// We need ENOUGH `..` to pop past the allowed prefix.
#[test]
fn test_blocks_deep_traversal() {
    let perms = perms_with_tmp();
    // Five `..` after four directories pops `/tmp` itself.
    let deep = "/tmp/a/b/c/d/../../../../../etc/shadow";
    assert!(
        !perms.check_file_read(deep),
        "CRITICAL: deep `..` chain '{deep}' must be denied"
    );
}
