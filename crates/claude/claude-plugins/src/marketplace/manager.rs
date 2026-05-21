//! Marketplace manager for plugin discovery and management.
//!
//! Manages known marketplace sources, caches marketplace manifests locally,
//! and provides plugin lookup across marketplaces.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single plugin entry in a marketplace index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Plugin description.
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin author.
    #[serde(default)]
    pub author: Option<String>,
    /// Plugin homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
    /// Plugin keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Plugin dependencies.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Marketplace name this entry belongs to.
    #[serde(default)]
    pub marketplace: Option<String>,
}

/// A marketplace index — list of available plugins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceIndex {
    /// Marketplace name.
    pub name: String,
    /// List of plugin entries.
    pub entries: Vec<MarketplaceEntry>,
    /// When the index was last fetched.
    #[serde(default)]
    pub fetched_at: Option<String>,
}

/// A known marketplace configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownMarketplace {
    /// Marketplace name.
    pub name: String,
    /// Marketplace source.
    pub source: MarketplaceSourceConfig,
    /// Whether auto-update is enabled.
    #[serde(default)]
    pub auto_update: Option<bool>,
}

/// Marketplace source configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "lowercase")]
pub enum MarketplaceSourceConfig {
    /// GitHub repository.
    Github { repo: String },
    /// URL to marketplace index.
    Url { url: String },
    /// Git repository.
    Git { url: String },
    /// Local directory.
    Directory { path: String },
}

/// Marketplace manager for tracking and querying marketplaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceManager {
    /// Known marketplaces by name.
    marketplaces: HashMap<String, KnownMarketplace>,
    /// Cached marketplace indices.
    indices: HashMap<String, MarketplaceIndex>,
    /// Cache directory for marketplace data.
    cache_dir: PathBuf,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

impl MarketplaceManager {
    /// Create a new marketplace manager with the given cache directory.
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            marketplaces: HashMap::new(),
            indices: HashMap::new(),
            cache_dir,
        }
    }

    /// List all known marketplaces.
    pub fn list_marketplaces(&self) -> Vec<&KnownMarketplace> {
        self.marketplaces.values().collect()
    }

    /// Add a marketplace.
    pub fn add_marketplace(&mut self, marketplace: KnownMarketplace) {
        self.marketplaces
            .insert(marketplace.name.clone(), marketplace);
    }

    /// Remove a marketplace by name.
    pub fn remove_marketplace(&mut self, name: &str) -> bool {
        self.indices.remove(name);
        self.marketplaces.remove(name).is_some()
    }

    /// Refresh a marketplace — fetch the latest plugin index.
    pub fn refresh_marketplace(&mut self, name: &str) -> Result<(), String> {
        let marketplace = self
            .marketplaces
            .get(name)
            .cloned()
            .ok_or_else(|| format!("marketplace '{name}' not found"))?;

        fs::create_dir_all(&self.cache_dir).map_err(|error| {
            format!(
                "failed to create cache dir {}: {error}",
                self.cache_dir.display()
            )
        })?;

        let mut index = match marketplace.source {
            MarketplaceSourceConfig::Directory { path } => {
                read_marketplace_index_from_path(name, Path::new(&path))?
            }
            MarketplaceSourceConfig::Url { url } => fetch_marketplace_index_from_url(name, &url)?,
            MarketplaceSourceConfig::Github { repo } => {
                let checkout_dir = marketplace_checkout_dir(&self.cache_dir, name);
                refresh_git_checkout(&format!("https://github.com/{repo}.git"), &checkout_dir)?;
                read_marketplace_index_from_path(name, &checkout_dir)?
            }
            MarketplaceSourceConfig::Git { url } => {
                let checkout_dir = marketplace_checkout_dir(&self.cache_dir, name);
                refresh_git_checkout(&url, &checkout_dir)?;
                read_marketplace_index_from_path(name, &checkout_dir)?
            }
        };
        index.fetched_at = Some(chrono::Utc::now().to_rfc3339());
        persist_marketplace_index(&self.cache_dir, name, &index)?;

        self.indices.insert(name.to_owned(), index);
        Ok(())
    }

    /// Get a cached marketplace index.
    pub fn get_marketplace_index(&self, name: &str) -> Option<&MarketplaceIndex> {
        self.indices.get(name)
    }

    /// Look up a plugin by ID across all marketplaces.
    pub fn get_plugin_by_id(
        &self,
        plugin_name: &str,
        marketplace_name: Option<&str>,
    ) -> Option<&MarketplaceEntry> {
        if let Some(mkt) = marketplace_name {
            self.indices
                .get(mkt)
                .and_then(|idx| idx.entries.iter().find(|e| e.name == plugin_name))
        } else {
            // Search all marketplaces
            for index in self.indices.values() {
                if let Some(entry) = index.entries.iter().find(|e| e.name == plugin_name) {
                    return Some(entry);
                }
            }
            None
        }
    }

    /// Search for plugins across all marketplaces.
    pub fn search_plugins(&self, query: &str) -> Vec<&MarketplaceEntry> {
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for index in self.indices.values() {
            for entry in &index.entries {
                let matches_name = entry.name.to_lowercase().contains(&query_lower);
                let matches_desc = entry
                    .description
                    .as_ref()
                    .is_some_and(|d| d.to_lowercase().contains(&query_lower));
                let matches_keyword = entry
                    .keywords
                    .iter()
                    .any(|k| k.to_lowercase().contains(&query_lower));

                if matches_name || matches_desc || matches_keyword {
                    results.push(entry);
                }
            }
        }

        results
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Number of known marketplaces.
    pub fn len(&self) -> usize {
        self.marketplaces.len()
    }

    /// Whether there are no marketplaces.
    pub fn is_empty(&self) -> bool {
        self.marketplaces.is_empty()
    }
}

