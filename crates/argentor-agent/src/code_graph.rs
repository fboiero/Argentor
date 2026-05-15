//! Lightweight code understanding module with regex-based parsing.
//!
//! Builds a symbol table, dependency graph, and call graph from source files
//! in multiple languages (Rust, Python, TypeScript, Go). This enables agents
//! to understand codebases before making changes.
//!
//! # Main types
//!
//! - [`CodeGraph`] — The central structure holding symbols, dependencies, and calls.
//! - [`CodeSymbol`] — A named code entity (function, struct, class, etc.) with location.
//! - [`Dependency`] — An import/dependency edge between files.
//! - [`CallRef`] — A function call reference linking caller to callee.
//! - [`ImpactAnalysis`] — Result of analyzing what would be affected by changing a symbol.
//! - [`CodeContext`] — Relevant code context for a given task description.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Supported programming languages for analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    /// Rust (.rs)
    Rust,
    /// Python (.py)
    Python,
    /// TypeScript/JavaScript (.ts, .tsx, .js, .jsx)
    TypeScript,
    /// Go (.go)
    Go,
    /// Unrecognized file type.
    Unknown,
}

/// Kind of code symbol.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// A standalone function.
    Function,
    /// A method inside an impl block, class, or struct.
    Method,
    /// A Rust struct or Go struct.
    Struct,
    /// A Python or TypeScript class.
    Class,
    /// A Rust trait.
    Trait,
    /// A TypeScript or Go interface.
    Interface,
    /// A Rust enum.
    Enum,
    /// A module declaration.
    Module,
    /// An import statement.
    Import,
    /// A constant binding.
    Constant,
    /// A type alias.
    TypeAlias,
}

/// Visibility of a code symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    /// Public (Rust `pub`, Python module-level, Go exported).
    Public,
    /// Private (Rust default, Go unexported, Python `_` prefix).
    Private,
    /// Protected (Python `__` prefix, TypeScript `protected`).
    Protected,
}

/// Risk level for an impact analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Few dependents, change is safe.
    Low,
    /// Some dependents, moderate care needed.
    Medium,
    /// Many dependents or public API, careful review required.
    High,
    /// Core symbol with wide transitive impact.
    Critical,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A code symbol with its location and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSymbol {
    /// The symbol's name (e.g., `"AgentRunner"`, `"new"`).
    pub name: String,
    /// What kind of symbol this is.
    pub kind: SymbolKind,
    /// File path where the symbol is defined.
    pub file: String,
    /// 1-based line number of the definition.
    pub line: usize,
    /// Whether the symbol is public, private, or protected.
    pub visibility: Visibility,
    /// Programming language of the source file.
    pub language: Language,
    /// Optional full signature (e.g., `"fn foo(x: i32) -> bool"`).
    pub signature: Option<String>,
    /// Optional parent context (e.g., `"impl MyStruct"` for methods).
    pub parent: Option<String>,
}

/// An import/dependency between files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// File that contains the import statement.
    pub from_file: String,
    /// Module or package being imported.
    pub to_module: String,
    /// Specific symbols imported (empty if importing the whole module).
    pub imported_symbols: Vec<String>,
    /// 1-based line number of the import statement.
    pub line: usize,
}

/// A function call reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRef {
    /// Qualified name of the calling function (e.g., `"file.rs::run"`).
    pub caller: String,
    /// Name of the function being called.
    pub callee: String,
    /// File where the call occurs.
    pub file: String,
    /// 1-based line number of the call site.
    pub line: usize,
}

/// Impact analysis result for a proposed change to a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    /// The symbol being analyzed.
    pub target_symbol: String,
    /// Symbols that directly call or reference the target.
    pub directly_affected: Vec<String>,
    /// Symbols that transitively depend on the target.
    pub transitively_affected: Vec<String>,
    /// Files that contain affected symbols.
    pub affected_files: Vec<String>,
    /// Test functions that could be affected.
    pub affected_tests: Vec<String>,
    /// Estimated risk level based on the breadth of impact.
    pub risk_level: RiskLevel,
}

/// Relevant code context gathered for a task description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeContext {
    /// Symbols relevant to the task.
    pub symbols: Vec<CodeSymbol>,
    /// Code snippets relevant to the task.
    pub snippets: Vec<CodeSnippet>,
    /// Dependencies relevant to the task.
    pub dependencies: Vec<Dependency>,
    /// Estimated total tokens for the collected context.
    pub total_tokens_estimate: usize,
}

/// A snippet of source code with relevance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSnippet {
    /// File the snippet comes from.
    pub file: String,
    /// 1-based start line.
    pub start_line: usize,
    /// 1-based end line (inclusive).
    pub end_line: usize,
    /// The source text of the snippet.
    pub content: String,
    /// How relevant this snippet is (0.0 to 1.0).
    pub relevance_score: f32,
}

/// Summary statistics for a [`CodeGraph`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraphSummary {
    /// Number of distinct files parsed.
    pub total_files: usize,
    /// Total number of symbols extracted.
    pub total_symbols: usize,
    /// Per-language file counts.
    pub languages: Vec<(Language, usize)>,
    /// Top-level module names.
    pub top_level_modules: Vec<String>,
    /// Number of public API symbols.
    pub public_api_count: usize,
}

// ---------------------------------------------------------------------------
// CodeGraph
// ---------------------------------------------------------------------------

/// The central code understanding structure.
///
/// Holds symbols, dependency edges, and call references extracted from source
/// files via lightweight regex-based parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraph {
    symbols: Vec<CodeSymbol>,
    dependencies: Vec<Dependency>,
    calls: Vec<CallRef>,
}

impl Default for CodeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeGraph {
    /// Create a new empty `CodeGraph`.
    pub fn new() -> Self {
        Self {
            symbols: Vec::new(),
            dependencies: Vec::new(),
            calls: Vec::new(),
        }
    }

    /// Detect language from a file path's extension.
    pub fn detect_language(path: &str) -> Language {
        let ext = path.rsplit('.').next().unwrap_or("");
        match ext {
            "rs" => Language::Rust,
            "py" => Language::Python,
            "ts" | "tsx" | "js" | "jsx" => Language::TypeScript,
            "go" => Language::Go,
            _ => Language::Unknown,
        }
    }

