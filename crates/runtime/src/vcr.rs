use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use regex::Regex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcrConfig {
    pub enabled: bool,
    pub record_mode: bool,
    pub fixtures_root: PathBuf,
}

impl Default for VcrConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            record_mode: false,
            fixtures_root: PathBuf::from("fixtures"),
        }
    }
}

impl VcrConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("NODE_ENV").ok().as_deref() == Some("test")
            || (std::env::var("USER_TYPE").ok().as_deref() == Some("ant")
                && std::env::var("FORCE_VCR")
                    .map(|v| v == "1" || v == "true")
                    .unwrap_or(false));

        let record_mode = std::env::var("VCR_RECORD")
            .map(|v| v == "1" || v == "true")
            .unwrap_or(false);

        let fixtures_root = std::env::var("CLAUDE_CODE_TEST_FIXTURES_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

        Self {
            enabled,
            record_mode,
            fixtures_root,
        }
    }

    pub fn should_use_vcr(&self) -> bool {
        self.enabled
    }

    pub fn should_record(&self) -> bool {
        self.enabled && self.record_mode
    }

    pub fn is_ci(&self) -> bool {
        std::env::var("CI").is_ok() || std::env::var("ENV").ok().as_deref() == Some("ci")
    }
}

pub struct VcrRecorder {
    config: VcrConfig,
    cwd: PathBuf,
    config_home: PathBuf,
}

impl VcrRecorder {
    pub fn new(config: VcrConfig) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let config_home = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("claude");

        Self {
            config,
            cwd,
            config_home,
        }
    }

    pub fn from_env() -> Self {
        Self::new(VcrConfig::from_env())
    }

    pub fn config(&self) -> &VcrConfig {
        &self.config
    }

    pub fn compute_hash(&self, input: &serde_json::Value) -> String {
        let json_str = serde_json::to_string(input).unwrap_or_default();
        let dehydrated = self.dehydrate_value(&json_str);
        format!("{:x}", Sha256::digest(dehydrated.as_bytes()))
            .chars()
            .take(12)
            .collect()
    }

    pub fn fixture_path(&self, fixture_name: &str, hash: &str) -> PathBuf {
        self.config.fixtures_root.join(format!("{}-{}.json", fixture_name, hash))
    }

    fn dehydrate_value(&self, s: &str) -> String {
        let cwd_str = self.cwd.to_string_lossy();
        let config_home_str = self.config_home.to_string_lossy();

        let mut result = s.to_string();
        result = result.replace(&*cwd_str, "[CWD]");
        result = result.replace(&*config_home_str, "[CONFIG_HOME]");

        if cfg!(windows) {
            let cwd_fwd = cwd_str.replace('\\', "/");
            let config_home_fwd = config_home_str.replace('\\', "/");
            result = result.replace(&cwd_fwd, "[CWD]");
            result = result.replace(&config_home_fwd, "[CONFIG_HOME]");

            let cwd_escaped = serde_json::to_string(&*cwd_str).unwrap_or_default();
            let cwd_escaped = cwd_escaped.trim_matches('"');
            let config_escaped = serde_json::to_string(&*config_home_str).unwrap_or_default();
            let config_escaped = config_escaped.trim_matches('"');
            result = result.replace(cwd_escaped, "[CWD]");
            result = result.replace(config_escaped, "[CONFIG_HOME]");
        }

        result = Regex::new(r#"num_files="\d+""#)
            .map(|re| re.replace_all(&result, r#"num_files="[NUM]""#).to_string())
            .unwrap_or(result);
        result = Regex::new(r#"duration_ms="\d+""#)
            .map(|re| re.replace_all(&result, r#"duration_ms="[DURATION]""#).to_string())
            .unwrap_or(result);
        result = Regex::new(r#"cost_usd="\d+""#)
            .map(|re| re.replace_all(&result, r#"cost_usd="[COST]""#).to_string())
            .unwrap_or(result);

        result
    }

    #[allow(dead_code)]
    fn hydrate_value(&self, s: &str) -> String {
        let mut result = s.to_string();
        result = result.replace("[NUM]", "1");
        result = result.replace("[DURATION]", "100");
        result = result.replace("[CONFIG_HOME]", &self.config_home.to_string_lossy());
        result = result.replace("[CWD]", &self.cwd.to_string_lossy());
        result
    }

    pub async fn read_fixture(&self, path: &Path) -> Result<Option<serde_json::Value>> {
        match fs::read_to_string(path).await {
            Ok(content) => {
                let value: serde_json::Value = serde_json::from_str(&content)?;
                Ok(Some(value))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn write_fixture(&self, path: &Path, fixture: &serde_json::Value) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let content = serde_json::to_string_pretty(fixture)?;
        fs::write(path, content).await?;
        Ok(())
    }
}

pub struct VcrFixtureManager {
    recorder: VcrRecorder,
}

impl VcrFixtureManager {
    pub fn new() -> Self {
        Self {
            recorder: VcrRecorder::from_env(),
        }
    }

    pub fn with_config(config: VcrConfig) -> Self {
        Self {
            recorder: VcrRecorder::new(config),
        }
    }

    pub async fn with_fixture<T, F, Fut>(
        &self,
        fixture_name: &str,
        input: &serde_json::Value,
        f: F,
    ) -> Result<T>
    where
        T: for<'de> Deserialize<'de> + Serialize,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        if !self.recorder.config().should_use_vcr() {
            return f().await;
        }

        let hash = self.recorder.compute_hash(input);
        let path = self.recorder.fixture_path(fixture_name, &hash);

        if let Some(cached) = self.recorder.read_fixture(&path).await? {
            if let Ok(result) = serde_json::from_value::<T>(cached) {
                return Ok(result);
            }
        }

        if self.recorder.config().is_ci() && !self.recorder.config().should_record() {
            return Err(anyhow!(
                "Fixture missing: {}. Re-run tests with VCR_RECORD=1, then commit the result.",
                path.display()
            ));
        }

        let result = f().await?;
        let result_value = serde_json::to_value(&result)?;
        self.recorder.write_fixture(&path, &result_value).await?;

        Ok(result)
    }
}

impl Default for VcrFixtureManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vcr_config_default() {
        let config = VcrConfig::default();
        assert!(!config.enabled);
        assert!(!config.record_mode);
    }

    #[test]
    fn test_compute_hash() {
        let recorder = VcrRecorder::new(VcrConfig::default());
        let input = serde_json::json!({"test": "value"});
        let hash = recorder.compute_hash(&input);
        assert_eq!(hash.len(), 12);
    }

    #[test]
    fn test_fixture_path() {
        let recorder = VcrRecorder::new(VcrConfig {
            fixtures_root: PathBuf::from("/tmp/fixtures"),
            ..Default::default()
        });
        let path = recorder.fixture_path("test-fixture", "abc123");
        assert!(path.to_string_lossy().ends_with("test-fixture-abc123.json"));
        assert!(path.to_string_lossy().contains("fixtures"));
    }
}