fn marketplace_checkout_dir(cache_dir: &Path, name: &str) -> PathBuf {
    cache_dir
        .join("checkouts")
        .join(name.replace(['\\', '/', ':'], "_"))
}

fn persist_marketplace_index(
    cache_dir: &Path,
    name: &str,
    index: &MarketplaceIndex,
) -> Result<(), String> {
    let cache_file = cache_dir.join(format!("{}.json", name.replace(['\\', '/', ':'], "_")));
    let payload = serde_json::to_vec_pretty(index)
        .map_err(|error| format!("failed to serialize marketplace index: {error}"))?;
    fs::write(&cache_file, payload).map_err(|error| {
        format!(
            "failed to write cache file {}: {error}",
            cache_file.display()
        )
    })
}

fn read_marketplace_index_from_path(
    name: &str,
    source_path: &Path,
) -> Result<MarketplaceIndex, String> {
    let manifest_path = locate_marketplace_manifest(source_path)?;
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    parse_marketplace_index(name, &raw)
}

fn locate_marketplace_manifest(source_path: &Path) -> Result<PathBuf, String> {
    if source_path.is_file() {
        return Ok(source_path.to_path_buf());
    }

    let candidates = [
        source_path.join(".codex-plugin").join("marketplace.json"),
        source_path.join(".claude-plugin").join("marketplace.json"),
        source_path.join("marketplace.json"),
        source_path.join("index.json"),
    ];

    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| {
            format!(
                "no marketplace manifest found under {}",
                source_path.display()
            )
        })
}