    /// Parse a single file and extract symbols, imports, and calls.
    ///
    /// Language is auto-detected from the file extension. If the language is
    /// not recognized, parsing is silently skipped.
    pub fn parse_file(&mut self, path: &str, content: &str) {
        match Self::detect_language(path) {
            Language::Rust => self.parse_rust(path, content),
            Language::Python => self.parse_python(path, content),
            Language::TypeScript => self.parse_typescript(path, content),
            Language::Go => self.parse_go(path, content),
            Language::Unknown => {}
        }
    }

    // -- Language-specific parsers ------------------------------------------

    /// Parse Rust source code.
    fn parse_rust(&mut self, path: &str, content: &str) {
        self.parse_rust_imports(path, content);
        self.parse_rust_items(path, content);
        self.parse_rust_calls(path, content);
    }

    /// Parse Python source code.
    fn parse_python(&mut self, path: &str, content: &str) {
        self.parse_python_imports(path, content);
        self.parse_python_items(path, content);
    }

    /// Parse TypeScript/JavaScript source code.
    fn parse_typescript(&mut self, path: &str, content: &str) {
        self.parse_typescript_imports(path, content);
        self.parse_typescript_items(path, content);
    }

    /// Parse Go source code.
    fn parse_go(&mut self, path: &str, content: &str) {
        self.parse_go_imports(path, content);
        self.parse_go_items(path, content);
    }

    // -- Public accessors ---------------------------------------------------

    /// Get all symbols in the graph.
    pub fn symbols(&self) -> &[CodeSymbol] {
        &self.symbols
    }

    /// Get all dependencies.
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// Get all call references.
    pub fn calls(&self) -> &[CallRef] {
        &self.calls
    }

    /// Find symbols whose name matches the given string exactly.
    pub fn find_symbol(&self, name: &str) -> Vec<&CodeSymbol> {
        self.symbols.iter().filter(|s| s.name == name).collect()
    }

    /// Find all symbols defined in a specific file.
    pub fn symbols_in_file(&self, file: &str) -> Vec<&CodeSymbol> {
        self.symbols.iter().filter(|s| s.file == file).collect()
    }

    /// Find all call references where the callee matches `symbol_name`.
    pub fn find_references(&self, symbol_name: &str) -> Vec<&CallRef> {
        self.calls
            .iter()
            .filter(|c| c.callee == symbol_name)
            .collect()
    }

    /// Find all import dependencies that bring in `symbol_name`.
    pub fn find_imports(&self, symbol_name: &str) -> Vec<&Dependency> {
        self.dependencies
            .iter()
            .filter(|d| {
                d.imported_symbols.iter().any(|s| s == symbol_name)
                    || d.to_module.ends_with(symbol_name)
            })
            .collect()
    }

    /// Perform impact analysis for changing a symbol.
    ///
    /// Determines which symbols, files, and tests are directly or transitively
    /// affected, and assigns a [`RiskLevel`].
    pub fn impact_analysis(&self, symbol_name: &str) -> ImpactAnalysis {
        // Direct callers / references
        let direct_refs = self.find_references(symbol_name);
        let directly_affected: Vec<String> = direct_refs.iter().map(|r| r.caller.clone()).collect();

        // Collect affected files from direct refs
        let mut affected_files_set: HashSet<String> = HashSet::new();
        for r in &direct_refs {
            affected_files_set.insert(r.file.clone());
        }

        // Also include files that import this symbol
        for dep in self.find_imports(symbol_name) {
            affected_files_set.insert(dep.from_file.clone());
        }

        // Transitive: find callers of the direct callers (one more level)
        let mut transitively_affected: Vec<String> = Vec::new();
        let mut transitive_seen: HashSet<String> = HashSet::new();
        for caller in &directly_affected {
            // Extract the function name from "file::func" format
            let func_name = caller.rsplit("::").next().unwrap_or(caller);
            for call in &self.calls {
                if call.callee == func_name
                    && !directly_affected.contains(&call.caller)
                    && transitive_seen.insert(call.caller.clone())
                {
                    transitively_affected.push(call.caller.clone());
                    affected_files_set.insert(call.file.clone());
                }
            }
        }

        // Identify affected tests
        let affected_tests: Vec<String> = self
            .symbols
            .iter()
            .filter(|s| s.name.starts_with("test_") && affected_files_set.contains(&s.file))
            .map(|s| s.name.clone())
            .collect();

        let affected_files: Vec<String> = affected_files_set.into_iter().collect();

        // Determine risk level
        let total_affected = directly_affected.len() + transitively_affected.len();
        let is_public = self
            .symbols
            .iter()
            .any(|s| s.name == symbol_name && s.visibility == Visibility::Public);

        let risk_level = if total_affected > 10 || (is_public && total_affected > 5) {
            RiskLevel::Critical
        } else if total_affected > 5 || (is_public && total_affected > 2) {
            RiskLevel::High
        } else if total_affected > 2 || is_public {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        };

        ImpactAnalysis {
            target_symbol: symbol_name.to_string(),
            directly_affected,
            transitively_affected,
            affected_files,
            affected_tests,
            risk_level,
        }
    }

