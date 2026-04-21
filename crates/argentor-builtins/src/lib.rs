//! Built-in skills for the Argentor framework.
//!
//! Provides ready-to-use skills covering shell execution, file I/O, HTTP fetching,
//! semantic memory, artifact storage, browser automation, Docker sandboxing,
//! human-in-the-loop approval, and agent delegation.
//!
//! # Main entry points
//!
//! - [`register_builtins()`] — Register the standard set of built-in skills.
//! - [`register_builtins_with_memory()`] — Register builtins including memory skills.
//! - [`register_builtins_with_approval()`] — Register builtins with a custom approval channel.
//! - [`register_all()`] — Register builtins with memory and approval.
//! - [`register_orchestration_builtins()`] — Register orchestration-specific skills.
//! - [`register_builtins_with_browser()`] — Register builtins with browser automation.

/// Agent delegation skill for sub-agent spawning.
pub mod agent_delegate;
/// Artifact storage skill and backends.
pub mod artifact_store;
/// Simple browser skill (URL fetching).
pub mod browser;
/// WebDriver-based browser automation skill.
pub mod browser_automation;
/// Pure-math calculator skill for precise computations.
pub mod calculator;
/// Language-aware code analysis skill.
pub mod code_analysis;
/// Color conversion skill (Hex/RGB/HSL, contrast ratio, lighten/darken).
pub mod color_converter;
/// Cron expression parsing, validation, and scheduling skill.
pub mod cron_parser;
/// CSV parsing, filtering, sorting, statistics, and format conversion.
pub mod csv_processor;
/// Data format validation skill.
pub mod data_validator;
/// Date/time operations skill.
pub mod datetime_tool;
/// Text diff generation and patching skill.
pub mod diff_tool;
/// DNS lookup, reverse resolution, and connectivity checks.
pub mod dns_lookup;
/// Docker-sandboxed shell execution.
pub mod docker_sandbox;
/// DOCX (Microsoft Word) document loader skill.
pub mod docx_loader;
/// Encoding/decoding skill (Base64, hex, URL, HTML, JWT).
pub mod encode_decode;
/// Environment variable management and .env file parsing skill.
pub mod env_manager;
/// EPUB ebook loader skill.
pub mod epub_loader;
/// Excel (XLSX) spreadsheet loader skill.
pub mod excel_loader;
/// File-system-based artifact backend for persistent storage.
pub mod file_artifact_backend;
/// File hashing skill (SHA-256, SHA-512, MD5, checksum verification).
pub mod file_hasher;
/// File read skill.
pub mod file_read;
/// File write skill.
pub mod file_write;
/// Git operations skill (libgit2-based, no shell commands).
pub mod git;
/// Cryptographic hashing skill (SHA-256, SHA-512, HMAC-SHA256).
pub mod hash_tool;
/// HTML document loader skill.
pub mod html_loader;
/// HTTP fetch skill.
pub mod http_fetch;
/// Human-in-the-loop approval skill and channels.
pub mod human_approval;
/// IP address tools skill (parsing, CIDR, subnet calculation).
pub mod ip_tools;
/// JSON query and manipulation skill.
pub mod json_query;
/// JWT inspection skill (decode, claims, expiry check).
pub mod jwt_tool;
/// Knowledge graph skill for entity-relationship operations.
pub mod knowledge_graph_skill;
/// Markdown processing skill (plain text, headings, links, TOC).
pub mod markdown_renderer;
/// Semantic memory store and search skills.
pub mod memory;
/// In-memory metrics collection skill (counters, gauges, histograms).
pub mod metrics_collector;
/// PDF document loader skill.
pub mod pdf_loader;
/// PowerPoint (PPTX) presentation loader skill.
pub mod pptx_loader;
/// Prompt injection detection and PII scanning skill.
pub mod prompt_guard;
/// Regex operations skill.
pub mod regex_tool;
/// RSS/Atom feed reader and search.
pub mod rss_reader;
/// SDK client code generator for Python and TypeScript.
pub mod sdk_generator;
/// Secret scanning skill for detecting leaked credentials.
pub mod secret_scanner;
/// Semantic versioning skill (parse, compare, bump, range matching).
pub mod semver_tool;
/// Shell command execution skill.
pub mod shell;
/// Stdin-based interactive approval channel.
pub mod stdin_approval;
/// Extractive text summarization skill.
pub mod summarizer;
/// Task status reporting skill.
pub mod task_status;
/// Simple template engine skill ({{variable}} rendering, conditionals, loops).
pub mod template_engine;
/// Test runner skill for multi-language test execution and result parsing.
pub mod test_runner;
/// Text transformation skill for string manipulation operations.
pub mod text_transform;
/// UUID generation and parsing skill.
pub mod uuid_generator;
/// Web scraping skill for extracting text, links, metadata from web pages.
pub mod web_scraper;
/// Web search skill using DuckDuckGo HTML endpoint.
pub mod web_search;
/// XcapitSFF backend integration skills.
pub mod xcapitsff_skills;
/// YAML processing skill (parse, stringify, validate, merge, conversion).
pub mod yaml_processor;
/// Internal minimal ZIP archive reader for OOXML loaders.
pub mod zip_reader;

