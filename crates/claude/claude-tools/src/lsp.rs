//! Simplified LSP client for code intelligence.
//!
//! Provides text-based analysis as a fallback when no real LSP server is
//! available. Supports:
//! - Symbol definition search (regex-based)
//! - Symbol reference search
//! - Hover information (context around symbol usage)
//! - Diagnostics (runs linters like `cargo check`, `tsc --noEmit`, etc.)
//! - Completion suggestions (text-based heuristics)
//! - Document symbols, workspace symbols
//! - Go-to-implementation search
//! - Call hierarchy (incoming/outgoing calls)

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::IGNORED_DIRS;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A location in source code.
#[derive(Debug, Clone)]
pub struct Location {
    /// File path relative to the workspace root.
    pub file: String,
    /// 1-based line number.
    pub line: u32,
    /// 1-based column number (best-effort).
    pub column: u32,
    /// The source line content at this location.
    pub context: String,
}

/// A diagnostic message from a linter or compiler.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity level: "error", "warning", or "info".
    pub severity: String,
    /// The diagnostic message.
    pub message: String,
    /// 1-based line number where the issue was found.
    pub line: u32,
    /// File path relative to the workspace root.
    pub file: String,
}

// ---------------------------------------------------------------------------
// LSP client
// ---------------------------------------------------------------------------

/// Simplified LSP client that uses text-based analysis.
///
/// Gracefully degrades when no LSP server is available, providing
/// best-effort code intelligence through regex search and linter invocation.
pub struct LspClient {
    /// Workspace root directory.
    workspace_root: PathBuf,
}