    /// Build relevant context for a task description.
    ///
    /// Uses keyword matching against symbol names, signatures, and file paths
    /// to collect the most relevant symbols and snippets within a token budget.
    pub fn relevant_context(&self, task: &str, max_tokens: usize) -> CodeContext {
        let keywords: Vec<&str> = task.split_whitespace().collect();

        // Score each symbol by keyword overlap
        let mut scored: Vec<(&CodeSymbol, f32)> = self
            .symbols
            .iter()
            .filter_map(|sym| {
                let score = Self::relevance_score(sym, &keywords);
                if score > 0.0 {
                    Some((sym, score))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut context_symbols = Vec::new();
        let mut snippets = Vec::new();
        let mut context_deps = Vec::new();
        let mut tokens_used: usize = 0;

        for (sym, score) in &scored {
            // Rough estimate: name + signature ~ 10-30 tokens
            let sym_tokens = 10 + sym.signature.as_ref().map_or(0, |s| s.len() / 4);
            if tokens_used + sym_tokens > max_tokens {
                break;
            }
            tokens_used += sym_tokens;
            context_symbols.push((*sym).clone());

            // Add a snippet placeholder for the symbol's location
            if let Some(sig) = &sym.signature {
                let snippet_tokens = sig.len() / 4;
                if tokens_used + snippet_tokens <= max_tokens {
                    tokens_used += snippet_tokens;
                    snippets.push(CodeSnippet {
                        file: sym.file.clone(),
                        start_line: sym.line,
                        end_line: sym.line,
                        content: sig.clone(),
                        relevance_score: *score,
                    });
                }
            }
        }

        // Collect relevant dependencies
        for dep in &self.dependencies {
            let dep_relevant = keywords.iter().any(|kw| {
                let kw_lower = kw.to_lowercase();
                dep.to_module.to_lowercase().contains(&kw_lower)
                    || dep
                        .imported_symbols
                        .iter()
                        .any(|s| s.to_lowercase().contains(&kw_lower))
            });
            if dep_relevant {
                let dep_tokens = 5 + dep.imported_symbols.len() * 3;
                if tokens_used + dep_tokens <= max_tokens {
                    tokens_used += dep_tokens;
                    context_deps.push(dep.clone());
                }
            }
        }

        CodeContext {
            symbols: context_symbols,
            snippets,
            dependencies: context_deps,
            total_tokens_estimate: tokens_used,
        }
    }

    /// Get a summary of the codebase structure.
    pub fn summary(&self) -> CodeGraphSummary {
        let mut file_set: HashSet<String> = HashSet::new();
        let mut lang_counts: HashMap<Language, HashSet<String>> = HashMap::new();
        let mut modules: HashSet<String> = HashSet::new();
        let mut public_api_count: usize = 0;

        for sym in &self.symbols {
            file_set.insert(sym.file.clone());
            lang_counts
                .entry(sym.language)
                .or_default()
                .insert(sym.file.clone());

            if sym.kind == SymbolKind::Module {
                modules.insert(sym.name.clone());
            }
            if sym.visibility == Visibility::Public {
                public_api_count += 1;
            }
        }

        // Also count files from dependencies (imports in files without symbols)
        for dep in &self.dependencies {
            file_set.insert(dep.from_file.clone());
        }

        let mut languages: Vec<(Language, usize)> = lang_counts
            .into_iter()
            .map(|(lang, files)| (lang, files.len()))
            .collect();
        languages.sort_by_key(|entry| std::cmp::Reverse(entry.1));

        let mut top_level_modules: Vec<String> = modules.into_iter().collect();
        top_level_modules.sort();

        CodeGraphSummary {
            total_files: file_set.len(),
            total_symbols: self.symbols.len(),
            languages,
            top_level_modules,
            public_api_count,
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Compute a relevance score for a symbol against a set of keywords.
    fn relevance_score(sym: &CodeSymbol, keywords: &[&str]) -> f32 {
        let mut score: f32 = 0.0;
        let name_lower = sym.name.to_lowercase();
        let file_lower = sym.file.to_lowercase();
        let sig_lower = sym
            .signature
            .as_ref()
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        for kw in keywords {
            let kw_lower = kw.to_lowercase();
            if kw_lower.len() < 3 {
                continue; // skip short words like "a", "to", etc.
            }
            if name_lower.contains(&kw_lower) {
                score += 1.0;
            }
            if sig_lower.contains(&kw_lower) {
                score += 0.5;
            }
            if file_lower.contains(&kw_lower) {
                score += 0.3;
            }
        }

        // Bonus for public symbols
        if sym.visibility == Visibility::Public {
            score *= 1.2;
        }

        score
    }

    // -- Rust parsing -------------------------------------------------------

    /// Extract `use` statements from Rust source.
    fn parse_rust_imports(&mut self, path: &str, content: &str) {
        // Match: use path::to::module; or use path::to::{A, B};
        let re_use_simple = build_regex(r"^use\s+([\w:]+(?:::\*)?)\s*;");
        let re_use_braces = build_regex(r"^use\s+([\w:]+)::\{([^}]+)\}\s*;");

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if let Some(caps) = re_use_braces.captures(trimmed) {
                let module = caps.get(1).map_or("", |m| m.as_str());
                let symbols_str = caps.get(2).map_or("", |m| m.as_str());
                let imported: Vec<String> = symbols_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: imported,
                    line: line_num + 1,
                });
            } else if let Some(caps) = re_use_simple.captures(trimmed) {
                let full_path = caps.get(1).map_or("", |m| m.as_str());
                // The last segment is the imported symbol
                let last_segment = full_path
                    .rsplit("::")
                    .next()
                    .unwrap_or(full_path)
                    .to_string();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: full_path.to_string(),
                    imported_symbols: vec![last_segment],
                    line: line_num + 1,
                });
            }
        }
    }