pub use agent_delegate::{AgentDelegateSkill, TaskInfo, TaskQueueHandle, TaskSummary};
pub use artifact_store::{ArtifactBackend, ArtifactStoreSkill, InMemoryArtifactBackend};
pub use browser::BrowserSkill;
pub use browser_automation::{BrowserAction, BrowserAutomationSkill, BrowserConfig, BrowserResult};
pub use calculator::CalculatorSkill;
pub use code_analysis::CodeAnalysisSkill;
pub use color_converter::ColorConverterSkill;
pub use cron_parser::CronParserSkill;
pub use csv_processor::CsvProcessorSkill;
pub use data_validator::DataValidatorSkill;
pub use datetime_tool::DateTimeSkill;
pub use diff_tool::DiffSkill;
pub use dns_lookup::DnsLookupSkill;
pub use docx_loader::DocxLoaderSkill;
pub use encode_decode::EncodeDecodeSkill;
pub use env_manager::EnvManagerSkill;
pub use epub_loader::EpubLoaderSkill;
pub use excel_loader::ExcelLoaderSkill;
pub use file_artifact_backend::FileArtifactBackend;
pub use file_hasher::FileHasherSkill;
pub use file_read::FileReadSkill;
pub use file_write::FileWriteSkill;
pub use git::GitSkill;
pub use hash_tool::HashSkill;
pub use html_loader::HtmlLoaderSkill;
pub use http_fetch::HttpFetchSkill;
pub use human_approval::{
    ApprovalChannel, ApprovalDecision, ApprovalRequest, AutoApproveChannel,
    CallbackApprovalChannel, HumanApprovalSkill, RiskLevel,
};
pub use ip_tools::IpToolsSkill;
pub use json_query::JsonQuerySkill;
pub use jwt_tool::JwtToolSkill;
pub use knowledge_graph_skill::KnowledgeGraphSkill;
pub use markdown_renderer::MarkdownRendererSkill;
pub use memory::{MemorySearchSkill, MemoryStoreSkill};
pub use metrics_collector::MetricsCollectorSkill;
pub use pdf_loader::PdfLoaderSkill;
pub use pptx_loader::PptxLoaderSkill;
pub use prompt_guard::PromptGuardSkill;
pub use regex_tool::RegexSkill;
pub use rss_reader::RssReaderSkill;
pub use sdk_generator::SdkGenerator;
pub use secret_scanner::SecretScannerSkill;
pub use semver_tool::SemverToolSkill;
pub use shell::{CommandPolicy, ShellSkill};
pub use stdin_approval::StdinApprovalChannel;
pub use summarizer::SummarizerSkill;
pub use task_status::TaskStatusSkill;
pub use template_engine::TemplateEngineSkill;
pub use test_runner::TestRunnerSkill;
pub use text_transform::TextTransformSkill;
pub use uuid_generator::UuidGeneratorSkill;
pub use web_scraper::WebScraperSkill;
pub use web_search::{SearchProvider, WebSearchSkill};
pub use xcapitsff_skills::{
    register_xcapitsff_skills, XcapitCustomer360Skill, XcapitKbSearchSkill, XcapitLeadInfoSkill,
    XcapitSearchSkill, XcapitTicketInfoSkill,
};
pub use yaml_processor::YamlProcessorSkill;

pub use docker_sandbox::{DockerSandboxConfig, ExecResult};

#[cfg(feature = "docker")]
pub use docker_sandbox::{DockerSandbox, DockerShellSkill};

#[cfg(feature = "browser")]
pub use browser_automation::BrowserAutomation;

