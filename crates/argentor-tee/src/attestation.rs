//! Cryptographic attestation — proving the enclave is genuine and unmodified.
//!
//! Attestation is the foundation of TEE trust. A remote verifier issues a
//! `nonce` to the enclave, the enclave returns an [`AttestationReport`]
//! signed by a hardware-rooted key, and the verifier checks the measurements,
//! signature, and freshness against expected values.

use crate::types::{CodeMeasurements, TeeKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};

/// Attestation report from an enclave.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationReport {
    /// The TEE technology that produced this report.
    pub kind: TeeKind,
    /// Unique enclave identifier.
    pub enclave_id: String,
    /// Code measurements captured at attestation time.
    pub measurements: CodeMeasurements,
    /// The nonce supplied by the verifier (binds the report to a challenge).
    pub nonce: String,
    /// UTC timestamp when the report was produced.
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Hex-encoded signature over the report.
    pub signature: String,
    /// Hex-encoded DER certificate chain (leaf first).
    pub certificate_chain: Vec<String>,
    /// Arbitrary data the enclave chose to attest to (e.g. public key).
    pub user_data: Vec<u8>,
}

impl AttestationReport {
    /// Build a mock report for testing / scaffolding purposes.
    pub fn mock(kind: TeeKind, enclave_id: impl Into<String>, nonce: impl Into<String>) -> Self {
        let nonce_s: String = nonce.into();
        let enclave_id_s: String = enclave_id.into();
        let mut h = Sha384::new();
        h.update(enclave_id_s.as_bytes());
        h.update(nonce_s.as_bytes());
        let sig = hex::encode(h.finalize());

        Self {
            kind,
            enclave_id: enclave_id_s,
            measurements: CodeMeasurements {
                image_hash: "0".repeat(96),
                kernel_hash: Some("1".repeat(96)),
                application_hash: Some("2".repeat(96)),
                mrenclave: Some("3".repeat(64)),
                mrsigner: Some("4".repeat(64)),
            },
            nonce: nonce_s,
            timestamp: chrono::Utc::now(),
            signature: sig,
            certificate_chain: vec!["deadbeef".into()],
            user_data: Vec::new(),
        }
    }

    /// Age of the report in seconds (from timestamp to now).
    pub fn age_seconds(&self) -> i64 {
        chrono::Utc::now()
            .signed_duration_since(self.timestamp)
            .num_seconds()
    }
}

/// Verification result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Overall verdict — true iff all sub-checks passed.
    pub valid: bool,
    /// TEE technology the report was produced by.
    pub kind: TeeKind,
    /// Whether the measurements matched the expected values.
    pub measurements_match: bool,
    /// Whether the cryptographic signature verified.
    pub signature_valid: bool,
    /// Whether the timestamp was within the accepted freshness window.
    pub timestamp_within_validity: bool,
    /// Human-readable reason when `valid` is false.
    pub failure_reason: Option<String>,
}

impl VerificationResult {
    /// Build a successful result.
    pub fn success(kind: TeeKind) -> Self {
        Self {
            valid: true,
            kind,
            measurements_match: true,
            signature_valid: true,
            timestamp_within_validity: true,
            failure_reason: None,
        }
    }

    /// Build a failed result with a reason.
    pub fn failure(kind: TeeKind, reason: impl Into<String>) -> Self {
        Self {
            valid: false,
            kind,
            measurements_match: false,
            signature_valid: false,
            timestamp_within_validity: false,
            failure_reason: Some(reason.into()),
        }
    }
}

/// Expected measurements (provided by relying party).
#[derive(Debug, Clone)]
pub struct ExpectedMeasurements {
    /// Expected enclave image hash (Nitro PCR0). `None` skips the check.
    pub image_hash: Option<String>,
    /// Expected MRENCLAVE (Intel SGX). `None` skips the check.
    pub mrenclave: Option<String>,
    /// Expected MRSIGNER (Intel SGX). `None` skips the check.
    pub mrsigner: Option<String>,
    /// Maximum acceptable age of the report, in seconds.
    pub max_age_seconds: u64,
}

impl ExpectedMeasurements {
    /// Build an empty expectation set (all checks skipped except freshness).
    pub fn any(max_age_seconds: u64) -> Self {
        Self {
            image_hash: None,
            mrenclave: None,
            mrsigner: None,
            max_age_seconds,
        }
    }
}