    /// Extract items (functions, structs, enums, traits, impls, consts, type aliases)
    /// from Rust source.
    fn parse_rust_items(&mut self, path: &str, content: &str) {
        let re_fn = build_regex(
            r"^(\s*)(pub\s+)?(async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)(\s*->\s*[^{]+)?",
        );
        let re_struct = build_regex(r"^(\s*)(pub\s+)?struct\s+(\w+)");
        let re_enum = build_regex(r"^(\s*)(pub\s+)?enum\s+(\w+)");
        let re_trait = build_regex(r"^(\s*)(pub\s+)?trait\s+(\w+)");
        let re_impl = build_regex(r"^\s*impl\s+(?:<[^>]*>\s+)?(\w+)(?:\s+for\s+(\w+))?");
        let re_const = build_regex(r"^(\s*)(pub\s+)?const\s+(\w+)\s*:");
        let re_type = build_regex(r"^(\s*)(pub\s+)?type\s+(\w+)");
        let re_mod = build_regex(r"^(\s*)(pub\s+)?mod\s+(\w+)");

        let mut current_impl: Option<String> = None;

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            // Track impl blocks
            if let Some(caps) = re_impl.captures(trimmed) {
                let trait_or_type = caps.get(1).map_or("", |m| m.as_str());
                if let Some(for_type) = caps.get(2) {
                    current_impl = Some(format!("impl {trait_or_type} for {}", for_type.as_str()));
                } else {
                    current_impl = Some(format!("impl {trait_or_type}"));
                }
                continue;
            }

            // Reset impl tracking on closing brace at column 0
            if trimmed == "}" && !line.starts_with(' ') && !line.starts_with('\t') {
                current_impl = None;
                continue;
            }

            // Functions / methods
            if let Some(caps) = re_fn.captures(line) {
                let is_pub = caps.get(2).is_some();
                let is_async = caps.get(3).is_some();
                let name = caps.get(4).map_or("", |m| m.as_str());
                let params = caps.get(5).map_or("", |m| m.as_str());
                let ret = caps.get(6).map_or("", |m| m.as_str()).trim();

                let kind = if current_impl.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };

                let async_kw = if is_async { "async " } else { "" };
                let pub_kw = if is_pub { "pub " } else { "" };
                let ret_str = if ret.is_empty() {
                    String::new()
                } else {
                    format!(" {ret}")
                };
                let signature = format!("{pub_kw}{async_kw}fn {name}({params}){ret_str}");

                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: Some(signature),
                    parent: current_impl.clone(),
                });
                continue;
            }

            // Structs
            if let Some(caps) = re_struct.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Struct,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: Some(format!("{}struct {name}", if is_pub { "pub " } else { "" })),
                    parent: None,
                });
                continue;
            }

            // Enums
            if let Some(caps) = re_enum.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Enum,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: Some(format!("{}enum {name}", if is_pub { "pub " } else { "" })),
                    parent: None,
                });
                continue;
            }

            // Traits
            if let Some(caps) = re_trait.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Trait,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: Some(format!("{}trait {name}", if is_pub { "pub " } else { "" })),
                    parent: None,
                });
                continue;
            }

            // Constants
            if let Some(caps) = re_const.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: None,
                    parent: current_impl.clone(),
                });
                continue;
            }

            // Type aliases
            if let Some(caps) = re_type.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: None,
                    parent: None,
                });
                continue;
            }

            // Modules
            if let Some(caps) = re_mod.captures(line) {
                let is_pub = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Module,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Rust,
                    signature: None,
                    parent: None,
                });
            }
        }
    }

    /// Extract function call references from Rust source.
    fn parse_rust_calls(&mut self, path: &str, content: &str) {
        let re_call = build_regex(r"(\w+)\s*\(");
        let re_fn_decl = build_regex(r"^\s*(pub\s+)?(async\s+)?fn\s+(\w+)");

        // Rust keywords and macros that look like calls but aren't
        let skip_keywords: HashSet<&str> = [
            "if",
            "for",
            "while",
            "match",
            "return",
            "fn",
            "let",
            "mut",
            "pub",
            "use",
            "struct",
            "enum",
            "trait",
            "impl",
            "type",
            "const",
            "mod",
            "where",
            "Some",
            "None",
            "Ok",
            "Err",
            "vec",
            "println",
            "eprintln",
            "format",
            "write",
            "writeln",
            "assert",
            "assert_eq",
            "assert_ne",
            "debug_assert",
            "debug_assert_eq",
            "cfg",
            "derive",
            "allow",
            "warn",
            "deny",
        ]
        .into_iter()
        .collect();

        let mut current_fn: Option<String> = None;

        for (line_num, line) in content.lines().enumerate() {
            // Track current function
            if let Some(caps) = re_fn_decl.captures(line) {
                let fname = caps.get(3).map_or("", |m| m.as_str());
                current_fn = Some(format!("{path}::{fname}"));
            }

            // Only record calls inside functions
            let caller = match &current_fn {
                Some(c) => c.clone(),
                None => continue,
            };

            for caps in re_call.captures_iter(line) {
                let callee = caps.get(1).map_or("", |m| m.as_str());
                if skip_keywords.contains(callee) {
                    continue;
                }
                // Skip if this line IS the function declaration itself
                if re_fn_decl.is_match(line) {
                    continue;
                }
                self.calls.push(CallRef {
                    caller: caller.clone(),
                    callee: callee.to_string(),
                    file: path.to_string(),
                    line: line_num + 1,
                });
            }
        }
    }

    // -- Python parsing -----------------------------------------------------

    /// Extract Python imports.
    fn parse_python_imports(&mut self, path: &str, content: &str) {
        let re_from_import = build_regex(r"^from\s+([\w.]+)\s+import\s+(.+)");
        let re_import = build_regex(r"^import\s+([\w.]+)");

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if let Some(caps) = re_from_import.captures(trimmed) {
                let module = caps.get(1).map_or("", |m| m.as_str());
                let symbols_str = caps.get(2).map_or("", |m| m.as_str());
                let imported: Vec<String> = symbols_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: imported,
                    line: line_num + 1,
                });
            } else if let Some(caps) = re_import.captures(trimmed) {
                let module = caps.get(1).map_or("", |m| m.as_str());
                let last = module.rsplit('.').next().unwrap_or(module).to_string();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: vec![last],
                    line: line_num + 1,
                });
            }
        }
    }

    /// Extract Python classes and functions.
    fn parse_python_items(&mut self, path: &str, content: &str) {
        let re_class = build_regex(r"^class\s+(\w+)");
        let re_def = build_regex(r"^(\s*)(async\s+)?def\s+(\w+)\s*\(([^)]*)\)");

        let mut current_class: Option<String> = None;

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            // Classes
            if let Some(caps) = re_class.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                current_class = Some(name.to_string());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if name.starts_with('_') {
                        if name.starts_with("__") && !name.ends_with("__") {
                            Visibility::Protected
                        } else {
                            Visibility::Private
                        }
                    } else {
                        Visibility::Public
                    },
                    language: Language::Python,
                    signature: Some(format!("class {name}")),
                    parent: None,
                });
                continue;
            }

            // Functions / methods
            if let Some(caps) = re_def.captures(line) {
                let indent = caps.get(1).map_or("", |m| m.as_str());
                let is_async = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                let params = caps.get(4).map_or("", |m| m.as_str());

                // If indented and we have a current class, it's a method
                let is_method = !indent.is_empty() && current_class.is_some();
                let kind = if is_method {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };

                // Reset class tracking if we hit a non-indented def
                if indent.is_empty() {
                    current_class = None;
                }

                let visibility = if name.starts_with("__") && !name.ends_with("__") {
                    Visibility::Protected
                } else if name.starts_with('_') {
                    Visibility::Private
                } else {
                    Visibility::Public
                };

                let async_kw = if is_async { "async " } else { "" };
                let signature = format!("{async_kw}def {name}({params})");

                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility,
                    language: Language::Python,
                    signature: Some(signature),
                    parent: if is_method {
                        current_class.clone()
                    } else {
                        None
                    },
                });
            }
        }
    }

    // -- TypeScript parsing -------------------------------------------------

    /// Extract TypeScript/JavaScript imports.
    fn parse_typescript_imports(&mut self, path: &str, content: &str) {
        let re_import_named = build_regex(r#"import\s+\{([^}]+)\}\s+from\s+['"]([^'"]+)['"]"#);
        let re_import_default = build_regex(r#"import\s+(\w+)\s+from\s+['"]([^'"]+)['"]"#);

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if let Some(caps) = re_import_named.captures(trimmed) {
                let symbols_str = caps.get(1).map_or("", |m| m.as_str());
                let module = caps.get(2).map_or("", |m| m.as_str());
                let imported: Vec<String> = symbols_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: imported,
                    line: line_num + 1,
                });
            } else if let Some(caps) = re_import_default.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let module = caps.get(2).map_or("", |m| m.as_str());
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: vec![name.to_string()],
                    line: line_num + 1,
                });
            }
        }
    }

    /// Extract TypeScript/JavaScript classes, functions, interfaces, etc.
    fn parse_typescript_items(&mut self, path: &str, content: &str) {
        let re_class = build_regex(r"^(\s*)(export\s+)?(abstract\s+)?class\s+(\w+)");
        let re_interface = build_regex(r"^(\s*)(export\s+)?interface\s+(\w+)");
        let re_function =
            build_regex(r"^(\s*)(export\s+)?(async\s+)?function\s+(\w+)\s*\(([^)]*)\)");
        let re_const_fn = build_regex(r"^(\s*)(export\s+)?const\s+(\w+)\s*=\s*(?:async\s+)?\(");
        let re_export_const = build_regex(r"^(\s*)(export\s+)const\s+(\w+)\s*[=:]");
        let re_export_type = build_regex(r"^(\s*)(export\s+)?type\s+(\w+)");

        for (line_num, line) in content.lines().enumerate() {
            // Classes
            if let Some(caps) = re_class.captures(line) {
                let is_export = caps.get(2).is_some();
                let name = caps.get(4).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Class,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_export {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::TypeScript,
                    signature: Some(format!(
                        "{}class {name}",
                        if is_export { "export " } else { "" }
                    )),
                    parent: None,
                });
                continue;
            }

            // Interfaces
            if let Some(caps) = re_interface.captures(line) {
                let is_export = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_export {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::TypeScript,
                    signature: Some(format!(
                        "{}interface {name}",
                        if is_export { "export " } else { "" }
                    )),
                    parent: None,
                });
                continue;
            }

            // Named functions
            if let Some(caps) = re_function.captures(line) {
                let is_export = caps.get(2).is_some();
                let is_async = caps.get(3).is_some();
                let name = caps.get(4).map_or("", |m| m.as_str());
                let params = caps.get(5).map_or("", |m| m.as_str());

                let export_kw = if is_export { "export " } else { "" };
                let async_kw = if is_async { "async " } else { "" };
                let signature = format!("{export_kw}{async_kw}function {name}({params})");

                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_export {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::TypeScript,
                    signature: Some(signature),
                    parent: None,
                });
                continue;
            }

            // Arrow functions: const X = ( or const X = async (
            if let Some(caps) = re_const_fn.captures(line) {
                let is_export = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_export {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::TypeScript,
                    signature: Some(format!(
                        "{}const {name} = (...)",
                        if is_export { "export " } else { "" }
                    )),
                    parent: None,
                });
                continue;
            }

            // Type aliases
            if let Some(caps) = re_export_type.captures(line) {
                let is_export = caps.get(2).is_some();
                let name = caps.get(3).map_or("", |m| m.as_str());
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::TypeAlias,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_export {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::TypeScript,
                    signature: None,
                    parent: None,
                });
                continue;
            }

            // Exported constants (but not arrow functions, already handled)
            if let Some(caps) = re_export_const.captures(line) {
                let name = caps.get(3).map_or("", |m| m.as_str());
                // Make sure it's not a const arrow function already captured
                if !re_const_fn.is_match(line) {
                    self.symbols.push(CodeSymbol {
                        name: name.to_string(),
                        kind: SymbolKind::Constant,
                        file: path.to_string(),
                        line: line_num + 1,
                        visibility: Visibility::Public,
                        language: Language::TypeScript,
                        signature: None,
                        parent: None,
                    });
                }
            }
        }
    }

    // -- Go parsing ---------------------------------------------------------

    /// Extract Go imports.
    fn parse_go_imports(&mut self, path: &str, content: &str) {
        // Single import: import "fmt"
        let re_single = build_regex(r#"^import\s+"([^"]+)""#);
        // Block import start
        let re_block_start = build_regex(r"^import\s*\(");
        let re_block_item = build_regex(r#"^\s*(?:(\w+)\s+)?"([^"]+)""#);

        let mut in_block = false;

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            if in_block {
                if trimmed == ")" {
                    in_block = false;
                    continue;
                }
                if let Some(caps) = re_block_item.captures(trimmed) {
                    let module = caps.get(2).map_or("", |m| m.as_str());
                    let last = module.rsplit('/').next().unwrap_or(module).to_string();
                    self.dependencies.push(Dependency {
                        from_file: path.to_string(),
                        to_module: module.to_string(),
                        imported_symbols: vec![last],
                        line: line_num + 1,
                    });
                }
                continue;
            }

            if re_block_start.is_match(trimmed) {
                in_block = true;
                continue;
            }

            if let Some(caps) = re_single.captures(trimmed) {
                let module = caps.get(1).map_or("", |m| m.as_str());
                let last = module.rsplit('/').next().unwrap_or(module).to_string();
                self.dependencies.push(Dependency {
                    from_file: path.to_string(),
                    to_module: module.to_string(),
                    imported_symbols: vec![last],
                    line: line_num + 1,
                });
            }
        }
    }

    /// Extract Go types, functions, and constants.
    fn parse_go_items(&mut self, path: &str, content: &str) {
        // func Name( or func (receiver) Name(
        let re_func = build_regex(r"^func\s+(\(\w+\s+\*?\w+\)\s+)?(\w+)\s*\(([^)]*)\)");
        let re_struct = build_regex(r"^type\s+(\w+)\s+struct\b");
        let re_interface = build_regex(r"^type\s+(\w+)\s+interface\b");
        let re_const = build_regex(r"^(?:const|var)\s+(\w+)\s");

        for (line_num, line) in content.lines().enumerate() {
            let trimmed = line.trim();

            // Functions / methods
            if let Some(caps) = re_func.captures(trimmed) {
                let receiver = caps.get(1).map(|m| m.as_str().trim().to_string());
                let name = caps.get(2).map_or("", |m| m.as_str());
                let params = caps.get(3).map_or("", |m| m.as_str());

                let is_exported = name.chars().next().is_some_and(char::is_uppercase);

                let kind = if receiver.is_some() {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };

                let sig = if let Some(ref recv) = receiver {
                    format!("func {recv} {name}({params})")
                } else {
                    format!("func {name}({params})")
                };

                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_exported {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Go,
                    signature: Some(sig),
                    parent: receiver,
                });
                continue;
            }

            // Structs
            if let Some(caps) = re_struct.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let is_exported = name.chars().next().is_some_and(char::is_uppercase);
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Struct,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_exported {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Go,
                    signature: Some(format!("type {name} struct")),
                    parent: None,
                });
                continue;
            }

            // Interfaces
            if let Some(caps) = re_interface.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let is_exported = name.chars().next().is_some_and(char::is_uppercase);
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Interface,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_exported {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Go,
                    signature: Some(format!("type {name} interface")),
                    parent: None,
                });
                continue;
            }

            // Constants / variables
            if let Some(caps) = re_const.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let is_exported = name.chars().next().is_some_and(char::is_uppercase);
                self.symbols.push(CodeSymbol {
                    name: name.to_string(),
                    kind: SymbolKind::Constant,
                    file: path.to_string(),
                    line: line_num + 1,
                    visibility: if is_exported {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    },
                    language: Language::Go,
                    signature: None,
                    parent: None,
                });
            }
        }
    }
}