fn parse_marketplace_index(name: &str, raw: &str) -> Result<MarketplaceIndex, String> {
    if let Ok(mut index) = serde_json::from_str::<MarketplaceIndex>(raw) {
        normalize_marketplace_index(name, &mut index);
        return Ok(index);
    }

    let value: Value =
        serde_json::from_str(raw).map_err(|error| format!("invalid marketplace JSON: {error}"))?;

    let mut index = if value.is_array() {
        MarketplaceIndex {
            name: name.to_owned(),
            entries: serde_json::from_value(value)
                .map_err(|error| format!("invalid marketplace entry list: {error}"))?,
            fetched_at: None,
        }
    } else {
        let entries_value = value
            .get("entries")
            .cloned()
            .or_else(|| value.get("plugins").cloned())
            .ok_or_else(|| "marketplace JSON must contain `entries` or `plugins`".to_owned())?;

        MarketplaceIndex {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_owned(),
            entries: serde_json::from_value(entries_value)
                .map_err(|error| format!("invalid marketplace entries: {error}"))?,
            fetched_at: value
                .get("fetched_at")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    };

    normalize_marketplace_index(name, &mut index);
    Ok(index)
}

fn normalize_marketplace_index(requested_name: &str, index: &mut MarketplaceIndex) {
    if index.name.trim().is_empty() {
        index.name = requested_name.to_owned();
    }
    for entry in &mut index.entries {
        if entry.marketplace.is_none() {
            entry.marketplace = Some(index.name.clone());
        }
    }
}

fn fetch_marketplace_index_from_url(name: &str, url: &str) -> Result<MarketplaceIndex, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("failed to build HTTP client: {error}"))?;
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("failed to fetch marketplace {url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "marketplace fetch failed with HTTP {} from {}",
            response.status(),
            url
        ));
    }
    let raw = response
        .text()
        .map_err(|error| format!("failed to read marketplace body from {url}: {error}"))?;
    parse_marketplace_index(name, &raw)
}

fn refresh_git_checkout(url: &str, checkout_dir: &Path) -> Result<(), String> {
    if checkout_dir.join(".git").exists() {
        run_git(
            checkout_dir.parent().unwrap_or(checkout_dir),
            [
                "-C",
                checkout_dir.to_string_lossy().as_ref(),
                "pull",
                "--ff-only",
            ],
        )
        .map(|_| ())
    } else {
        if checkout_dir.exists() {
            fs::remove_dir_all(checkout_dir).map_err(|error| {
                format!(
                    "failed to reset stale checkout {}: {error}",
                    checkout_dir.display()
                )
            })?;
        }
        if let Some(parent) = checkout_dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create checkout parent {}: {error}",
                    parent.display()
                )
            })?;
        }
        run_git(
            checkout_dir.parent().unwrap_or(checkout_dir),
            [
                "clone",
                "--depth",
                "1",
                url,
                checkout_dir.to_string_lossy().as_ref(),
            ],
        )
        .map(|_| ())
    }
}