/// Stateless attestation verifier.
///
/// The current implementation performs structural checks only. A real verifier
/// must validate the signature against the vendor's root CA, check CRLs, and
/// verify the certificate chain. See TODO markers below.
pub struct AttestationVerifier {
    expected: ExpectedMeasurements,
}

impl AttestationVerifier {
    /// Create a new verifier with the given expected measurements.
    pub fn new(expected: ExpectedMeasurements) -> Self {
        Self { expected }
    }

    /// Verify an attestation report.
    pub fn verify(&self, report: &AttestationReport) -> VerificationResult {
        // 1. Measurements check.
        let measurements_match = self.measurements_match(&report.measurements);
        if !measurements_match {
            return VerificationResult {
                valid: false,
                kind: report.kind,
                measurements_match: false,
                signature_valid: false,
                timestamp_within_validity: false,
                failure_reason: Some("measurements do not match expected values".into()),
            };
        }

        // 2. Freshness check.
        let age = report.age_seconds();
        let timestamp_within_validity = age >= 0 && (age as u64) <= self.expected.max_age_seconds;
        if !timestamp_within_validity {
            return VerificationResult {
                valid: false,
                kind: report.kind,
                measurements_match: true,
                signature_valid: false,
                timestamp_within_validity: false,
                failure_reason: Some(format!(
                    "report is stale: age={}s, max={}s",
                    age, self.expected.max_age_seconds
                )),
            };
        }

        // 3. Signature check.
        // TODO: real implementation must verify the signature against the vendor's
        // root CA (AWS Nitro root, Intel SGX AESM, AMD ARK). For scaffolding we
        // only check that the signature field is non-empty and hex-shaped.
        let signature_valid = !report.signature.is_empty()
            && report.signature.len() % 2 == 0
            && report.signature.chars().all(|c| c.is_ascii_hexdigit());
        if !signature_valid {
            return VerificationResult {
                valid: false,
                kind: report.kind,
                measurements_match: true,
                signature_valid: false,
                timestamp_within_validity: true,
                failure_reason: Some("signature is missing or malformed".into()),
            };
        }

        VerificationResult {
            valid: true,
            kind: report.kind,
            measurements_match: true,
            signature_valid: true,
            timestamp_within_validity: true,
            failure_reason: None,
        }
    }