/// Build a compiled [`Regex`] from a pattern string.
///
/// Returns a `Regex` or panics at startup if the pattern is invalid.
/// All patterns in this module are compile-time constants, so this
/// is effectively a static check.
fn build_regex(pattern: &str) -> Regex {
    // All patterns in this module are hardcoded string literals validated
    // at development time. A malformed literal would be caught by tests
    // during CI. We match on the result to avoid `.unwrap()`.
    match Regex::new(pattern) {
        Ok(re) => re,
        Err(e) => {
            tracing::error!("BUG: invalid regex pattern '{pattern}': {e}");
            // Degrade gracefully: `a]b` is a valid regex that will never
            // match normal source code, so parsing simply extracts nothing.
            // The `$^` pattern (end-of-string then start-of-string) matches
            // nothing in single-line mode.
            match Regex::new(r"$^") {
                Ok(fallback) => fallback,
                // `$^` is guaranteed valid, but if somehow it fails we
                // have no choice but to panic — this is a logic error.
                Err(_) => unreachable!("fallback regex r\"$^\" must compile"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -- Language detection --------------------------------------------------

    #[test]
    fn test_detect_language_rust() {
        assert_eq!(CodeGraph::detect_language("src/main.rs"), Language::Rust);
        assert_eq!(CodeGraph::detect_language("lib.rs"), Language::Rust);
    }

    #[test]
    fn test_detect_language_python() {
        assert_eq!(CodeGraph::detect_language("app.py"), Language::Python);
        assert_eq!(
            CodeGraph::detect_language("scripts/deploy.py"),
            Language::Python
        );
    }

    #[test]
    fn test_detect_language_typescript() {
        assert_eq!(CodeGraph::detect_language("index.ts"), Language::TypeScript);
        assert_eq!(CodeGraph::detect_language("App.tsx"), Language::TypeScript);
        assert_eq!(CodeGraph::detect_language("util.js"), Language::TypeScript);
        assert_eq!(
            CodeGraph::detect_language("Component.jsx"),
            Language::TypeScript
        );
    }

    #[test]
    fn test_detect_language_go() {
        assert_eq!(CodeGraph::detect_language("main.go"), Language::Go);
        assert_eq!(
            CodeGraph::detect_language("pkg/server/handler.go"),
            Language::Go
        );
    }

    #[test]
    fn test_detect_language_unknown() {
        assert_eq!(CodeGraph::detect_language("data.csv"), Language::Unknown);
        assert_eq!(CodeGraph::detect_language("Dockerfile"), Language::Unknown);
        assert_eq!(CodeGraph::detect_language("notes.md"), Language::Unknown);
    }

    // -- Rust parsing -------------------------------------------------------

    fn sample_rust_code() -> &'static str {
        r#"
use std::collections::HashMap;
use crate::config::ModelConfig;

pub struct AgentRunner {
    config: ModelConfig,
    tools: Vec<String>,
}

impl AgentRunner {
    pub fn new(config: ModelConfig) -> Self {
        Self { config, tools: Vec::new() }
    }

    pub async fn run(&mut self, prompt: &str) -> Result<String, Error> {
        self.process(prompt)
    }

    fn process(&self, input: &str) -> Result<String, Error> {
        Ok(input.to_string())
    }
}

pub trait Backend {
    fn chat(&self, messages: &[Message]) -> Result<Response, Error>;
}

pub enum LlmProvider {
    OpenAi,
    Claude,
    Gemini,
}

const MAX_RETRIES: usize = 3;

pub type Result<T> = std::result::Result<T, Error>;
"#
    }

    #[test]
    fn test_parse_rust_functions() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let fns: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Method || s.kind == SymbolKind::Function)
            .collect();

        // We expect: new, run, process (methods in impl), chat (in trait)
        let names: Vec<&str> = fns.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"new"), "should find 'new' method");
        assert!(names.contains(&"run"), "should find 'run' method");
        assert!(names.contains(&"process"), "should find 'process' method");
        assert!(names.contains(&"chat"), "should find 'chat' method");

        // Check visibility
        let new_sym = fns.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new_sym.visibility, Visibility::Public);

        let process_sym = fns.iter().find(|s| s.name == "process").unwrap();
        assert_eq!(process_sym.visibility, Visibility::Private);

        // Check async signature
        let run_sym = fns.iter().find(|s| s.name == "run").unwrap();
        assert!(
            run_sym.signature.as_ref().unwrap().contains("async"),
            "run should have async in signature"
        );
    }

    #[test]
    fn test_parse_rust_structs_and_enums() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let structs: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Struct)
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "AgentRunner");
        assert_eq!(structs[0].visibility, Visibility::Public);

        let enums: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Enum)
            .collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "LlmProvider");
        assert_eq!(enums[0].visibility, Visibility::Public);
    }

    #[test]
    fn test_parse_rust_traits_and_impls() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let traits: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Trait)
            .collect();
        assert_eq!(traits.len(), 1);
        assert_eq!(traits[0].name, "Backend");
        assert_eq!(traits[0].visibility, Visibility::Public);

        // Methods inside impl should have parent set
        let new_sym = graph
            .symbols()
            .iter()
            .find(|s| s.name == "new" && s.kind == SymbolKind::Method)
            .unwrap();
        assert!(new_sym.parent.is_some());
        assert!(
            new_sym.parent.as_ref().unwrap().contains("AgentRunner"),
            "new's parent should reference AgentRunner"
        );
    }

    #[test]
    fn test_parse_rust_imports() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let deps = graph.dependencies();
        assert!(deps.len() >= 2, "should have at least 2 imports");

        let hashmap_dep = deps
            .iter()
            .find(|d| d.to_module.contains("HashMap"))
            .expect("should find HashMap import");
        assert!(hashmap_dep
            .imported_symbols
            .contains(&"HashMap".to_string()));

        let config_dep = deps
            .iter()
            .find(|d| d.to_module.contains("config"))
            .expect("should find config import");
        assert!(config_dep
            .imported_symbols
            .contains(&"ModelConfig".to_string()));
    }

    // -- Python parsing -----------------------------------------------------

    fn sample_python_code() -> &'static str {
        r#"