use argentor_memory::{EmbeddingProvider, VectorStore};
use argentor_skills::SkillRegistry;
use std::sync::Arc;

/// Register the 29 utility skills inspired by Vercel AI SDK, LangChain, CrewAI,
/// AutoGPT, and Semantic Kernel. These are self-contained (no external API keys).
///
/// **Data & Text:** calculator, text_transform, json_query, regex_tool, data_validator,
///   datetime_tool, csv_processor, yaml_processor, markdown_renderer, template_engine
/// **Crypto & Encoding:** hash_tool, encode_decode, uuid_generator, jwt_tool, file_hasher
/// **Versioning & Config:** semver_tool, env_manager, cron_parser
/// **Web & Network:** web_search, web_scraper, rss_reader, dns_lookup, ip_tools
/// **Security & AI:** prompt_guard, secret_scanner, diff_tool, summarizer
/// **Observability:** metrics_collector, color_converter
pub fn register_utility_skills(registry: &SkillRegistry) {
    // Data & Text
    registry.register(Arc::new(CalculatorSkill::default()));
    registry.register(Arc::new(TextTransformSkill::default()));
    registry.register(Arc::new(JsonQuerySkill::default()));
    registry.register(Arc::new(RegexSkill::default()));
    registry.register(Arc::new(DataValidatorSkill::default()));
    registry.register(Arc::new(DateTimeSkill::default()));
    registry.register(Arc::new(CsvProcessorSkill::default()));
    registry.register(Arc::new(YamlProcessorSkill::default()));
    registry.register(Arc::new(MarkdownRendererSkill::default()));
    registry.register(Arc::new(TemplateEngineSkill::default()));
    // Crypto & Encoding
    registry.register(Arc::new(HashSkill::default()));
    registry.register(Arc::new(EncodeDecodeSkill::default()));
    registry.register(Arc::new(UuidGeneratorSkill::default()));
    registry.register(Arc::new(JwtToolSkill::default()));
    registry.register(Arc::new(FileHasherSkill::default()));
    // Versioning & Config
    registry.register(Arc::new(SemverToolSkill::default()));
    registry.register(Arc::new(EnvManagerSkill::default()));
    registry.register(Arc::new(CronParserSkill::default()));
    // Web & Network
    registry.register(Arc::new(WebSearchSkill::default()));
    registry.register(Arc::new(WebScraperSkill::default()));
    registry.register(Arc::new(RssReaderSkill::default()));
    registry.register(Arc::new(DnsLookupSkill::default()));
    registry.register(Arc::new(IpToolsSkill::default()));
    // Security & AI
    registry.register(Arc::new(PromptGuardSkill::default()));
    registry.register(Arc::new(SecretScannerSkill::default()));
    registry.register(Arc::new(DiffSkill::default()));
    registry.register(Arc::new(SummarizerSkill::default()));
    // Observability & Utilities
    registry.register(Arc::new(MetricsCollectorSkill::new()));
    registry.register(Arc::new(ColorConverterSkill::default()));
    // Document Loaders (RAG)
    registry.register(Arc::new(PdfLoaderSkill::default()));
    registry.register(Arc::new(DocxLoaderSkill::default()));
    registry.register(Arc::new(HtmlLoaderSkill::default()));
    registry.register(Arc::new(EpubLoaderSkill::default()));
    registry.register(Arc::new(ExcelLoaderSkill::default()));
    registry.register(Arc::new(PptxLoaderSkill::default()));
}

/// Register all built-in skills into the given registry.
/// Uses the provided vector store and embedding provider for memory skills.
pub fn register_builtins_with_memory(
    registry: &SkillRegistry,
    store: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
) {
    registry.register(Arc::new(ShellSkill::new()));
    registry.register(Arc::new(FileReadSkill::new()));
    registry.register(Arc::new(FileWriteSkill::new()));
    registry.register(Arc::new(HttpFetchSkill::new()));
    registry.register(Arc::new(BrowserSkill::new()));
    registry.register(Arc::new(GitSkill::new()));
    registry.register(Arc::new(CodeAnalysisSkill::new()));
    registry.register(Arc::new(TestRunnerSkill::new()));
    registry.register(Arc::new(MemoryStoreSkill::new(
        store.clone(),
        embedder.clone(),
    )));
    registry.register(Arc::new(MemorySearchSkill::new(store, embedder)));
    registry.register(Arc::new(HumanApprovalSkill::auto_approve()));
    register_utility_skills(registry);
}

