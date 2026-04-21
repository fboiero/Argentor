//! Argentor TEE — Trusted Execution Environment integration.
//!
//! Supports AWS Nitro Enclaves, Intel SGX, AMD SEV-SNP for confidential computing.
//! Critical for compliance use cases (banking, healthcare, government).
//!
//! # Status
//!
//! All providers are STUBS in v1.x. Real integration requires:
//!
//! - **AWS Nitro**: `aws-nitro-enclaves-cli` + `aws-nitro-enclaves-sdk-rust`
//! - **Intel SGX**: Gramine or Occlum runtime + `sgx-sdk`
//! - **AMD SEV**: KVM + `sev-guest` crate
//!
//! See `examples/` for usage patterns once real backends are wired.
//!
//! # Overview
//!
//! A TEE (Trusted Execution Environment) is a hardware-isolated region of a
//! processor that protects code and data from the rest of the system — including
//! the operating system, hypervisor, and cloud provider. Argentor uses TEEs to
//! make sensitive agent workloads invisible to the underlying infrastructure.
//!
//! # Quick example
//!
//! ```no_run
//! use argentor_tee::{TeeKind, EnclaveConfig};
//!
//! let cfg = EnclaveConfig {
//!     kind: TeeKind::AwsNitro,
//!     memory_mb: 2048,
//!     cpu_count: 2,
//!     debug_mode: false,
//!     enclave_image_path: Some("/opt/argentor/enclave.eif".into()),
//! };
//! ```

#![allow(clippy::uninlined_format_args)]

pub mod attestation;
pub mod provider;
pub mod types;

#[cfg(feature = "amd-sev")]
pub mod amd_sev;
#[cfg(feature = "aws-nitro")]
pub mod aws_nitro;
#[cfg(feature = "intel-sgx")]
pub mod intel_sgx;

pub use attestation::{
    AttestationReport, AttestationVerifier, ExpectedMeasurements, VerificationResult,
};
pub use provider::TeeProvider;
pub use types::{CodeMeasurements, EnclaveConfig, EnclaveInfo, EnclaveStatus, TeeKind};
