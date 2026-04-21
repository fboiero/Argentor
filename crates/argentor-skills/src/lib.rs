//! Skill system with WASM-sandboxed runtime, plugin loading, and registry.
//!
//! This crate defines the skill abstraction used by Argentor agents,
//! including dynamic loading of WASM plugins, a central registry,
//! and markdown-based prompt skills.
//!
//! # Main types
//!
//! - [`Skill`] — Trait that every executable skill implements.
//! - [`SkillDescriptor`] — Metadata describing a skill's name, parameters, and capabilities.
//! - [`SkillRegistry`] — Central registry for discovering and invoking skills.
//! - [`WasmSkillRuntime`] — Wasmtime-based sandbox for running untrusted skill plugins.
//! - [`SkillLoader`] — Loads WASM skill plugins from configuration.
//! - [`MarkdownSkill`] — A skill defined via a Markdown file with YAML frontmatter.

/// Dynamic tool generation — agents create new tools at runtime from specs.
pub mod dynamic_gen;
/// WASM skill loader and configuration.
pub mod loader;
/// Markdown-based prompt and callable skills.
pub mod markdown_skill;
/// Skill marketplace: catalog, search, dependency resolution, and publishing.
pub mod marketplace;
/// Plugin system with manifest and event hooks.
pub mod plugin;
/// Central skill registry and tool groups.
pub mod registry;
/// Core skill trait and descriptor.
pub mod skill;
/// Fluent builder for defining skills without boilerplate (like `@tool` in Python SDKs).
pub mod tool_builder;
/// Skill vetting, signing, and secure registry index.
pub mod vetting;
/// Wasmtime-based WASM skill runtime.
pub mod wasm_runtime;

pub use loader::{SkillConfig, SkillLoader};
pub use markdown_skill::{LoadedMarkdownSkills, MarkdownSkill, MarkdownSkillLoader};
pub use marketplace::{
    builtin_catalog_entries, CatalogEntry, CompatibilityResult, InstalledSkill, MarketplaceCatalog,
    MarketplaceClient, MarketplaceEntry, MarketplaceManager, MarketplaceSearch, SearchResponse,
    SkillCache, SkillDependency, SortBy, UpgradeInfo,
};
pub use plugin::{Plugin, PluginEvent, PluginManifest, PluginRegistry};
pub use registry::{SkillRegistry, ToolGroup};
pub use skill::{Skill, SkillDescriptor};
pub use tool_builder::ToolBuilder;
pub use vetting::{SkillIndex, SkillManifest, SkillVetter, VettingResult};
pub use wasm_runtime::WasmSkillRuntime;