/// Register built-in skills without memory (backwards compatible).
pub fn register_builtins(registry: &SkillRegistry) {
    registry.register(Arc::new(ShellSkill::new()));
    registry.register(Arc::new(FileReadSkill::new()));
    registry.register(Arc::new(FileWriteSkill::new()));
    registry.register(Arc::new(HttpFetchSkill::new()));
    registry.register(Arc::new(BrowserSkill::new()));
    registry.register(Arc::new(GitSkill::new()));
    registry.register(Arc::new(CodeAnalysisSkill::new()));
    registry.register(Arc::new(TestRunnerSkill::new()));
    registry.register(Arc::new(HumanApprovalSkill::auto_approve()));
    register_utility_skills(registry);
}

/// Register built-in skills with a custom approval channel for HITL.
pub fn register_builtins_with_approval(
    registry: &SkillRegistry,
    approval_channel: Arc<dyn ApprovalChannel>,
) {
    registry.register(Arc::new(ShellSkill::new()));
    registry.register(Arc::new(FileReadSkill::new()));
    registry.register(Arc::new(FileWriteSkill::new()));
    registry.register(Arc::new(HttpFetchSkill::new()));
    registry.register(Arc::new(BrowserSkill::new()));
    registry.register(Arc::new(GitSkill::new()));
    registry.register(Arc::new(CodeAnalysisSkill::new()));
    registry.register(Arc::new(TestRunnerSkill::new()));
    registry.register(Arc::new(HumanApprovalSkill::new(approval_channel)));
    register_utility_skills(registry);
}

/// Register all built-in skills including memory and a custom approval channel.
pub fn register_all(
    registry: &SkillRegistry,
    store: Arc<dyn VectorStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    approval_channel: Arc<dyn ApprovalChannel>,
) {
    registry.register(Arc::new(ShellSkill::new()));
    registry.register(Arc::new(FileReadSkill::new()));
    registry.register(Arc::new(FileWriteSkill::new()));
    registry.register(Arc::new(HttpFetchSkill::new()));
    registry.register(Arc::new(BrowserSkill::new()));
    registry.register(Arc::new(GitSkill::new()));
    registry.register(Arc::new(CodeAnalysisSkill::new()));
    registry.register(Arc::new(TestRunnerSkill::new()));
    registry.register(Arc::new(MemoryStoreSkill::new(
        store.clone(),
        embedder.clone(),
    )));
    registry.register(Arc::new(MemorySearchSkill::new(store, embedder)));
    registry.register(Arc::new(HumanApprovalSkill::new(approval_channel)));
    register_utility_skills(registry);
}

/// Register orchestration-specific skills (artifact_store, agent_delegate, task_status).
/// These require a TaskQueueHandle and ArtifactBackend from the orchestrator.
pub fn register_orchestration_builtins(
    registry: &SkillRegistry,
    queue: Arc<dyn TaskQueueHandle>,
    artifact_backend: Arc<dyn ArtifactBackend>,
) {
    registry.register(Arc::new(ArtifactStoreSkill::new(artifact_backend)));
    registry.register(Arc::new(AgentDelegateSkill::new(queue.clone())));
    registry.register(Arc::new(TaskStatusSkill::new(queue)));
}

/// Register built-in skills plus the browser automation skill.
///
/// This registers all the standard builtins and adds `BrowserAutomationSkill`
/// configured with the given `BrowserConfig`. The actual WebDriver connection
/// is established lazily when the skill is first invoked, and only when the
/// `browser` feature is enabled.
pub fn register_builtins_with_browser(registry: &SkillRegistry, config: BrowserConfig) {
    registry.register(Arc::new(ShellSkill::new()));
    registry.register(Arc::new(FileReadSkill::new()));
    registry.register(Arc::new(FileWriteSkill::new()));
    registry.register(Arc::new(HttpFetchSkill::new()));
    registry.register(Arc::new(BrowserSkill::new()));
    registry.register(Arc::new(GitSkill::new()));
    registry.register(Arc::new(CodeAnalysisSkill::new()));
    registry.register(Arc::new(TestRunnerSkill::new()));
    registry.register(Arc::new(HumanApprovalSkill::auto_approve()));
    registry.register(Arc::new(BrowserAutomationSkill::new(config)));
}
