//! AWS Nitro Enclaves stub provider.
//!
//! Real integration requires:
//! - `aws-nitro-enclaves-cli` installed on the parent EC2 instance
//! - `aws-nitro-enclaves-sdk-rust` for KMS integration
//! - EC2 instance with Nitro Enclaves enabled (nitro_enclaves=enabled)
//! - `.eif` (Enclave Image File) built with `nitro-cli build-enclave`
//!
//! See <https://docs.aws.amazon.com/enclaves/latest/user/nitro-enclave.html>.

use crate::attestation::AttestationReport;
use crate::provider::TeeProvider;
use crate::types::{CodeMeasurements, EnclaveConfig, EnclaveInfo, EnclaveStatus, TeeKind};
use argentor_core::{ArgentorError, ArgentorResult};
use async_trait::async_trait;
use std::sync::Mutex;

/// Stub AWS Nitro Enclaves provider.
///
/// Keeps an in-memory registry of pretend enclaves so the scaffolding can
/// round-trip `spawn → list → terminate` without hardware.
pub struct AwsNitroProvider {
    enclaves: Mutex<Vec<EnclaveInfo>>,
}

impl AwsNitroProvider {
    /// Construct a new stub provider.
    pub fn new() -> Self {
        Self {
            enclaves: Mutex::new(Vec::new()),
        }
    }

    fn mock_measurements() -> CodeMeasurements {
        // TODO: real impl reads PCR0 / PCR1 / PCR2 from nitro-cli describe-enclaves
        CodeMeasurements {
            image_hash: "0".repeat(96),
            kernel_hash: Some("1".repeat(96)),
            application_hash: Some("2".repeat(96)),
            mrenclave: None,
            mrsigner: None,
        }
    }
}

impl Default for AwsNitroProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TeeProvider for AwsNitroProvider {
    fn name(&self) -> &str {
        "aws-nitro-stub"
    }

    fn kind(&self) -> TeeKind {
        TeeKind::AwsNitro
    }

    fn is_available(&self) -> bool {
        // TODO: real impl checks for /dev/nitro_enclaves and nitro-cli binary
        false
    }

    async fn spawn_enclave(&self, config: EnclaveConfig) -> ArgentorResult<EnclaveInfo> {
        // TODO: real impl with aws-nitro-enclaves-cli:
        //   nitro-cli run-enclave --eif-path ... --memory ... --cpu-count ...
        config
            .validate()
            .map_err(|e| ArgentorError::Security(format!("invalid enclave config: {}", e)))?;
        if config.kind != TeeKind::AwsNitro {
            return Err(ArgentorError::Security(format!(
                "AwsNitroProvider cannot spawn {:?}",
                config.kind
            )));
        }

        let info = EnclaveInfo {
            enclave_id: format!("i-{:x}", rand_id()),
            kind: TeeKind::AwsNitro,
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
        // TODO: real impl with nitro-cli terminate-enclave --enclave-id ...
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
        // TODO: real impl calls NSM (Nitro Security Module) via
        // aws-nitro-enclaves-sdk-rust and returns a COSE_Sign1 document
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
        Ok(AttestationReport::mock(
            TeeKind::AwsNitro,
            enclave_id,
            nonce,
        ))
    }

    async fn list_enclaves(&self) -> ArgentorResult<Vec<EnclaveInfo>> {
        // TODO: real impl calls nitro-cli describe-enclaves
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
        EnclaveConfig::production(TeeKind::AwsNitro, 2048, 2)
    }

    #[test]
    fn construction_yields_empty_registry() {
        let p = AwsNitroProvider::new();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[test]
    fn name_and_kind() {
        let p = AwsNitroProvider::new();
        assert_eq!(p.name(), "aws-nitro-stub");
        assert_eq!(p.kind(), TeeKind::AwsNitro);
    }

    #[test]
    fn is_available_returns_false_for_stub() {
        let p = AwsNitroProvider::new();
        assert!(!p.is_available());
    }

    #[test]
    fn default_is_same_as_new() {
        let p = AwsNitroProvider::default();
        assert!(p.enclaves.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn spawn_enclave_succeeds_with_valid_config() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert_eq!(info.kind, TeeKind::AwsNitro);
        assert_eq!(info.status, EnclaveStatus::Running);
        assert!(!info.enclave_id.is_empty());
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_invalid_memory() {
        let p = AwsNitroProvider::new();
        let mut c = cfg();
        c.memory_mb = 32;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn spawn_enclave_rejects_wrong_kind() {
        let p = AwsNitroProvider::new();
        let mut c = cfg();
        c.kind = TeeKind::IntelSgx;
        assert!(p.spawn_enclave(c).await.is_err());
    }

    #[tokio::test]
    async fn spawn_then_list_contains_enclave() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let list = p.list_enclaves().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].enclave_id, info.enclave_id);
    }

    #[tokio::test]
    async fn list_is_empty_initially() {
        let p = AwsNitroProvider::new();
        let list = p.list_enclaves().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn terminate_removes_enclave() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        p.terminate_enclave(&info.enclave_id).await.unwrap();
        let list = p.list_enclaves().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn terminate_unknown_id_errors() {
        let p = AwsNitroProvider::new();
        assert!(p.terminate_enclave("i-ghost").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_empty_nonce() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        assert!(p.get_attestation(&info.enclave_id, "").await.is_err());
    }

    #[tokio::test]
    async fn attestation_rejects_unknown_enclave() {
        let p = AwsNitroProvider::new();
        assert!(p.get_attestation("i-ghost", "n1").await.is_err());
    }

    #[tokio::test]
    async fn attestation_succeeds_with_valid_params() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p
            .get_attestation(&info.enclave_id, "nonce-1")
            .await
            .unwrap();
        assert_eq!(r.kind, TeeKind::AwsNitro);
        assert_eq!(r.enclave_id, info.enclave_id);
        assert_eq!(r.nonce, "nonce-1");
    }

    #[tokio::test]
    async fn attestation_measurements_match_stub_shape() {
        let p = AwsNitroProvider::new();
        let info = p.spawn_enclave(cfg()).await.unwrap();
        let r = p.get_attestation(&info.enclave_id, "n").await.unwrap();
        assert_eq!(r.measurements.image_hash.len(), 96);
        assert!(r.measurements.kernel_hash.is_some());
        assert!(r.measurements.application_hash.is_some());
    }

    #[tokio::test]
    async fn spawning_multiple_enclaves_produces_distinct_ids() {
        let p = AwsNitroProvider::new();
        let a = p.spawn_enclave(cfg()).await.unwrap();
        // ensure nanosecond clock advances
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        let b = p.spawn_enclave(cfg()).await.unwrap();
        assert_ne!(a.enclave_id, b.enclave_id);
        let list = p.list_enclaves().await.unwrap();
        assert_eq!(list.len(), 2);
    }
}
