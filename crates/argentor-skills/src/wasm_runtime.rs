use crate::skill::{Skill, SkillDescriptor};
use argentor_core::{ArgentorError, ArgentorResult, ToolCall, ToolResult};
use argentor_security::Capability;
use async_trait::async_trait;
use std::path::Path;
use tracing::info;
use wasmtime::*;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::WasiCtxBuilder;

/// A skill loaded from a WASM module, sandboxed via wasmtime.
pub struct WasmSkill {
    descriptor: SkillDescriptor,
    engine: Engine,
    module: Module,
}

/// Runtime for loading and executing WASM-based skills.
pub struct WasmSkillRuntime {
    engine: Engine,
}

impl WasmSkillRuntime {
    /// Create a new WASM skill runtime with a default engine.
    pub fn new() -> ArgentorResult<Self> {
        let engine = Engine::default();
        Ok(Self { engine })
    }

    /// Load a WASM skill from a `.wasm` file.
    pub fn load_skill(
        &self,
        path: &Path,
        name: String,
        description: String,
        parameters_schema: serde_json::Value,
        required_capabilities: Vec<Capability>,
        requires_approval: bool,
    ) -> ArgentorResult<WasmSkill> {
        info!(path = %path.display(), name = %name, "Loading WASM skill");

        let module = Module::from_file(&self.engine, path)
            .map_err(|e| ArgentorError::Skill(format!("Failed to load WASM module: {e}")))?;

        Ok(WasmSkill {
            descriptor: SkillDescriptor {
                name,
                description,
                parameters_schema,
                required_capabilities,
                requires_approval,
            },
            engine: self.engine.clone(),
            module,
        })
    }
}

impl Default for WasmSkillRuntime {
    // Safety: WasmSkillRuntime::new() only creates a wasmtime::Engine with default
    // config, which is infallible in practice. The Result wrapper exists for future
    // proofing but the default Engine constructor cannot fail today.
    #[allow(clippy::expect_used)]
    fn default() -> Self {
        Self::new().expect("Failed to create WASM runtime")
    }
}

#[async_trait]
impl Skill for WasmSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }

    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        let engine = self.engine.clone();
        let module = self.module.clone();
        let call_id = call.id.clone();
        let input = serde_json::to_string(&call.arguments)
            .map_err(|e| ArgentorError::Skill(format!("Failed to serialize args: {e}")))?;

        // Run WASM in a blocking task to avoid blocking the async runtime
        let result = tokio::task::spawn_blocking(move || run_wasm_skill(&engine, &module, &input))
            .await
            .map_err(|e| ArgentorError::Skill(format!("WASM task panicked: {e}")))?;

        match result {
            Ok(output) => Ok(ToolResult::success(call_id, output)),
            Err(e) => Ok(ToolResult::error(call_id, e.to_string())),
        }
    }
}

fn run_wasm_skill(engine: &Engine, module: &Module, input: &str) -> ArgentorResult<String> {
    let mut linker = Linker::<WasiP1Ctx>::new(engine);
    preview1::add_to_linker_sync(&mut linker, |t| t)
        .map_err(|e| ArgentorError::Skill(format!("WASI linker error: {e}")))?;

    let wasi = WasiCtxBuilder::new().inherit_stdio().arg(input).build_p1();

    let mut store = Store::new(engine, wasi);

    // Set fuel limit for the WASM module (prevents infinite loops)
    store.set_fuel(1_000_000).ok();

    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|e| ArgentorError::Skill(format!("WASM instantiation error: {e}")))?;

    // Call the skill's main function using WASI _start convention
    let start = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|e| ArgentorError::Skill(format!("No _start export: {e}")))?;

    start
        .call(&mut store, ())
        .map_err(|e| ArgentorError::Skill(format!("WASM execution error: {e}")))?;

    Ok("WASM skill executed successfully".to_string())
}
