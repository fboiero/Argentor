//! Intel SGX stub provider.
//!
//! Real integration requires:
//! - SGX-capable CPU (Intel Xeon Scalable 3rd gen+, or Core with SGX support)
//! - Intel SGX SDK + PSW (platform software) installed
//! - Either Gramine (<https://gramineproject.io>) or Occlum (<https://occlum.io>)
//!   to run unmodified Linux binaries inside an enclave
//! - DCAP quoting enclave for remote attestation
//!
//! See <https://www.intel.com/content/www/us/en/developer/tools/software-guard-extensions/overview.html>.

use crate::attestation::AttestationReport;
use crate::provider::TeeProvider;
use crate::types::{CodeMeasurements, EnclaveConfig, EnclaveInfo, EnclaveStatus, TeeKind};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::sync::Mutex;

/// Stub Intel SGX provider.
pub struct IntelSgxProvider {
    enclaves: Mutex<Vec<EnclaveInfo>>,
}

impl IntelSgxProvider {
    /// Construct a new stub SGX provider.
    pub fn new() -> Self {
        Self {
            enclaves: Mutex::new(Vec::new()),
        }
    }

    fn mock_measurements() -> CodeMeasurements {
        // TODO: real impl reads MRENCLAVE / MRSIGNER from the SGX report
        CodeMeasurements {
            image_hash: String::new(),
            kernel_hash: None,
            application_hash: None,
            mrenclave: Some("a".repeat(64)),
            mrsigner: Some("b".repeat(64)),
        }
    }
}

impl Default for IntelSgxProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TeeProvider for IntelSgxProvider {
    fn name(&self) -> &str {
        "intel-sgx-stub"
    }

    fn kind(&self) -> TeeKind {
        TeeKind::IntelSgx
    }

    fn is_available(&self) -> bool {
        // TODO: real impl checks CPUID leaf 0x12, /dev/sgx_enclave presence,
        // and PSW service availability.
        false
    }

    async fn spawn_enclave(&self, config: EnclaveConfig) -> ArgentorResult<EnclaveInfo> {
        // TODO: real impl loads a signed .so enclave via sgx_create_enclave()
        // (SGX SDK) or launches a Gramine/Occlum container.
        config
            .validate()
            .map_err(|e| ArgentorError::Security(format!("invalid enclave config: {}", e)))?;
        if config.kind != TeeKind::IntelSgx {
            return Err(ArgentorError::Security(format!(
                "IntelSgxProvider cannot spawn {:?}",
                config.kind
            )));
        }

        let info = EnclaveInfo {
            enclave_id: format!("sgx-{:x}", rand_id()),
            kind: TeeKind::IntelSgx,
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
        // TODO: real impl calls sgx_destroy_enclave(eid)
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
        // TODO: real impl calls the DCAP quoting enclave to produce an ECDSA
        // attestation quote signed by Intel's root certificate.
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
        let mut report = AttestationReport::mock(TeeKind::IntelSgx, enclave_id, nonce);
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
        EnclaveConfig::production(TeeKind::IntelSgx, 256, 1)
    }

    #[test]
    fn construction_yields_empty_registry() {
        let p = IntelSgxProvider::new();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[test]
    fn name_and_kind() {
        let p = IntelSgxProvider::new();
        assert_eq!(p.name(), "intel-sgx-stub");
        assert_eq!(p.kind(), TeeKind::IntelSgx);
    }

    #[test]
    fn is_available_returns_false_for_stub() {
        assert!(!IntelSgxProvider::new().is_available());
    }

    #[test]
    fn default_equals_new() {
        let p = IntelSgxProvider::default();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn spawn_enclave_succeeds_with_valid_config() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert_eq!(info.kind, TeeKind::IntelSgx);
        assert_eq!(info.status, EnclaveStatus::Running);
        assert!(info.measurements.mrenclave.is_some());
        assert!(info.measurements.mrsigner.is_some());
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_invalid_memory() {
        let p = IntelSgxProvider::new();
        let mut c = cfg();
        c.memory_mb = 16;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_wrong_kind() {
        let p = IntelSgxProvider::new();
        let mut c = cfg();
        c.kind = TeeKind::AwsNitro;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn list_after_spawn() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let list = p.list_enclaves().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].enclave_id, info.enclave_id);
    }

    #[tokio::test]
    async fn list_is_empty_initially() {
        let p = IntelSgxProvider::new();
        assert!(p.list_enclaves().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminate_removes_enclave() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        p.terminate_enclave(&info.enclave_id).await.unwrap();
        assert!(p.list_enclaves().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminate_unknown_id_errors() {
        let p = IntelSgxProvider::new();
        assert!(p.terminate_enclave("sgx-ghost").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_empty_nonce() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert!(p.get_attestation(&info.enclave_id, "").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_unknown_enclave() {
        let p = IntelSgxProvider::new();
        assert!(p.get_attestation("sgx-missing", "n").await.is_err());
    }

    #[tokio::test]
    async fn attestation_ok() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p
            .get_attestation(&info.enclave_id, "sgx-nonce")
            .await
            .unwrap();
        assert_eq!(r.kind, TeeKind::IntelSgx);
        assert_eq!(r.nonce, "sgx-nonce");
    }

    #[tokio::test]
    async fn attestation_includes_sgx_measurements() {
        let p = IntelSgxProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p.get_attestation(&info.enclave_id, "n").await.unwrap();
        assert!(r.measurements.mrenclave.is_some());
        assert!(r.measurements.mrsigner.is_some());
    }

    #[tokio::test]
    async fn multiple_enclaves_distinct_ids() {
        let p = IntelSgxProvider::new();
        let a = p.spawn_enclave(cfg()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let b = p.spawn_enclave(cfg()).await.unwrap();
        assert_ne!(a.enclave_id, b.enclave_id);
    }
}