fn run_git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("failed to execute git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        Err(format!("git {} failed: {}", args.join(" "), detail))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;
    use tempfile::tempdir;

    fn write_marketplace_manifest(root: &Path, entries_json: &str) {
        let manifest_dir = root.join(".codex-plugin");
        fs::create_dir_all(&manifest_dir).expect("create manifest dir");
        fs::write(
            manifest_dir.join("marketplace.json"),
            format!(
                r#"{{
                    "name": "test-mkt",
                    "entries": {entries_json}
                }}"#
            ),
        )
        .expect("write marketplace manifest");
    }

    #[test]
    fn new_manager_is_empty() {
        let mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        assert!(mgr.is_empty());
    }

    #[test]
    fn add_and_list_marketplaces() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        mgr.add_marketplace(KnownMarketplace {
            name: "test-mkt".to_owned(),
            source: MarketplaceSourceConfig::Github {
                repo: "org/repo".to_owned(),
            },
            auto_update: None,
        });
        assert_eq!(mgr.len(), 1);
        let list = mgr.list_marketplaces();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn remove_marketplace() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        mgr.add_marketplace(KnownMarketplace {
            name: "test-mkt".to_owned(),
            source: MarketplaceSourceConfig::Github {
                repo: "org/repo".to_owned(),
            },
            auto_update: None,
        });
        assert!(mgr.remove_marketplace("test-mkt"));
        assert!(mgr.is_empty());
    }

    #[test]
    fn remove_nonexistent() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        assert!(!mgr.remove_marketplace("nonexistent"));
    }

    #[test]
    fn refresh_marketplace() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("marketplace");
        write_marketplace_manifest(
            &source,
            r#"[{"name":"my-plugin","version":"1.0.0","description":"A test plugin"}]"#,
        );

        let mut mgr = MarketplaceManager::new(temp.path().join("cache"));
        mgr.add_marketplace(KnownMarketplace {
            name: "test-mkt".to_owned(),
            source: MarketplaceSourceConfig::Directory {
                path: source.to_string_lossy().into_owned(),
            },
            auto_update: None,
        });
        assert!(mgr.refresh_marketplace("test-mkt").is_ok());
        let index = mgr
            .get_marketplace_index("test-mkt")
            .expect("marketplace index");
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].marketplace.as_deref(), Some("test-mkt"));
        assert!(temp.path().join("cache").join("test-mkt.json").exists());
    }

    #[test]
    fn refresh_nonexistent_fails() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        assert!(mgr.refresh_marketplace("nonexistent").is_err());
    }

    #[test]
    fn get_plugin_by_id() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        mgr.add_marketplace(KnownMarketplace {
            name: "test-mkt".to_owned(),
            source: MarketplaceSourceConfig::Github {
                repo: "org/repo".to_owned(),
            },
            auto_update: None,
        });
        mgr.indices.insert(
            "test-mkt".to_owned(),
            MarketplaceIndex {
                name: "test-mkt".to_owned(),
                entries: vec![MarketplaceEntry {
                    name: "my-plugin".to_owned(),
                    version: "1.0.0".to_owned(),
                    description: Some("A test plugin".to_owned()),
                    author: None,
                    homepage: None,
                    keywords: vec![],
                    dependencies: vec![],
                    marketplace: Some("test-mkt".to_owned()),
                }],
                fetched_at: None,
            },
        );

        let entry = mgr.get_plugin_by_id("my-plugin", Some("test-mkt"));
        assert!(entry.is_some());
        assert_eq!(entry.expect("entry").name, "my-plugin");
    }

    #[test]
    fn search_plugins() {
        let mut mgr = MarketplaceManager::new(PathBuf::from("/tmp/cache"));
        mgr.indices.insert(
            "mkt".to_owned(),
            MarketplaceIndex {
                name: "mkt".to_owned(),
                entries: vec![
                    MarketplaceEntry {
                        name: "rust-formatter".to_owned(),
                        version: "1.0.0".to_owned(),
                        description: Some("Format Rust code".to_owned()),
                        author: None,
                        homepage: None,
                        keywords: vec!["rust".to_owned()],
                        dependencies: vec![],
                        marketplace: None,
                    },
                    MarketplaceEntry {
                        name: "python-linter".to_owned(),
                        version: "1.0.0".to_owned(),
                        description: Some("Lint Python code".to_owned()),
                        author: None,
                        homepage: None,
                        keywords: vec!["python".to_owned()],
                        dependencies: vec![],
                        marketplace: None,
                    },
                ],
                fetched_at: None,
            },
        );

        let results = mgr.search_plugins("rust");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "rust-formatter");
    }

    #[test]
    fn refresh_marketplace_from_url() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let body =
                r#"{"name":"remote-mkt","plugins":[{"name":"remote-plugin","version":"0.1.0"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("write");
        });

        let temp = tempdir().expect("tempdir");
        let mut mgr = MarketplaceManager::new(temp.path().join("cache"));
        mgr.add_marketplace(KnownMarketplace {
            name: "remote".to_owned(),
            source: MarketplaceSourceConfig::Url {
                url: format!("http://{address}/marketplace.json"),
            },
            auto_update: None,
        });

        mgr.refresh_marketplace("remote").expect("refresh");
        server.join().expect("join server");

        let index = mgr
            .get_marketplace_index("remote")
            .expect("marketplace index");
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].name, "remote-plugin");
        assert_eq!(index.entries[0].marketplace.as_deref(), Some("remote-mkt"));
    }
}
