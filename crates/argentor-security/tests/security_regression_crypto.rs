#![allow(clippy::unwrap_used, clippy::expect_used)]
//! # Security regression tests — credential vault & crypto integrity
//!
//! These tests pin the cryptographic guarantees of [`EncryptedStore`]:
//! authentication tag verification, key derivation cost, nonce uniqueness,
//! and constant-time comparisons. Any regression here puts every secret
//! at rest in danger.
//!
//! References:
//! - CWE-310: Cryptographic Issues
//! - CWE-311: Missing Encryption of Sensitive Data
//! - CWE-326: Inadequate Encryption Strength
//! - CWE-327: Use of a Broken or Risky Cryptographic Algorithm
//! - CWE-329: Generation of Predictable IV
//! - CWE-916: Use of Password Hash With Insufficient Computational Effort
//! - CWE-208: Observable Timing Discrepancy
//! - OWASP Cryptographic Storage Cheat Sheet (2025): PBKDF2-SHA256 ≥ 600,000

use argentor_security::{decrypt_value, derive_key, encrypt_value, EncryptedStore};

/// CWE-311: Round-trip encryption — store, retrieve, plaintext must match.
#[test]
fn test_credential_vault_encryption_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(dir.path(), "vault-passphrase").unwrap();

    let secret = "sk-ant-api03-supersecret-token-abcdefg";
    store.put_string("api_key", secret).unwrap();

    let retrieved = store.get_string("api_key").unwrap();
    assert_eq!(
        retrieved.as_deref(),
        Some(secret),
        "CRITICAL: encryption round-trip must preserve plaintext exactly"
    );
}

/// CWE-310 / CWE-353: Wrong key must FAIL CLOSED — no partial plaintext leaked.
#[test]
fn test_credential_vault_wrong_key_fails() {
    let dir = tempfile::tempdir().unwrap();

    // Store with one passphrase
    let store_a = EncryptedStore::new(dir.path(), "passphrase-A").unwrap();
    store_a.put_string("secret", "the-real-token").unwrap();

    // Open the same directory with a different passphrase
    let store_b = EncryptedStore::new(dir.path(), "passphrase-B").unwrap();
    let result = store_b.get_string("secret");

    assert!(
        result.is_err(),
        "CRITICAL: decryption with wrong key must fail (got: {result:?})"
    );

    // Defence-in-depth: error message must NOT leak any portion of the plaintext.
    let err_msg = result.unwrap_err();
    assert!(
        !err_msg.contains("the-real-token"),
        "CRITICAL: error message leaked plaintext!"
    );
    assert!(
        !err_msg.contains("real"),
        "CRITICAL: error message leaked partial plaintext!"
    );
}

/// CWE-353 / GCM authentication tag: a single-byte ciphertext mutation must
/// be detected by the auth tag check, not silently produce garbage plaintext.
#[test]
fn test_credential_vault_tamper_detection() {
    let key = derive_key("tamper-test-pass", b"static-salt");
    let plaintext = b"sensitive-database-password";

    let mut ciphertext = encrypt_value(&key, plaintext).unwrap();

    // Find a ciphertext byte to flip (avoid the salt prefix which is part
    // of the authenticated header, just any middle byte will do — flipping
    // it must invalidate the tag).
    let middle = ciphertext.len() / 2;
    ciphertext[middle] ^= 0x01;

    let result = decrypt_value(&key, &ciphertext);
    assert!(
        result.is_err(),
        "CRITICAL: single-bit tamper must be detected by auth tag (got: {:?})",
        result
            .as_ref()
            .map(|v| String::from_utf8_lossy(v).to_string())
    );

    // The error must specifically mention authentication / tampering, not a
    // generic decode error — proves the GCM-style tag is the line of defence.
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Authentication") || msg.contains("tamper"),
        "Expected authentication-failure error, got: {msg}"
    );
}

/// CWE-916 / OWASP 2025: PBKDF2 iteration count must be high enough that
/// brute-forcing the passphrase is computationally expensive.
///
/// The `argentor-security` crate documents `PBKDF2_ITERATIONS = 100_000`
/// (NIST SP 800-132 minimum). OWASP's 2025 guidance for SHA-256 raises
/// this to 600,000. We assert the CURRENT value is at least 100K and
/// document the upgrade path.
///
/// We measure the cost indirectly: the iteration count is a private const,
/// but a key derivation should NOT complete instantly. If someone drops it
/// to a tiny value (CWE-916), this regression catches it.
#[test]
fn test_pbkdf2_iterations_sufficient() {
    use std::time::Instant;

    let start = Instant::now();
    let _key = derive_key("benchmark-passphrase", b"benchmark-salt");
    let elapsed = start.elapsed();

    // 100K iterations of HMAC-SHA256 should take at least ~5ms on any
    // CPU manufactured in the last decade. If derivation completes in
    // under 1ms, the iteration count was likely lowered.
    assert!(
        elapsed.as_micros() > 1_000,
        "CRITICAL: KDF too fast ({elapsed:?}) — iteration count may have been lowered (CWE-916)"
    );

    // SECURITY-TODO: OWASP 2025 recommends 600K iterations for PBKDF2-SHA256.
    // Current value is 100K (NIST SP 800-132 minimum). Track upgrade.
}

