#![allow(clippy::expect_used, missing_docs)]
use criterion::{black_box, criterion_group, criterion_main, Criterion};

use argentor_core::{ArgentorResult, ToolCall, ToolResult};
use argentor_skills::registry::ToolGroup;
use argentor_skills::skill::{Skill, SkillDescriptor};
use argentor_skills::vetting::{SkillManifest, SkillVetter};
use argentor_skills::SkillRegistry;
use async_trait::async_trait;
use std::sync::Arc;

struct BenchSkill {
    descriptor: SkillDescriptor,
}

impl BenchSkill {
    fn new(name: &str) -> Self {
        Self {
            descriptor: SkillDescriptor {
                name: name.to_string(),
                description: format!("Bench skill {name}"),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![],
                requires_approval: false,
            },
        }
    }
}

#[async_trait]
impl Skill for BenchSkill {
    fn descriptor(&self) -> &SkillDescriptor {
        &self.descriptor
    }
    async fn execute(&self, call: ToolCall) -> ArgentorResult<ToolResult> {
        Ok(ToolResult::success(&call.id, "ok"))
    }
}

fn bench_registry_lookup(c: &mut Criterion) {
    let mut registry = SkillRegistry::new();
    for i in 0..100 {
        registry.register(Arc::new(BenchSkill::new(&format!("skill_{i}"))));
    }

    c.bench_function("registry lookup (hit)", |b| {
        b.iter(|| registry.get(black_box("skill_50")));
    });

    c.bench_function("registry lookup (miss)", |b| {
        b.iter(|| registry.get(black_box("nonexistent")));
    });

    c.bench_function("list 100 descriptors", |b| {
        b.iter(|| registry.list_descriptors());
    });

    c.bench_function("filter_by_names (10 of 100)", |b| {
        let names: Vec<String> = (0..10).map(|i| format!("skill_{i}")).collect();
        b.iter(|| registry.filter_by_names(black_box(&names)));
    });
}

fn bench_registry_register(c: &mut Criterion) {
    c.bench_function("register 100 skills", |b| {
        b.iter(|| {
            let mut registry = SkillRegistry::new();
            for i in 0..100 {
                registry.register(Arc::new(BenchSkill::new(&format!("skill_{i}"))));
            }
            black_box(registry)
        });
    });
}

fn bench_skill_vetting(c: &mut Criterion) {
    // Minimal WASM module: magic + version
    let wasm_bytes: Vec<u8> = vec![
        0x00, 0x61, 0x73, 0x6D, // magic: \0asm
        0x01, 0x00, 0x00, 0x00, // version: 1
    ];

    c.bench_function("SkillManifest::compute_checksum", |b| {
        b.iter(|| SkillManifest::compute_checksum(black_box(&wasm_bytes)));
    });

    let manifest = SkillManifest {
        name: "bench-skill".into(),
        version: "1.0.0".into(),
        description: "Benchmark skill".into(),
        author: "bench".into(),
        license: None,
        checksum: SkillManifest::compute_checksum(&wasm_bytes),
        capabilities: vec![],
        signature: None,
        signer_key: None,
        min_argentor_version: None,
        tags: vec![],
        repository: None,
    };

    let vetter = SkillVetter::new();
    c.bench_function("SkillVetter::vet (unsigned)", |b| {
        b.iter(|| vetter.vet(black_box(&manifest), black_box(&wasm_bytes)));
    });
}

fn bench_descriptor_generation(c: &mut Criterion) {
    c.bench_function("SkillDescriptor creation (minimal)", |b| {
        b.iter(|| {
            black_box(SkillDescriptor {
                name: black_box("echo").to_string(),
                description: black_box("Echoes input back").to_string(),
                parameters_schema: serde_json::json!({}),
                required_capabilities: vec![],
                requires_approval: false,
            })
        });
    });

    c.bench_function("SkillDescriptor creation (with schema)", |b| {
        b.iter(|| {
            black_box(SkillDescriptor {
                name: black_box("file_read").to_string(),
                description: black_box("Read a file from disk").to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path to read" },
                        "offset": { "type": "integer", "description": "Line offset" },
                        "limit": { "type": "integer", "description": "Max lines to read" }
                    },
                    "required": ["path"]
                }),
                required_capabilities: vec![argentor_security::Capability::FileRead {
                    allowed_paths: vec!["/tmp".into(), "/workspace".into()],
                }],
                requires_approval: false,
            })
        });
    });

    c.bench_function("SkillDescriptor serialize", |b| {
        let desc = SkillDescriptor {
            name: "shell".to_string(),
            description: "Execute a shell command in the sandbox".to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout": { "type": "integer" }
                },
                "required": ["command"]
            }),
            required_capabilities: vec![argentor_security::Capability::ShellExec {
                allowed_commands: vec!["ls".into(), "echo".into(), "cat".into()],
            }],
            requires_approval: false,
        };
        b.iter(|| serde_json::to_string(black_box(&desc)));
    });
}

