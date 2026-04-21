#![allow(clippy::unwrap_used, clippy::expect_used)]
//! # Security regression tests — WASM plugin vetting pipeline
//!
//! These tests pin the behaviour of [`SkillVetter`] under known supply-chain
//! attacks: tampered binaries, forged signatures, oversized payloads,
//! malformed WASM, capability escalation. Argentor positions itself as a
//! security-first agent framework; if the vetting pipeline regresses, every
//! third-party skill becomes a backdoor.
//!
//! References:
//! - CWE-345: Insufficient Verification of Data Authenticity
//! - CWE-347: Improper Verification of Cryptographic Signature
//! - CWE-353: Missing Support for Integrity Check
//! - CWE-494: Download of Code Without Integrity Check
//! - CWE-770: Allocation of Resources Without Limits or Throttling
//! - SLSA Level 3+ supply-chain integrity

use argentor_skills::vetting::{SkillManifest, SkillVetter};

/// Construct the smallest valid WASM module — magic bytes + version.
fn minimal_wasm() -> Vec<u8> {
    vec![
        0x00, 0x61, 0x73, 0x6d, // \0asm magic
        0x01, 0x00, 0x00, 0x00, // version 1
    ]
}

/// Build a baseline manifest matching a given WASM binary.
fn manifest_for(wasm: &[u8]) -> SkillManifest {
    SkillManifest {
        name: "regression_test_skill".into(),
        version: "1.0.0".into(),
        description: "Regression test fixture".into(),
        author: "regression-suite".into(),
        license: Some("AGPL-3.0-only".into()),
        checksum: SkillManifest::compute_checksum(wasm),
        capabilities: vec!["file_read".into()],
        signature: None,
        signer_key: None,
        min_argentor_version: None,
        tags: vec![],
        repository: None,
    }
}

/// CWE-347: Skill without a signature must be rejected when signatures are required.
#[test]
fn test_rejects_unsigned_skill() {
    let wasm = minimal_wasm();
    let manifest = manifest_for(&wasm); // signature: None

    let vetter = SkillVetter::new().with_require_signatures(true);
    let result = vetter.vet(&manifest, &wasm);

    assert!(
        !result.passed,
        "CRITICAL: unsigned skill must be rejected when signatures are required"
    );
    assert!(result
        .checks
        .iter()
        .any(|c| c.name == "signature" && !c.passed));
}

/// CWE-347: Skill signed by an UNTRUSTED key must be rejected even though
/// the signature itself is mathematically valid.
#[test]
fn test_rejects_wrong_signature() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let wasm = minimal_wasm();
    let mut manifest = manifest_for(&wasm);

    // Sign with key A
    let key_a = SigningKey::generate(&mut OsRng);
    manifest.sign(&hex::encode(key_a.to_bytes())).unwrap();

    // But trust only key B
    let key_b = SigningKey::generate(&mut OsRng);
    let trusted = vec![hex::encode(key_b.verifying_key().to_bytes())];

    let vetter = SkillVetter::new()
        .with_require_signatures(true)
        .with_trusted_keys(trusted);

    let result = vetter.vet(&manifest, &wasm);
    assert!(
        !result.passed,
        "CRITICAL: signature from non-trusted key must be rejected (CWE-347)"
    );
}

/// CWE-345 / CWE-353: Manifest claims hash X, actual file hashes to Y.
/// The vetter must catch the mismatch — this is the supply-chain integrity
/// check that prevents binary-swap attacks.
#[test]
fn test_rejects_checksum_mismatch() {
    let wasm = minimal_wasm();
    let manifest = manifest_for(&wasm);

    // Tamper with the binary AFTER the manifest was built
    let mut tampered = wasm.clone();
    tampered.push(0xff);
    tampered.push(0xee);

    let vetter = SkillVetter::new();
    let result = vetter.vet(&manifest, &tampered);

    assert!(
        !result.passed,
        "CRITICAL: checksum mismatch must be detected (CWE-345)"
    );
    assert!(result
        .checks
        .iter()
        .any(|c| c.name == "checksum" && !c.passed));
}

/// CWE-770: Oversized WASM payload (> configured limit) must be rejected
/// before any further parsing happens.
#[test]
fn test_rejects_oversized_wasm() {
    // Create an "11MB" valid-magic WASM blob
    let mut huge = Vec::with_capacity(11 * 1024 * 1024);
    huge.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    huge.resize(11 * 1024 * 1024, 0xaa);

    let manifest = SkillManifest {
        checksum: SkillManifest::compute_checksum(&huge),
        ..manifest_for(&huge)
    };

    let vetter = SkillVetter::new(); // default: 10 MB limit
    let result = vetter.vet(&manifest, &huge);

    assert!(
        !result.passed,
        "CRITICAL: oversized WASM (>10MB) must be rejected (CWE-770)"
    );
    assert!(result
        .checks
        .iter()
        .any(|c| c.name == "size_limit" && !c.passed));
}