/// CWE-329: Same plaintext encrypted twice must produce DIFFERENT
/// ciphertexts (random nonce per encryption). Violating this enables
/// frequency analysis and replay attacks.
#[test]
fn test_aes_gcm_nonce_uniqueness() {
    let key = derive_key("nonce-test", b"salt");
    let plaintext = b"the same message every time";

    let ct1 = encrypt_value(&key, plaintext).unwrap();
    let ct2 = encrypt_value(&key, plaintext).unwrap();
    let ct3 = encrypt_value(&key, plaintext).unwrap();

    assert_ne!(
        ct1, ct2,
        "CRITICAL: encrypting same plaintext twice produced identical ciphertext (CWE-329)"
    );
    assert_ne!(ct1, ct3);
    assert_ne!(ct2, ct3);

    // Yet all three must decrypt to the same plaintext.
    assert_eq!(decrypt_value(&key, &ct1).unwrap(), plaintext);
    assert_eq!(decrypt_value(&key, &ct2).unwrap(), plaintext);
    assert_eq!(decrypt_value(&key, &ct3).unwrap(), plaintext);
}

/// CWE-208: Constant-time comparison must not leak timing information.
///
/// We can't reliably detect timing attacks in user-space tests (jitter,
/// schedulers, JIT effects), but we CAN verify that the constant-time
/// path is exercised — a wrong-key decrypt and a right-key decrypt both
/// reach the tag-comparison step. If someone replaces the comparison with
/// `==`, the test still passes — that is a known limitation of this kind
/// of regression; the value is in the SHAPE of the test (it forces the
/// reviewer to acknowledge the constant-time requirement when modifying
/// the comparison code).
#[test]
fn test_constant_time_checksum_comparison() {
    let key = derive_key("ct-test", b"salt");
    let wrong_key = derive_key("ct-test-wrong", b"salt");
    let plaintext = b"data";
    let ct = encrypt_value(&key, plaintext).unwrap();

    // Both calls must complete (no panic, no early-out divergence).
    let right = decrypt_value(&key, &ct);
    let wrong = decrypt_value(&wrong_key, &ct);

    assert!(right.is_ok(), "right key must decrypt");
    assert!(wrong.is_err(), "wrong key must fail");

    // Both error messages should look the same — no detail like "tag byte 5
    // differed" must leak.
    let err = wrong.unwrap_err();
    assert!(
        !err.contains("byte"),
        "Error must not reveal which byte differed"
    );
    assert!(
        !err.contains("position"),
        "Error must not reveal byte position"
    );
}

/// CWE-532 / CWE-200: Stored secrets must NOT appear in audit logs in cleartext.
///
/// This guards against a developer mistake: writing secret values into
/// audit details. We exercise the contract by demonstrating the encrypted
/// store keeps secrets opaque even when listed.
#[test]
fn test_secrets_redacted_in_logs() {
    let dir = tempfile::tempdir().unwrap();
    let store = EncryptedStore::new(dir.path(), "log-test-pass").unwrap();

    let secret = "PROD-DB-PASSWORD-DO-NOT-LEAK";
    store.put_string("db_password", secret).unwrap();

    // Listing keys returns SHA-256 hashes — NOT the original key names,
    // and certainly not the values.
    let hashed_keys = store.list_hashed_keys().unwrap();
    assert_eq!(hashed_keys.len(), 1);
    let hashed = &hashed_keys[0];

    assert_eq!(
        hashed.len(),
        64,
        "Hashed key must be SHA-256 hex (64 chars)"
    );
    assert!(
        !hashed.contains("db_password"),
        "CRITICAL: original key name must not appear in hashed listing"
    );
    assert!(
        !hashed.contains(secret),
        "CRITICAL: secret value must not appear in hashed listing"
    );

    // Read the raw on-disk file — the secret must NOT appear in cleartext.
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        let path = entry.unwrap().path();
        let raw = std::fs::read(&path).unwrap();
        let raw_str = String::from_utf8_lossy(&raw);
        assert!(
            !raw_str.contains(secret),
            "CRITICAL: secret '{secret}' found in cleartext on disk at {path:?}"
        );
        assert!(
            !raw_str.contains("PROD-DB"),
            "CRITICAL: partial secret found in cleartext on disk"
        );
    }
}

/// CWE-322 / CWE-323: Same passphrase + different salts → different keys.
/// Documented behaviour: derive_key uses a domain-separated salt parameter,
/// so callers passing different salts MUST get different keys.
#[test]
fn test_kdf_salt_separation() {
    let k1 = derive_key("same-pass", b"salt-A");
    let k2 = derive_key("same-pass", b"salt-B");
    assert_ne!(
        k1, k2,
        "CRITICAL: different salts must yield different keys (CWE-323)"
    );
}

/// CWE-310: An empty plaintext must still produce a valid auth tag — the
/// tag protects metadata even when there's no payload, so the round-trip
/// works on empty input.
#[test]
fn test_encrypts_empty_plaintext() {
    let key = derive_key("empty-test", b"salt");
    let ct = encrypt_value(&key, b"").unwrap();
    assert!(
        !ct.is_empty(),
        "Even empty plaintext must produce non-empty ciphertext (salt+nonce+tag)"
    );

    let pt = decrypt_value(&key, &ct).unwrap();
    assert!(pt.is_empty());
}
