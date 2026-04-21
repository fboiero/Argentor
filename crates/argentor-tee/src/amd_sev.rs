//! AMD SEV-SNP stub provider.
//!
//! Real integration requires:
//! - AMD EPYC CPU with SEV-SNP enabled (3rd gen "Milan" or newer)
//! - Host kernel with SEV-SNP support (Linux 5.19+)
//! - `sev` / `sev-guest` Rust crates for guest attestation
//! - QEMU/KVM with SEV-SNP launch policy
//! - AMD ARK root certificate for signature verification
//!
//! See <https://www.amd.com/en/developer/sev.html>.

use crate::attestation::AttestationReport;
use crate::provider::TeeProvider;
use crate::types::{CodeMeasurements, EnclaveConfig, EnclaveInfo, EnclaveStatus, TeeKind};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::sync::Mutex;

/// Stub AMD SEV-SNP provider.
pub struct AmdSevProvider {
    enclaves: Mutex<Vec<EnclaveInfo>>,
}

impl AmdSevProvider {
    /// Construct a new stub SEV provider.
    pub fn new() -> Self {
        Self {
            enclaves: Mutex::new(Vec::new()),
        }
    }

    fn mock_measurements() -> CodeMeasurements {
        // TODO: real impl reads the SEV-SNP launch digest (MEASUREMENT field
        // of the attestation report).
        CodeMeasurements {
            image_hash: "c".repeat(96),
            kernel_hash: None,
            application_hash: None,
            mrenclave: None,
            mrsigner: None,
        }
    }
}

impl Default for AmdSevProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TeeProvider for AmdSevProvider {
    fn name(&self) -> &str {
        "amd-sev-stub"
    }

    fn kind(&self) -> TeeKind {
        TeeKind::AmdSev
    }

    fn is_available(&self) -> bool {
        // TODO: real impl checks CPUID for SEV-SNP support and /dev/sev-guest
        false
    }

    async fn spawn_enclave(&self, config: EnclaveConfig) -> ArgentorResult<EnclaveInfo> {
        // TODO: real impl launches a confidential VM via QEMU with
        // `-object sev-snp-guest,...` and waits for boot.
        config
            .validate()
            .map_err(|e| ArgentorError::Security(format!("invalid enclave config: {}", e)))?;
        if config.kind != TeeKind::AmdSev {
            return Err(ArgentorError::Security(format!(
                "AmdSevProvider cannot spawn {:?}",
                config.kind
            )));
        }

        let info = EnclaveInfo {
            enclave_id: format!("sev-{:x}", rand_id()),
            kind: TeeKind::AmdSev,
            status: EnclaveStatus::Running,
            measurements: Self::mock_measurements(),
            created_at: chrono::Utc::now(),
        };
        self.enclaves
            .lock()
            .map_err(|e| ArgentorError::Security(format!("lock poisoned: {}", e)))?
            .push(info.clone());
        Ok(info)
    }

    async fn terminate_enclave(&self, enclave_id: &str) -> ArgentorResult<()> {
        // TODO: real impl sends shutdown signal to the confidential VM
        let mut guard = self
            .enclaves
            .lock()
            .map_err(|e| ArgentorError::Security(format!("lock poisoned: {}", e)))?;
        let before = guard.len();
        guard.retain(|e| e.enclave_id != enclave_id);
        if guard.len() == before {
            return Err(ArgentorError::Security(format!(
                "no such enclave: {}",
                enclave_id
            )));
        }
        Ok(())
    }

    async fn get_attestation(
        &self,
        enclave_id: &str,
        nonce: &str,
    ) -> ArgentorResult<AttestationReport> {
        // TODO: real impl uses /dev/sev-guest SNP_GET_REPORT ioctl with
        // the nonce in REPORT_DATA, signed by the VCEK.
        if nonce.is_empty() {
            return Err(ArgentorError::Security("nonce must not be empty".into()));
        }
        let guard = self
            .enclaves
            .lock()
            .map_err(|e| ArgentorError::Security(format!("lock poisoned: {}", e)))?;
        if !guard.iter().any(|e| e.enclave_id == enclave_id) {
            return Err(ArgentorError::Security(format!(
                "no such enclave: {}",
                enclave_id
            )));
        }
        let mut report = AttestationReport::mock(TeeKind::AmdSev, enclave_id, nonce);
        report.measurements = Self::mock_measurements();
        Ok(report)
    }

    async fn list_enclaves(&self) -> ArgentorResult<Vec<EnclaveInfo>> {
        Ok(self
            .enclaves
            .lock()
            .map_err(|e| ArgentorError::Security(format!("lock poisoned: {}", e)))?
            .clone())
    }
}

fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn cfg() -> EnclaveConfig {
        EnclaveConfig::production(TeeKind::AmdSev, 4096, 4)
    }

    #[test]
    fn construction_yields_empty_registry() {
        let p = AmdSevProvider::new();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[test]
    fn name_and_kind() {
        let p = AmdSevProvider::new();
        assert_eq!(p.name(), "amd-sev-stub");
        assert_eq!(p.kind(), TeeKind::AmdSev);
    }

    #[test]
    fn is_available_returns_false_for_stub() {
        assert!(!AmdSevProvider::new().is_available());
    }

    #[test]
    fn default_equals_new() {
        let p = AmdSevProvider::default();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn spawn_enclave_succeeds_with_valid_config() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert_eq!(info.kind, TeeKind::AmdSev);
        assert_eq!(info.status, EnclaveStatus::Running);
        assert_eq!(info.measurements.image_hash.len(), 96);
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_invalid_memory() {
        let p = AmdSevProvider::new();
        let mut c = cfg();
        c.memory_mb = 0;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_wrong_kind() {
        let p = AmdSevProvider::new();
        let mut c = cfg();
        c.kind = TeeKind::AwsNitro;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn list_after_spawn() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let list = p.list_enclaves().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].enclave_id, info.enclave_id);
    }

    #[tokio::test]
    async fn list_is_empty_initially() {
        let p = AmdSevProvider::new();
        assert!(p.list_enclaves().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminate_removes_enclave() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        p.terminate_enclave(&info.enclave_id).await.unwrap();
        assert!(p.list_enclaves().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminate_unknown_id_errors() {
        let p = AmdSevProvider::new();
        assert!(p.terminate_enclave("sev-ghost").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_empty_nonce() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert!(p.get_attestation(&info.enclave_id, "").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_unknown_enclave() {
        let p = AmdSevProvider::new();
        assert!(p.get_attestation("sev-missing", "n").await.is_err());
    }

    #[tokio::test]
    async fn attestation_ok() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p
            .get_attestation(&info.enclave_id, "sev-nonce")
            .await
            .unwrap();
        assert_eq!(r.kind, TeeKind::AmdSev);
        assert_eq!(r.nonce, "sev-nonce");
    }

    #[tokio::test]
    async fn attestation_measurements_include_image_hash() {
        let p = AmdSevProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p.get_attestation(&info.enclave_id, "n").await.unwrap();
        assert_eq!(r.measurements.image_hash.len(), 96);
    }

    #[tokio::test]
    async fn multiple_enclaves_distinct_ids() {
        let p = AmdSevProvider::new();
        let a = p.spawn_enclave(cfg()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let b = p.spawn_enclave(cfg()).await.unwrap();
        assert_ne!(a.enclave_id, b.enclave_id);
    }
}