    fn measurements_match(&self, got: &CodeMeasurements) -> bool {
        if let Some(ref expected) = self.expected.image_hash {
            if got.image_hash != *expected {
                return false;
            }
        }
        if let Some(ref expected) = self.expected.mrenclave {
            if got.mrenclave.as_deref() != Some(expected.as_str()) {
                return false;
            }
        }
        if let Some(ref expected) = self.expected.mrsigner {
            if got.mrsigner.as_deref() != Some(expected.as_str()) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn attestation_report_mock_populates_fields() {
        let r = AttestationReport::mock(TeeKind::AwsNitro, "enc-1", "n-1");
        assert_eq!(r.enclave_id, "enc-1");
        assert_eq!(r.nonce, "n-1");
        assert_eq!(r.kind, TeeKind::AwsNitro);
        assert!(!r.signature.is_empty());
        assert_eq!(r.measurements.image_hash.len(), 96);
    }

    #[test]
    fn attestation_report_mock_is_deterministic_per_input() {
        let a = AttestationReport::mock(TeeKind::IntelSgx, "enc-2", "n-2");
        let b = AttestationReport::mock(TeeKind::IntelSgx, "enc-2", "n-2");
        // signature is deterministic because it's SHA-384(enclave_id || nonce)
        assert_eq!(a.signature, b.signature);
    }

    #[test]
    fn attestation_report_age_is_non_negative() {
        let r = AttestationReport::mock(TeeKind::Stub, "e", "n");
        assert!(r.age_seconds() >= 0);
    }

    #[test]
    fn attestation_report_serde_roundtrip() {
        let r = AttestationReport::mock(TeeKind::AwsNitro, "e1", "n1");
        let j = serde_json::to_string(&r).unwrap();
        let back: AttestationReport = serde_json::from_str(&j).unwrap();
        assert_eq!(back.enclave_id, "e1");
        assert_eq!(back.nonce, "n1");
    }

    #[test]
    fn verification_result_success_helper() {
        let r = VerificationResult::success(TeeKind::AwsNitro);
        assert!(r.valid);
        assert!(r.measurements_match);
        assert!(r.signature_valid);
        assert!(r.timestamp_within_validity);
        assert!(r.failure_reason.is_none());
    }

    #[test]
    fn verification_result_failure_helper() {
        let r = VerificationResult::failure(TeeKind::IntelSgx, "bad sig");
        assert!(!r.valid);
        assert_eq!(r.failure_reason.as_deref(), Some("bad sig"));
    }

    #[test]
    fn expected_measurements_any_skips_identity_checks() {
        let e = ExpectedMeasurements::any(60);
        assert!(e.image_hash.is_none());
        assert!(e.mrenclave.is_none());
        assert!(e.mrsigner.is_none());
        assert_eq!(e.max_age_seconds, 60);
    }

    #[test]
    fn verifier_accepts_valid_mock_report() {
        let v = AttestationVerifier::new(ExpectedMeasurements::any(3600));
        let r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        let res = v.verify(&r);
        assert!(res.valid, "result: {:?}", res);
    }

    #[test]
    fn verifier_rejects_mismatched_image_hash() {
        let v = AttestationVerifier::new(ExpectedMeasurements {
            image_hash: Some("ff".repeat(48)),
            mrenclave: None,
            mrsigner: None,
            max_age_seconds: 3600,
        });
        let r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        let res = v.verify(&r);
        assert!(!res.valid);
        assert!(!res.measurements_match);
    }

    #[test]
    fn verifier_rejects_mismatched_mrenclave() {
        let v = AttestationVerifier::new(ExpectedMeasurements {
            image_hash: None,
            mrenclave: Some("zz".repeat(32)),
            mrsigner: None,
            max_age_seconds: 3600,
        });
        let r = AttestationReport::mock(TeeKind::IntelSgx, "e", "n");
        let res = v.verify(&r);
        assert!(!res.valid);
    }

    #[test]
    fn verifier_rejects_mismatched_mrsigner() {
        let v = AttestationVerifier::new(ExpectedMeasurements {
            image_hash: None,
            mrenclave: None,
            mrsigner: Some("aa".repeat(32)),
            max_age_seconds: 3600,
        });
        let r = AttestationReport::mock(TeeKind::IntelSgx, "e", "n");
        let res = v.verify(&r);
        assert!(!res.valid);
    }

    #[test]
    fn verifier_accepts_matching_mrenclave_and_mrsigner() {
        let r = AttestationReport::mock(TeeKind::IntelSgx, "e", "n");
        let v = AttestationVerifier::new(ExpectedMeasurements {
            image_hash: None,
            mrenclave: r.measurements.mrenclave.clone(),
            mrsigner: r.measurements.mrsigner.clone(),
            max_age_seconds: 3600,
        });
        let res = v.verify(&r);
        assert!(res.valid);
    }

    #[test]
    fn verifier_rejects_stale_report() {
        let mut r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        r.timestamp = chrono::Utc::now() - chrono::Duration::seconds(3600);
        let v = AttestationVerifier::new(ExpectedMeasurements::any(10));
        let res = v.verify(&r);
        assert!(!res.valid);
        assert!(!res.timestamp_within_validity);
    }

    #[test]
    fn verifier_rejects_future_timestamp() {
        let mut r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        r.timestamp = chrono::Utc::now() + chrono::Duration::seconds(600);
        let v = AttestationVerifier::new(ExpectedMeasurements::any(60));
        let res = v.verify(&r);
        assert!(!res.valid);
    }

    #[test]
    fn verifier_rejects_empty_signature() {
        let mut r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        r.signature = String::new();
        let v = AttestationVerifier::new(ExpectedMeasurements::any(3600));
        let res = v.verify(&r);
        assert!(!res.valid);
        assert!(!res.signature_valid);
    }

    #[test]
    fn verifier_rejects_non_hex_signature() {
        let mut r = AttestationReport::mock(TeeKind::AwsNitro, "e", "n");
        r.signature = "not-hex-at-all!".into();
        let v = AttestationVerifier::new(ExpectedMeasurements::any(3600));
        let res = v.verify(&r);
        assert!(!res.valid);
    }
}