/// CWE-345: Bytes that don't start with the WASM magic number must be
/// rejected — this catches misnamed payloads (a `.wasm` file that's
/// actually a shell script, ELF, or PE binary).
#[test]
fn test_rejects_invalid_wasm_magic() {
    let evil_payloads: &[(&str, Vec<u8>)] = &[
        (
            "ELF binary",
            vec![0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00],
        ),
        (
            "Mach-O binary",
            vec![0xfe, 0xed, 0xfa, 0xce, 0x07, 0x00, 0x00, 0x01],
        ),
        ("Shell script", b"#!/bin/sh\nrm -rf /\n".to_vec()),
        (
            "ZIP file",
            vec![0x50, 0x4b, 0x03, 0x04, 0x00, 0x00, 0x00, 0x00],
        ),
    ];

    for (label, payload) in evil_payloads {
        let manifest = manifest_for(payload);
        let vetter = SkillVetter::new();
        let result = vetter.vet(&manifest, payload);

        assert!(
            !result.passed,
            "CRITICAL: '{label}' must be rejected — invalid WASM magic"
        );
        assert!(result
            .checks
            .iter()
            .any(|c| c.name == "wasm_valid" && !c.passed));
    }
}

/// CWE-285: A skill that requests blocked capabilities must be rejected.
#[test]
fn test_rejects_excessive_capabilities() {
    let wasm = minimal_wasm();
    let mut manifest = manifest_for(&wasm);
    // Request a capability that the operator has explicitly blocked
    manifest.capabilities = vec!["shell_exec".into(), "file_write".into()];

    let vetter = SkillVetter::new().with_blocked_capabilities(vec!["shell_exec".into()]);

    let result = vetter.vet(&manifest, &wasm);
    assert!(
        !result.passed,
        "CRITICAL: skill requesting blocked capability 'shell_exec' must be rejected"
    );
    assert!(result
        .checks
        .iter()
        .any(|c| c.name == "blocked_capability" && !c.passed));
}

/// CWE-829: Suspicious WASI imports flagged by static analysis.
///
/// Documented behaviour: SUSPICIOUS_IMPORTS are FLAGGED (warning only) but
/// do not block. This is intentional — most WASI imports are legitimate.
/// The vetting check still emits the import_analysis line so an operator
/// can review. This regression test pins the contract: imports show up in
/// the report, even if they don't fail vetting.
#[test]
fn test_rejects_suspicious_imports() {
    // Construct a WASM-magic blob that ALSO contains the string "proc_exit"
    // somewhere inside — the static-analysis heuristic looks for the string.
    let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    wasm.extend_from_slice(b"...proc_exit...sock_connect...");

    let manifest = manifest_for(&wasm);
    let vetter = SkillVetter::new();
    let result = vetter.vet(&manifest, &wasm);

    // Documented: passes vetting but the import_analysis check reports the finding.
    let import_check = result
        .checks
        .iter()
        .find(|c| c.name == "import_analysis")
        .expect("import_analysis check must exist");
    assert!(
        import_check.message.contains("proc_exit") || import_check.message.contains("sock_connect"),
        "Import analysis must surface suspicious imports for operator review, got: {}",
        import_check.message
    );
}

/// Negative test: a properly signed, reasonably sized, safely-permissioned
/// skill must PASS vetting — no false positives that would block legitimate
/// publishing.
#[test]
fn test_accepts_valid_skill() {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let wasm = minimal_wasm();
    let mut manifest = manifest_for(&wasm);

    // Sign with a fresh key
    let signing_key = SigningKey::generate(&mut OsRng);
    let trusted_pub = hex::encode(signing_key.verifying_key().to_bytes());
    manifest.sign(&hex::encode(signing_key.to_bytes())).unwrap();

    let vetter = SkillVetter::new()
        .with_require_signatures(true)
        .with_trusted_keys(vec![trusted_pub]);

    let result = vetter.vet(&manifest, &wasm);
    assert!(
        result.passed,
        "False negative: valid signed skill must pass vetting (checks: {:?})",
        result.checks
    );
}

/// Integrity: the canonical bytes used for signing must EXCLUDE the
/// signature and signer_key fields — otherwise the signature would
/// chicken-and-egg itself.
#[test]
fn test_canonical_bytes_exclude_signature_fields() {
    let wasm = minimal_wasm();
    let mut manifest = manifest_for(&wasm);

    let canonical_before = manifest.canonical_bytes().unwrap();

    // Pretend to set a signature + key
    manifest.signature = Some("deadbeef".repeat(16));
    manifest.signer_key = Some("cafebabe".repeat(8));

    let canonical_after = manifest.canonical_bytes().unwrap();

    assert_eq!(
        canonical_before, canonical_after,
        "CRITICAL: canonical_bytes must be IDENTICAL with or without signature/signer fields, \
         otherwise signing breaks itself"
    );
}