from typing import List, Optional
import os
from dataclasses import dataclass

class AgentConfig:
    """Configuration for an agent."""

    def __init__(self, name: str, model: str):
        self.name = name
        self.model = model

    def validate(self) -> bool:
        return bool(self.name)

    def _internal_check(self):
        pass

    def __secret_method(self):
        pass

class Runner:
    def __init__(self, config: AgentConfig):
        self.config = config

    async def run(self, prompt: str) -> str:
        return await self.process(prompt)

    def process(self, text: str) -> str:
        return text

def setup_logging(level: str = "INFO"):
    pass

async def main():
    runner = Runner(AgentConfig("test", "gpt-4"))
    await runner.run("hello")
"#
    }

    #[test]
    fn test_parse_python_classes_and_functions() {
        let mut graph = CodeGraph::new();
        graph.parse_file("agent.py", sample_python_code());

        let classes: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Class)
            .collect();
        let class_names: Vec<&str> = classes.iter().map(|c| c.name.as_str()).collect();
        assert!(class_names.contains(&"AgentConfig"));
        assert!(class_names.contains(&"Runner"));

        let functions: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        let fn_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
        assert!(fn_names.contains(&"setup_logging"));
        assert!(fn_names.contains(&"main"));

        let methods: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .collect();
        let method_names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
        assert!(method_names.contains(&"__init__"));
        assert!(method_names.contains(&"validate"));
        assert!(method_names.contains(&"run"));
        assert!(method_names.contains(&"process"));
    }

    #[test]
    fn test_parse_python_imports() {
        let mut graph = CodeGraph::new();
        graph.parse_file("agent.py", sample_python_code());

        let deps = graph.dependencies();
        assert!(deps.len() >= 3, "should have at least 3 imports");

        let typing_dep = deps.iter().find(|d| d.to_module == "typing").unwrap();
        assert!(typing_dep.imported_symbols.contains(&"List".to_string()));
        assert!(typing_dep
            .imported_symbols
            .contains(&"Optional".to_string()));

        let os_dep = deps.iter().find(|d| d.to_module == "os").unwrap();
        assert!(os_dep.imported_symbols.contains(&"os".to_string()));
    }

    // -- TypeScript parsing -------------------------------------------------

    fn sample_typescript_code() -> &'static str {
        r#"