impl LspClient {
    /// Create a new LSP client for the given workspace.
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
        }
    }

    /// Search for symbol definitions using pattern matching.
    ///
    /// Looks for common definition patterns like `fn name(`, `struct Name`,
    /// `enum Name`, `impl Name`, `trait Name`, `type Name`, `const NAME`.
    pub fn find_definitions(&self, symbol: &str, file_path: Option<&str>) -> Result<Vec<Location>> {
        let escaped = regex::escape(symbol);
        let patterns = [
            format!(r"(?:pub\s+)?(?:async\s+)?fn\s+{escaped}\s*[\(<]"),
            format!(r"(?:pub\s+)?struct\s+{escaped}\b"),
            format!(r"(?:pub\s+)?enum\s+{escaped}\b"),
            format!(r"impl\s+(?:<[^>]*>\s+)?{escaped}\b"),
            format!(r"(?:pub\s+)?trait\s+{escaped}\b"),
            format!(r"(?:pub\s+)?type\s+{escaped}\s*="),
            format!(r"(?:pub\s+)?const\s+{escaped}\s*:"),
            format!(r"(?:pub\s+)?static\s+{escaped}\s*:"),
            format!(r"(?:pub\s+)?mod\s+{escaped}\b"),
        ];

        let combined = patterns.join("|");
        let re = regex::Regex::new(&format!("(?:{combined})"))
            .context("invalid symbol pattern for definitions")?;

        let search_root = match file_path {
            Some(fp) => {
                let p = self.workspace_root.join(fp);
                if p.is_dir() {
                    p
                } else if let Some(parent) = p.parent() {
                    parent.to_path_buf()
                } else {
                    self.workspace_root.clone()
                }
            }
            None => self.workspace_root.clone(),
        };

        let mut results = Vec::new();
        for entry in WalkDir::new(&search_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (idx, line) in contents.lines().enumerate() {
                if re.is_match(line) {
                    let relative = entry
                        .path()
                        .strip_prefix(&self.workspace_root)
                        .unwrap_or(entry.path());
                    results.push(Location {
                        file: relative.display().to_string(),
                        line: (idx + 1) as u32,
                        column: find_column(line, symbol) as u32,
                        context: line.trim().to_owned(),
                    });
                    if results.len() >= 50 {
                        return Ok(results);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Search for all references to a symbol.
    pub fn find_references(&self, symbol: &str) -> Result<Vec<Location>> {
        let escaped = regex::escape(symbol);
        let pattern = format!(r"\b{escaped}\b");
        let re = regex::Regex::new(&pattern).context("invalid symbol pattern for references")?;

        let mut results = Vec::new();
        let max_results = 100;
        for entry in WalkDir::new(&self.workspace_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (idx, line) in contents.lines().enumerate() {
                if re.is_match(line) {
                    let relative = entry
                        .path()
                        .strip_prefix(&self.workspace_root)
                        .unwrap_or(entry.path());
                    results.push(Location {
                        file: relative.display().to_string(),
                        line: (idx + 1) as u32,
                        column: find_column(line, symbol) as u32,
                        context: line.trim().to_owned(),
                    });
                    if results.len() >= max_results {
                        return Ok(results);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get hover information for a symbol at a given position.
    ///
    /// Returns the definition context if found, otherwise returns the
    /// surrounding lines in the specified file.
    pub fn hover(&self, file_path: &str, symbol: &str) -> Result<String> {
        // First try to find the definition.
        let definitions = self.find_definitions(symbol, None)?;
        if !definitions.is_empty() {
            let def = &definitions[0];
            return Ok(format!(
                "Definition of '{symbol}' at {}:{}:\n  {}",
                def.file, def.line, def.context
            ));
        }

        // Fallback: find context in the specified file.
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(format!("No hover information available for '{symbol}'."));
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;
        let escaped = regex::escape(symbol);
        let pattern = format!(r"\b{escaped}\b");
        let re = regex::Regex::new(&pattern).unwrap_or_else(|_| {
            regex::Regex::new(&regex::escape(symbol)).expect("escaped pattern should be valid")
        });
        let lines: Vec<&str> = contents.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                let start = idx.saturating_sub(2);
                let end = (idx + 3).min(lines.len());
                let context_lines: Vec<String> = (start..end)
                    .map(|i| {
                        let prefix = if i == idx { ">" } else { " " };
                        format!("{}{}: {}", prefix, i + 1, lines[i])
                    })
                    .collect();
                return Ok(format!(
                    "Hover info for '{symbol}' in {file_path}:\n{}",
                    context_lines.join("\n")
                ));
            }
        }

        Ok(format!("No hover information available for '{symbol}'."))
    }

    /// Run diagnostics for the workspace.
    ///
    /// Detects the project type and runs the appropriate linter:
    /// - `.rs` files → `cargo check --message-format=short`
    /// - `.ts`/`.js` files → `tsc --noEmit` (if available)
    /// - `.py` files → `python -m py_compile` (if available)
    pub async fn diagnostics(&self, file_path: &str) -> Result<Vec<Diagnostic>> {
        let ext = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        match ext {
            "rs" => self.rust_diagnostics().await,
            "ts" | "tsx" | "js" | "jsx" => self.typescript_diagnostics(file_path).await,
            "py" => self.python_diagnostics(file_path).await,
            _ => {
                // Generic: try cargo check if Cargo.toml exists.
                if self.workspace_root.join("Cargo.toml").exists() {
                    self.rust_diagnostics().await
                } else {
                    Ok(vec![])
                }
            }
        }
    }

    /// Get text-based completion suggestions.
    ///
    /// Analyzes the file content around the given position and suggests
    /// symbols that appear elsewhere in the workspace.
    pub fn completion(&self, file_path: &str, line: u32, column: u32) -> Result<Vec<String>> {
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(vec![]);
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;

        // Extract the partial identifier at the cursor position.
        let col = column as usize;
        let line_index = line.saturating_sub(1) as usize;
        let current_line = contents.lines().nth(line_index).unwrap_or("");
        if col == 0 || col > current_line.len() {
            return Ok(vec![]);
        }
        let before_cursor = &current_line[..col.min(current_line.len())];
        let partial: String = before_cursor
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();

        if partial.len() < 2 {
            return Ok(vec![]);
        }

        // Search for matching identifiers in the workspace.
        let escaped = regex::escape(&partial);
        let pattern = format!(r"\b({escaped}\w*)\b");
        let re = regex::Regex::new(&pattern).unwrap_or_else(|_| {
            regex::Regex::new(&regex::escape(&partial)).expect("escaped pattern should be valid")
        });

        let mut seen = std::collections::HashSet::new();
        let mut suggestions = Vec::new();
        for entry in WalkDir::new(&self.workspace_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(file_contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for cap in re.captures_iter(&file_contents) {
                if let Some(m) = cap.get(1) {
                    let word = m.as_str().to_owned();
                    if seen.insert(word.clone()) {
                        suggestions.push(word);
                        if suggestions.len() >= 20 {
                            return Ok(suggestions);
                        }
                    }
                }
            }
        }

        Ok(suggestions)
    }

    // -----------------------------------------------------------------------
    // Position-based lookups (used by TS-parity operations)
    // -----------------------------------------------------------------------

    /// Find definitions at a given position in a file.
    ///
    /// Extracts the symbol at the given line/character and delegates to
    /// [`find_definitions`].
    pub fn find_definitions_at(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>> {
        let symbol = self.extract_symbol_at(file_path, line, character)?;
        match symbol {
            Some(sym) => self.find_definitions(&sym, Some(file_path)),
            None => Ok(vec![]),
        }
    }

    /// Find references at a given position in a file.
    pub fn find_references_at(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>> {
        let symbol = self.extract_symbol_at(file_path, line, character)?;
        match symbol {
            Some(sym) => self.find_references(&sym),
            None => Ok(vec![]),
        }
    }

    /// Get hover information at a given position in a file.
    pub fn hover_at(&self, file_path: &str, line: u32, character: u32) -> Result<String> {
        let symbol = self.extract_symbol_at(file_path, line, character)?;
        match symbol {
            Some(sym) => self.hover(file_path, &sym),
            None => Ok("No symbol found at the given position.".to_owned()),
        }
    }

    /// Get all symbols (functions, structs, enums, etc.) in a document.
    ///
    /// Returns a list of formatted symbol strings with kind, name, and line.
    pub fn document_symbols(&self, file_path: &str) -> Result<Vec<String>> {
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(vec![]);
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;

        let patterns = [
            (r"(?:pub\s+)?(?:async\s+)?fn\s+(\w+)", "function"),
            (r"(?:pub\s+)?struct\s+(\w+)", "struct"),
            (r"(?:pub\s+)?enum\s+(\w+)", "enum"),
            (r"impl\s+(?:<[^>]*>\s+)?(\w+)", "impl"),
            (r"(?:pub\s+)?trait\s+(\w+)", "trait"),
            (r"(?:pub\s+)?type\s+(\w+)", "type alias"),
            (r"(?:pub\s+)?const\s+(\w+)", "constant"),
            (r"(?:pub\s+)?static\s+(\w+)", "static"),
            (r"(?:pub\s+)?mod\s+(\w+)", "module"),
        ];

        let mut results = Vec::new();
        for (idx, line) in contents.lines().enumerate() {
            let trimmed = line.trim();
            for (pattern, kind) in &patterns {
                let re = regex::Regex::new(pattern).expect("hard-coded symbol regex must compile");
                if let Some(cap) = re.captures(trimmed)
                    && let Some(name) = cap.get(1)
                {
                    results.push(format!("[{}] {} (line {})", kind, name.as_str(), idx + 1));
                }
            }
            if results.len() >= 100 {
                break;
            }
        }

        Ok(results)
    }

    /// Search for symbols across the workspace matching the given query.
    ///
    /// Uses a text-based search similar to `find_definitions` but returns
    /// formatted strings with kind, name, file, and line.
    pub fn workspace_symbols(&self, query: &str) -> Result<Vec<String>> {
        if query.is_empty() {
            return Ok(vec![]);
        }

        let escaped = regex::escape(query);
        let patterns = [
            (
                format!(r"(?:pub\s+)?(?:async\s+)?fn\s+({escaped})\b"),
                "function",
            ),
            (format!(r"(?:pub\s+)?struct\s+({escaped})\b"), "struct"),
            (format!(r"(?:pub\s+)?enum\s+({escaped})\b"), "enum"),
            (format!(r"impl\s+(?:<[^>]*>\s+)?({escaped})\b"), "impl"),
            (format!(r"(?:pub\s+)?trait\s+({escaped})\b"), "trait"),
            (format!(r"\b({escaped}\w*)\b"), "identifier"),
        ];

        let mut results = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in WalkDir::new(&self.workspace_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(file_contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let relative = entry
                .path()
                .strip_prefix(&self.workspace_root)
                .unwrap_or(entry.path())
                .display()
                .to_string();

            for (idx, line) in file_contents.lines().enumerate() {
                for (pattern, kind) in &patterns {
                    let re =
                        regex::Regex::new(pattern).expect("hard-coded symbol regex must compile");
                    if let Some(cap) = re.captures(line)
                        && let Some(name) = cap.get(1)
                    {
                        let key = format!("{kind}:{}", name.as_str());
                        if seen.insert(key) {
                            results.push(format!(
                                "[{}] {} ({}:{})",
                                kind,
                                name.as_str(),
                                relative,
                                idx + 1
                            ));
                            if results.len() >= 50 {
                                return Ok(results);
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Find implementations of a symbol (trait impls, interface impls).
    pub fn find_implementations(&self, symbol: &str) -> Result<Vec<Location>> {
        let escaped = regex::escape(symbol);
        let patterns = [
            format!(r"impl\s+(?:<[^>]*>\s+)?{escaped}\s+for\s+(\w+)"),
            format!(r"impl\s+(?:<[^>]*>\s+)?{escaped}\b"),
        ];

        let combined = patterns.join("|");
        let re = regex::Regex::new(&format!("(?:{combined})"))
            .context("invalid pattern for implementations")?;

        let mut results = Vec::new();
        for entry in WalkDir::new(&self.workspace_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            for (idx, line) in contents.lines().enumerate() {
                if re.is_match(line) {
                    let relative = entry
                        .path()
                        .strip_prefix(&self.workspace_root)
                        .unwrap_or(entry.path());
                    results.push(Location {
                        file: relative.display().to_string(),
                        line: (idx + 1) as u32,
                        column: find_column(line, symbol) as u32,
                        context: line.trim().to_owned(),
                    });
                    if results.len() >= 50 {
                        return Ok(results);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Find implementations at a given position.
    pub fn find_implementations_at(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<Location>> {
        let symbol = self.extract_symbol_at(file_path, line, character)?;
        match symbol {
            Some(sym) => self.find_implementations(&sym),
            None => Ok(vec![]),
        }
    }

    /// Prepare call hierarchy items at a given position.
    ///
    /// Returns the function/method at the position, if any, formatted for
    /// display.
    pub fn prepare_call_hierarchy(
        &self,
        file_path: &str,
        line: u32,
        _character: u32,
    ) -> Result<Vec<String>> {
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(vec![]);
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;

        let line_index = line.saturating_sub(1) as usize;
        let target_line = contents.lines().nth(line_index).unwrap_or("");

        // Look for a function/method definition on or near this line.
        let fn_re = regex::Regex::new(r"(?:pub\s+)?(?:async\s+)?fn\s+(\w+)")
            .expect("hard-coded function regex must compile");
        if let Some(cap) = fn_re.captures(target_line) {
            return Ok(vec![format!(
                "[function] {} ({}:{})",
                cap.get(1)
                    .expect("function regex has capture group")
                    .as_str(),
                file_path,
                line
            )]);
        }

        // Check a few lines above in case the fn keyword is on a preceding line.
        let lines: Vec<&str> = contents.lines().collect();
        let start = line_index.saturating_sub(3);
        for i in start..line_index.min(lines.len()) {
            if let Some(cap) = fn_re.captures(lines[i]) {
                return Ok(vec![format!(
                    "[function] {} ({}:{})",
                    cap.get(1)
                        .expect("function regex has capture group")
                        .as_str(),
                    file_path,
                    i + 1
                )]);
            }
        }

        Ok(vec![])
    }

    /// Find incoming calls (callers) for the function at the given position.
    ///
    /// Searches the workspace for call sites of the function at the position.
    pub fn incoming_calls(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Vec<String>> {
        let symbol = self.extract_symbol_at(file_path, line, character)?;
        let symbol = match symbol {
            Some(s) => s,
            None => return Ok(vec![]),
        };

        // Find the definition first to get the canonical name, then search for
        // call sites.
        let escaped = regex::escape(&symbol);
        let call_pattern = format!(r"\b{escaped}\s*\(");
        let re =
            regex::Regex::new(&call_pattern).context("invalid call pattern for incoming_calls")?;

        let mut results = Vec::new();
        for entry in WalkDir::new(&self.workspace_root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if is_ignored_path(entry.path()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let relative = entry
                .path()
                .strip_prefix(&self.workspace_root)
                .unwrap_or(entry.path())
                .display()
                .to_string();
            for (idx, l) in contents.lines().enumerate() {
                if re.is_match(l) {
                    results.push(format!("{}:{}: {}", relative, idx + 1, l.trim()));
                    if results.len() >= 50 {
                        return Ok(results);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Find outgoing calls (callees) for the function at the given position.
    ///
    /// Parses the function body at the position and extracts called functions.
    pub fn outgoing_calls(
        &self,
        file_path: &str,
        line: u32,
        _character: u32,
    ) -> Result<Vec<String>> {
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(vec![]);
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;

        let lines: Vec<&str> = contents.lines().collect();
        let line_index = line.saturating_sub(1) as usize;

        // Find the function body starting from the given line.
        // Collect lines until the brace count returns to zero or we hit another fn.
        let mut brace_count = 0i32;
        let mut started = false;
        let mut body_lines = Vec::new();

        for i in line_index..lines.len() {
            let l = lines[i];
            for ch in l.chars() {
                match ch {
                    '{' => {
                        brace_count += 1;
                        started = true;
                    }
                    '}' => {
                        brace_count -= 1;
                    }
                    _ => {}
                }
            }
            body_lines.push(l);
            if started && brace_count <= 0 {
                break;
            }
        }

        // Extract call identifiers from the body.
        let call_re =
            regex::Regex::new(r"\b([a-z_]\w*)\s*\(").context("invalid call extraction pattern")?;

        // Filter out control flow keywords and the function's own name.
        let keywords = [
            "if", "else", "match", "while", "for", "loop", "return", "let", "fn", "pub", "async",
            "unsafe", "impl", "struct", "enum", "trait", "use", "mod", "where", "as", "super",
            "self", "Self", "crate",
        ];

        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        for l in &body_lines {
            for cap in call_re.captures_iter(l) {
                if let Some(name) = cap.get(1) {
                    let n = name.as_str();
                    if !keywords.contains(&n) && seen.insert(n.to_owned()) {
                        results.push(format!("[call] {n}"));
                        if results.len() >= 30 {
                            return Ok(results);
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Extract the identifier at the given line/character position in a file.
    fn extract_symbol_at(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<String>> {
        let target = self.workspace_root.join(file_path);
        if !target.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&target)
            .with_context(|| format!("failed to read {file_path}"))?;

        let line_index = line.saturating_sub(1) as usize;
        let target_line = match contents.lines().nth(line_index) {
            Some(l) => l,
            None => return Ok(None),
        };

        let char_idx = (character as usize).saturating_sub(1);
        if char_idx >= target_line.len() {
            return Ok(None);
        }

        // Walk backwards and forwards from the cursor to find the full identifier.
        let chars: Vec<char> = target_line.chars().collect();
        let start = chars[..char_idx]
            .iter()
            .rposition(|c| !c.is_alphanumeric() && *c != '_')
            .map(|p| p + 1)
            .unwrap_or(0);
        let end = chars[char_idx..]
            .iter()
            .position(|c| !c.is_alphanumeric() && *c != '_')
            .map(|p| char_idx + p)
            .unwrap_or(chars.len());

        let symbol: String = chars[start..end].iter().collect();
        if symbol.is_empty() {
            Ok(None)
        } else {
            Ok(Some(symbol))
        }
    }

    // -----------------------------------------------------------------------
    // Language-specific diagnostics
    // -----------------------------------------------------------------------

    /// Run `cargo check` and parse diagnostics.
    async fn rust_diagnostics(&self) -> Result<Vec<Diagnostic>> {
        let output = tokio::process::Command::new("cargo")
            .args(["check", "--message-format=short"])
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Ok(parse_cargo_diagnostics(&stderr))
            }
            Err(e) => Ok(vec![Diagnostic {
                severity: "info".to_owned(),
                message: format!("Could not run cargo check: {e}"),
                line: 0,
                file: String::new(),
            }]),
        }
    }

    /// Run `tsc --noEmit` for TypeScript/JavaScript diagnostics.
    async fn typescript_diagnostics(&self, file_path: &str) -> Result<Vec<Diagnostic>> {
        let output = tokio::process::Command::new("npx")
            .args(["tsc", "--noEmit", "--pretty", "false"])
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Ok(parse_tsc_diagnostics(&stdout, file_path))
            }
            Err(_) => Ok(vec![Diagnostic {
                severity: "info".to_owned(),
                message: "TypeScript compiler not available.".to_owned(),
                line: 0,
                file: file_path.to_owned(),
            }]),
        }
    }

    /// Run `python -m py_compile` for Python diagnostics.
    async fn python_diagnostics(&self, file_path: &str) -> Result<Vec<Diagnostic>> {
        let target = self.workspace_root.join(file_path);
        let output = tokio::process::Command::new("python")
            .args(["-m", "py_compile", &target.display().to_string()])
            .current_dir(&self.workspace_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match output {
            Ok(output) => {
                if output.status.success() {
                    Ok(vec![])
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Ok(vec![Diagnostic {
                        severity: "error".to_owned(),
                        message: stderr.trim().to_owned(),
                        line: 0,
                        file: file_path.to_owned(),
                    }])
                }
            }
            Err(_) => Ok(vec![Diagnostic {
                severity: "info".to_owned(),
                message: "Python interpreter not available.".to_owned(),
                line: 0,
                file: file_path.to_owned(),
            }]),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse cargo check output into diagnostics.
fn parse_cargo_diagnostics(output: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        // Format: "src/main.rs:10:5: error[E0308]: mismatched types"
        let parts: Vec<&str> = line.splitn(4, ':').collect();
        if parts.len() >= 4 {
            let file = parts[0].trim().to_owned();
            let line_num = parts[1].trim().parse::<u32>().unwrap_or(0);
            let severity_and_code = parts[3].trim();
            let severity = if severity_and_code.starts_with("error") {
                "error"
            } else if severity_and_code.starts_with("warning") {
                "warning"
            } else {
                "info"
            };
            diagnostics.push(Diagnostic {
                severity: severity.to_owned(),
                message: severity_and_code.to_owned(),
                line: line_num,
                file,
            });
            if diagnostics.len() >= 50 {
                break;
            }
        }
    }
    diagnostics
}

/// Parse TypeScript compiler output into diagnostics.
fn parse_tsc_diagnostics(output: &str, default_file: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for line in output.lines() {
        // Format: "src/file.ts(10,5): error TS2304: Cannot find name 'x'."
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() >= 3 {
            let file_and_loc = parts[0].trim();
            // Line number is in the parenthesized portion of parts[0], e.g. "src/app.ts(15,3)"
            let line_num = file_and_loc
                .rfind('(')
                .map(|pos| extract_line_number(&file_and_loc[pos..]))
                .unwrap_or(0);
            let file = if let Some(pos) = file_and_loc.rfind('(') {
                file_and_loc[..pos].to_owned()
            } else {
                file_and_loc.to_owned()
            };
            let severity_part = parts[1].trim();
            let severity = if severity_part.starts_with("error") {
                "error"
            } else {
                "warning"
            };
            let message = parts[2].trim().to_owned();
            diagnostics.push(Diagnostic {
                severity: severity.to_owned(),
                message,
                line: line_num,
                file,
            });
            if diagnostics.len() >= 50 {
                break;
            }
        }
    }
    if diagnostics.is_empty() && !output.trim().is_empty() {
        diagnostics.push(Diagnostic {
            severity: "info".to_owned(),
            message: output.trim().chars().take(500).collect(),
            line: 0,
            file: default_file.to_owned(),
        });
    }
    diagnostics
}

/// Extract a line number from a string like "(10,5)".
fn extract_line_number(s: &str) -> u32 {
    let s = s.trim();
    let s = s.trim_start_matches('(');
    let s = s.split(',').next().unwrap_or("0");
    s.parse().unwrap_or(0)
}

/// Find the approximate column of a symbol in a line.
fn find_column(line: &str, symbol: &str) -> usize {
    line.find(symbol).map(|i| i + 1).unwrap_or(1)
}

/// Check if a path should be ignored during search.
fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|component| IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref()))
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a list of locations as a human-readable string.
pub fn format_locations(locations: &[Location]) -> String {
    if locations.is_empty() {
        return "No results found.".to_owned();
    }
    locations
        .iter()
        .map(|loc| format!("{}:{}: {}", loc.file, loc.line, loc.context))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a list of diagnostics as a human-readable string.
pub fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics found.".to_owned();
    }
    diagnostics
        .iter()
        .map(|d| {
            if d.file.is_empty() {
                format!("[{}] {}", d.severity, d.message)
            } else {
                format!("[{}] {}:{}: {}", d.severity, d.file, d.line, d.message)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format completion suggestions.
pub fn format_completions(suggestions: &[String]) -> String {
    if suggestions.is_empty() {
        return "No completions available.".to_owned();
    }
    format!("Completions:\n{}", suggestions.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cargo_diagnostics_extracts_errors() {
        let output = "src/main.rs:10:5: error[E0308]: mismatched types\nsrc/lib.rs:5:1: warning: unused variable";
        let diags = parse_cargo_diagnostics(output);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, "error");
        assert_eq!(diags[0].line, 10);
        assert_eq!(diags[1].severity, "warning");
    }

    #[test]
    fn parse_tsc_diagnostics_extracts_errors() {
        let output = "src/app.ts(15,3): error TS2304: Cannot find name 'foo'.";
        let diags = parse_tsc_diagnostics(output, "src/app.ts");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, "error");
        assert_eq!(diags[0].line, 15);
    }

    #[test]
    fn extract_line_number_handles_parens() {
        assert_eq!(extract_line_number("(10,5)"), 10);
        assert_eq!(extract_line_number("(1,1)"), 1);
        assert_eq!(extract_line_number("invalid"), 0);
    }

    #[test]
    fn format_locations_empty() {
        assert_eq!(format_locations(&[]), "No results found.");
    }

    #[test]
    fn format_diagnostics_empty() {
        assert_eq!(format_diagnostics(&[]), "No diagnostics found.");
    }
}