fn bench_registry_lookup_10(c: &mut Criterion) {
    // Realistic skill names matching Argentor builtins
    let skill_names = [
        "echo",
        "time",
        "help",
        "file_read",
        "file_write",
        "shell",
        "http_fetch",
        "browser",
        "memory_store",
        "memory_search",
        "agent_delegate",
        "task_status",
        "human_approval",
        "artifact_store",
    ];

    let mut registry = SkillRegistry::new();
    for name in &skill_names {
        registry.register(Arc::new(BenchSkill::new(name)));
    }

    c.bench_function("registry lookup (hit, 14 skills)", |b| {
        b.iter(|| registry.get(black_box("shell")));
    });

    c.bench_function("registry lookup (miss, 14 skills)", |b| {
        b.iter(|| registry.get(black_box("nonexistent")));
    });

    c.bench_function("registry lookup (first registered)", |b| {
        b.iter(|| registry.get(black_box("echo")));
    });

    c.bench_function("registry lookup (last registered)", |b| {
        b.iter(|| registry.get(black_box("artifact_store")));
    });

    c.bench_function("list descriptors (14 skills)", |b| {
        b.iter(|| black_box(registry.list_descriptors()));
    });
}

fn bench_tool_group_filtering(c: &mut Criterion) {
    let skill_names = [
        "echo",
        "time",
        "help",
        "file_read",
        "file_write",
        "shell",
        "http_fetch",
        "browser",
        "memory_store",
        "memory_search",
        "agent_delegate",
        "task_status",
        "human_approval",
        "artifact_store",
    ];

    let mut registry = SkillRegistry::new();
    for name in &skill_names {
        registry.register(Arc::new(BenchSkill::new(name)));
    }

    c.bench_function("filter_by_group (minimal, 3 skills)", |b| {
        b.iter(|| registry.filter_by_group(black_box("minimal")));
    });

    c.bench_function("filter_by_group (coding, 5 skills)", |b| {
        b.iter(|| registry.filter_by_group(black_box("coding")));
    });

    c.bench_function("filter_by_group (full, all skills)", |b| {
        b.iter(|| registry.filter_by_group(black_box("full")));
    });

    c.bench_function("filter_by_group (orchestration)", |b| {
        b.iter(|| registry.filter_by_group(black_box("orchestration")));
    });

    c.bench_function("skills_in_group (minimal)", |b| {
        b.iter(|| registry.skills_in_group(black_box("minimal")));
    });

    c.bench_function("skills_in_group (development)", |b| {
        b.iter(|| registry.skills_in_group(black_box("development")));
    });

    c.bench_function("filter_by_names (5 of 14)", |b| {
        let names: Vec<String> = vec![
            "echo".into(),
            "shell".into(),
            "file_read".into(),
            "memory_store".into(),
            "http_fetch".into(),
        ];
        b.iter(|| registry.filter_by_names(black_box(&names)));
    });

    c.bench_function("filter_to_new (coding group skills)", |b| {
        let names: Vec<String> = vec![
            "file_read".into(),
            "file_write".into(),
            "shell".into(),
            "memory_store".into(),
            "memory_search".into(),
        ];
        b.iter(|| registry.filter_to_new(black_box(&names)));
    });

    // Custom group registration + filtering
    c.bench_function("register_group + filter_by_group", |b| {
        b.iter(|| {
            let mut reg = registry.filter_by_group("full").expect("full group");
            reg.register_group(ToolGroup::new(
                "custom_bench",
                "Custom benchmark group",
                vec!["echo".into(), "shell".into(), "file_read".into()],
            ));
            black_box(reg.filter_by_group("custom_bench"))
        });
    });
}

criterion_group!(
    benches,
    bench_registry_lookup,
    bench_registry_register,
    bench_skill_vetting,
    bench_descriptor_generation,
    bench_registry_lookup_10,
    bench_tool_group_filtering,
);
criterion_main!(benches);