import { Router, Request, Response } from 'express';
import axios from 'axios';

export interface AgentConfig {
    name: string;
    model: string;
    temperature?: number;
}

export class Agent {
    private config: AgentConfig;

    constructor(config: AgentConfig) {
        this.config = config;
    }

    async run(prompt: string): Promise<string> {
        return this.process(prompt);
    }
}

export async function createAgent(name: string): Promise<Agent> {
    const config: AgentConfig = { name, model: 'gpt-4' };
    return new Agent(config);
}

export const defaultConfig: AgentConfig = {
    name: 'default',
    model: 'gpt-4',
};

const helper = (x: number) => x * 2;

export type AgentId = string;
"#
    }

    #[test]
    fn test_parse_typescript_classes_and_functions() {
        let mut graph = CodeGraph::new();
        graph.parse_file("agent.ts", sample_typescript_code());

        let classes: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Class)
            .collect();
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].name, "Agent");
        assert_eq!(classes[0].visibility, Visibility::Public);

        let interfaces: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Interface)
            .collect();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "AgentConfig");

        let functions: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        let fn_names: Vec<&str> = functions.iter().map(|f| f.name.as_str()).collect();
        assert!(fn_names.contains(&"createAgent"));
        assert!(fn_names.contains(&"helper"));

        // createAgent should be public, helper should be private
        let create = functions.iter().find(|f| f.name == "createAgent").unwrap();
        assert_eq!(create.visibility, Visibility::Public);
        let helper = functions.iter().find(|f| f.name == "helper").unwrap();
        assert_eq!(helper.visibility, Visibility::Private);
    }

    #[test]
    fn test_parse_typescript_imports() {
        let mut graph = CodeGraph::new();
        graph.parse_file("agent.ts", sample_typescript_code());

        let deps = graph.dependencies();
        assert!(deps.len() >= 2, "should have at least 2 imports");

        let express_dep = deps.iter().find(|d| d.to_module == "express").unwrap();
        assert!(express_dep.imported_symbols.contains(&"Router".to_string()));
        assert!(express_dep
            .imported_symbols
            .contains(&"Request".to_string()));

        let axios_dep = deps.iter().find(|d| d.to_module == "axios").unwrap();
        assert!(axios_dep.imported_symbols.contains(&"axios".to_string()));
    }

    // -- Go parsing ---------------------------------------------------------

    fn sample_go_code() -> &'static str {
        r#"
package main

import (
	"fmt"
	"net/http"
)

type Server struct {
	Port int
	Host string
}

type Handler interface {
	ServeHTTP(w http.ResponseWriter, r *http.Request)
}

func NewServer(port int) *Server {
	return &Server{Port: port, Host: "localhost"}
}

func (s *Server) Start() error {
	addr := fmt.Sprintf("%s:%d", s.Host, s.Port)
	return http.ListenAndServe(addr, nil)
}

func (s *Server) stop() {
	fmt.Println("stopping")
}

const MaxConnections = 100
var defaultTimeout = 30
"#
    }

    #[test]
    fn test_parse_go_functions_and_structs() {
        let mut graph = CodeGraph::new();
        graph.parse_file("server.go", sample_go_code());

        let structs: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Struct)
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Server");
        assert_eq!(structs[0].visibility, Visibility::Public);

        let interfaces: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Interface)
            .collect();
        assert_eq!(interfaces.len(), 1);
        assert_eq!(interfaces[0].name, "Handler");

        let functions: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .collect();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "NewServer");
        assert_eq!(functions[0].visibility, Visibility::Public);

        let methods: Vec<&CodeSymbol> = graph
            .symbols()
            .iter()
            .filter(|s| s.kind == SymbolKind::Method)
            .collect();
        let method_names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
        assert!(method_names.contains(&"Start"));
        assert!(method_names.contains(&"stop"));

        // Start is exported, stop is not
        let start = methods.iter().find(|m| m.name == "Start").unwrap();
        assert_eq!(start.visibility, Visibility::Public);
        let stop = methods.iter().find(|m| m.name == "stop").unwrap();
        assert_eq!(stop.visibility, Visibility::Private);
    }

    #[test]
    fn test_parse_go_imports() {
        let mut graph = CodeGraph::new();
        graph.parse_file("server.go", sample_go_code());

        let deps = graph.dependencies();
        assert!(deps.len() >= 2, "should have at least 2 imports");

        let fmt_dep = deps.iter().find(|d| d.to_module == "fmt").unwrap();
        assert!(fmt_dep.imported_symbols.contains(&"fmt".to_string()));

        let http_dep = deps.iter().find(|d| d.to_module == "net/http").unwrap();
        assert!(http_dep.imported_symbols.contains(&"http".to_string()));
    }

    // -- Graph queries ------------------------------------------------------

    #[test]
    fn test_find_symbol() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let results = graph.find_symbol("AgentRunner");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Struct);

        let results = graph.find_symbol("new");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SymbolKind::Method);

        let results = graph.find_symbol("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_references() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        // The `process` function is called inside `run`
        let refs = graph.find_references("process");
        assert!(
            !refs.is_empty(),
            "process should be referenced (called from run)"
        );
        // The caller should be run
        assert!(refs.iter().any(|r| r.caller.contains("run")));
    }

    #[test]
    fn test_symbols_in_file() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());
        graph.parse_file("agent.py", sample_python_code());

        let rust_syms = graph.symbols_in_file("src/runner.rs");
        assert!(!rust_syms.is_empty(), "should have symbols from runner.rs");
        assert!(rust_syms.iter().all(|s| s.file == "src/runner.rs"));

        let py_syms = graph.symbols_in_file("agent.py");
        assert!(!py_syms.is_empty(), "should have symbols from agent.py");
        assert!(py_syms.iter().all(|s| s.file == "agent.py"));

        let empty = graph.symbols_in_file("nonexistent.rs");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_impact_analysis() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let impact = graph.impact_analysis("process");

        // process is called by run, so run should be directly affected
        assert!(
            impact.directly_affected.iter().any(|a| a.contains("run")),
            "run should be directly affected by changes to process"
        );
        assert_eq!(impact.target_symbol, "process");

        // A public symbol with no callers still has medium risk (public API change)
        let impact_enum = graph.impact_analysis("LlmProvider");
        assert_eq!(impact_enum.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn test_relevant_context() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());

        let ctx = graph.relevant_context("AgentRunner config", 500);
        assert!(
            !ctx.symbols.is_empty(),
            "should find relevant symbols for 'AgentRunner config'"
        );
        // AgentRunner should be among the results
        assert!(
            ctx.symbols.iter().any(|s| s.name == "AgentRunner"),
            "AgentRunner should be in relevant context"
        );

        // With zero budget, nothing should be returned
        let ctx_empty = graph.relevant_context("AgentRunner", 0);
        assert!(ctx_empty.symbols.is_empty());
    }

    #[test]
    fn test_code_graph_summary() {
        let mut graph = CodeGraph::new();
        graph.parse_file("src/runner.rs", sample_rust_code());
        graph.parse_file("agent.py", sample_python_code());

        let summary = graph.summary();
        assert_eq!(summary.total_files, 2);
        assert!(summary.total_symbols > 0);
        assert!(summary.languages.len() >= 2);
        assert!(summary.public_api_count > 0);
    }

    #[test]
    fn test_parse_visibility() {
        let mut graph = CodeGraph::new();
        graph.parse_file("agent.py", sample_python_code());

        // _internal_check should be private
        let internal = graph
            .symbols()
            .iter()
            .find(|s| s.name == "_internal_check")
            .unwrap();
        assert_eq!(internal.visibility, Visibility::Private);

        // __secret_method should be protected (name-mangled)
        let secret = graph
            .symbols()
            .iter()
            .find(|s| s.name == "__secret_method")
            .unwrap();
        assert_eq!(secret.visibility, Visibility::Protected);

        // validate should be public
        let validate = graph
            .symbols()
            .iter()
            .find(|s| s.name == "validate")
            .unwrap();
        assert_eq!(validate.visibility, Visibility::Public);
    }

    #[test]
    fn test_empty_graph() {
        let graph = CodeGraph::new();

        assert!(graph.symbols().is_empty());
        assert!(graph.dependencies().is_empty());
        assert!(graph.calls().is_empty());
        assert!(graph.find_symbol("anything").is_empty());
        assert!(graph.find_references("anything").is_empty());
        assert!(graph.symbols_in_file("file.rs").is_empty());

        let impact = graph.impact_analysis("nothing");
        assert!(impact.directly_affected.is_empty());
        assert_eq!(impact.risk_level, RiskLevel::Low);

        let ctx = graph.relevant_context("task", 1000);
        assert!(ctx.symbols.is_empty());
        assert_eq!(ctx.total_tokens_estimate, 0);

        let summary = graph.summary();
        assert_eq!(summary.total_files, 0);
        assert_eq!(summary.total_symbols, 0);
    }
}
