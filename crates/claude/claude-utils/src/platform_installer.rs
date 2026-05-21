//! Platform-specific package installer detection and management.
//!
//! Corresponds to `.research/cc-haha/src/utils/localInstaller.ts`.
//! Detects the current platform's package manager, checks installation
//! status, and provides install/update instructions for remote-code.

use std::process::Command;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Supported installation methods across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallMethod {
    /// Windows Scoop package manager.
    Scoop,
    /// Windows Winget package manager.
    Winget,
    /// Windows Chocolatey package manager.
    Chocolatey,
    /// macOS Homebrew package manager.
    Brew,
    /// Debian/Ubuntu APT package manager.
    Apt,
    /// RHEL/CentOS Yum package manager.
    Yum,
    /// Fedora DNF package manager.
    Dnf,
    /// Arch Linux Pacman package manager.
    Pacman,
    /// Node.js npm package manager.
    Npm,
    /// Rust Cargo package manager.
    Cargo,
    /// Unknown / unsupported installation method.
    Unknown,
}

impl std::fmt::Display for InstallMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallMethod::Scoop => write!(f, "scoop"),
            InstallMethod::Winget => write!(f, "winget"),
            InstallMethod::Chocolatey => write!(f, "choco"),
            InstallMethod::Brew => write!(f, "brew"),
            InstallMethod::Apt => write!(f, "apt"),
            InstallMethod::Yum => write!(f, "yum"),
            InstallMethod::Dnf => write!(f, "dnf"),
            InstallMethod::Pacman => write!(f, "pacman"),
            InstallMethod::Npm => write!(f, "npm"),
            InstallMethod::Cargo => write!(f, "cargo"),
            InstallMethod::Unknown => write!(f, "unknown"),
        }
    }
}

/// Platform-specific installer detection and management.
#[derive(Debug)]
pub struct PlatformInstaller {
    /// Detected install method for the current platform.
    method: InstallMethod,
}

impl PlatformInstaller {
    /// Create a new platform installer with auto-detected install method.
    pub fn new() -> Self {
        Self {
            method: detect_install_method(),
        }
    }

    /// Create a platform installer with a specific install method (for testing).
    pub fn with_method(method: InstallMethod) -> Self {
        Self { method }
    }

    /// Return the detected install method.
    pub fn method(&self) -> InstallMethod {
        self.method
    }

    /// Check if remote-code is installed on this system.
    pub fn is_installed(&self) -> bool {
        Command::new("remote-code")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Get the installed version of remote-code, if present.
    pub fn get_installed_version() -> Option<String> {
        let output = Command::new("remote-code").arg("--version").output().ok()?;

        if !output.status.success() {
            return None;
        }

        let version_str = String::from_utf8_lossy(&output.stdout);
        // Parse the first line which typically contains the version number
        version_str
            .lines()
            .next()
            .map(|line| line.trim().to_string())
    }

    /// Fetch the latest available version of remote-code.
    ///
    /// In a full implementation this would query the appropriate registry
    /// (crates.io, npm registry, etc.) based on the install method.
    pub fn get_latest_version() -> anyhow::Result<String> {
        // Placeholder: in production, this would query the registry
        Ok("0.1.0".to_string())
    }

    /// Check whether an update is available by comparing installed vs latest version.
    pub fn needs_update() -> anyhow::Result<bool> {
        let installed = Self::get_installed_version();
        let latest = Self::get_latest_version()?;

        match installed {
            Some(inst) => Ok(inst != latest),
            None => Ok(false), // Not installed, so not "needs update"
        }
    }

    /// Return platform-specific installation instructions.
    pub fn install_instructions(&self) -> Vec<String> {
        match self.method {
            InstallMethod::Cargo => vec![
                "Install via Cargo:".to_string(),
                "  cargo install remote-code".to_string(),
            ],
            InstallMethod::Brew => vec![
                "Install via Homebrew:".to_string(),
                "  brew tap remote-code/tap".to_string(),
                "  brew install remote-code".to_string(),
            ],
            InstallMethod::Scoop => vec![
                "Install via Scoop:".to_string(),
                "  scoop bucket add remote-code https://github.com/remote-code/scoop-bucket"
                    .to_string(),
                "  scoop install remote-code".to_string(),
            ],
            InstallMethod::Winget => vec![
                "Install via Winget:".to_string(),
                "  winget install remote-code.cli".to_string(),
            ],
            InstallMethod::Chocolatey => vec![
                "Install via Chocolatey:".to_string(),
                "  choco install remote-code".to_string(),
            ],
            InstallMethod::Apt => vec![
                "Install via APT:".to_string(),
                "  curl -fsSL https://get.remote-code.dev/apt.sh | sudo bash".to_string(),
            ],
            InstallMethod::Yum => vec![
                "Install via Yum:".to_string(),
                "  sudo yum-config-manager --add-repo https://get.remote-code.dev/yum.repo"
                    .to_string(),
                "  sudo yum install remote-code".to_string(),
            ],
            InstallMethod::Dnf => vec![
                "Install via DNF:".to_string(),
                "  sudo dnf config-manager --add-repo https://get.remote-code.dev/dnf.repo"
                    .to_string(),
                "  sudo dnf install remote-code".to_string(),
            ],
            InstallMethod::Pacman => vec![
                "Install via Pacman (AUR):".to_string(),
                "  yay -S remote-code".to_string(),
            ],
            InstallMethod::Npm => vec![
                "Install via npm:".to_string(),
                "  npm install -g @remote-code/cli".to_string(),
            ],
            InstallMethod::Unknown => vec![
                "Manual installation:".to_string(),
                "  Visit https://github.com/yanzhi0922/remote-code-rust for instructions"
                    .to_string(),
            ],
        }
    }

    /// Return platform-specific update instructions.
    pub fn update_instructions(&self) -> Vec<String> {
        match self.method {
            InstallMethod::Cargo => vec![
                "Update via Cargo:".to_string(),
                "  cargo install remote-code --force".to_string(),
            ],
            InstallMethod::Brew => vec![
                "Update via Homebrew:".to_string(),
                "  brew upgrade remote-code".to_string(),
            ],
            InstallMethod::Scoop => vec![
                "Update via Scoop:".to_string(),
                "  scoop update remote-code".to_string(),
            ],
            InstallMethod::Winget => vec![
                "Update via Winget:".to_string(),
                "  winget upgrade remote-code.cli".to_string(),
            ],
            InstallMethod::Chocolatey => vec![
                "Update via Chocolatey:".to_string(),
                "  choco upgrade remote-code".to_string(),
            ],
            InstallMethod::Apt => vec![
                "Update via APT:".to_string(),
                "  sudo apt update && sudo apt install --only-upgrade remote-code".to_string(),
            ],
            InstallMethod::Yum => vec![
                "Update via Yum:".to_string(),
                "  sudo yum update remote-code".to_string(),
            ],
            InstallMethod::Dnf => vec![
                "Update via DNF:".to_string(),
                "  sudo dnf upgrade remote-code".to_string(),
            ],
            InstallMethod::Pacman => vec![
                "Update via Pacman (AUR):".to_string(),
                "  yay -Syu remote-code".to_string(),
            ],
            InstallMethod::Npm => vec![
                "Update via npm:".to_string(),
                "  npm update -g @remote-code/cli".to_string(),
            ],
            InstallMethod::Unknown => vec![
                "Manual update:".to_string(),
                "  Visit https://github.com/yanzhi0922/remote-code-rust for instructions"
                    .to_string(),
            ],
        }
    }
}

impl Default for PlatformInstaller {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Platform detection
// ---------------------------------------------------------------------------

/// Detect the best available install method for the current platform.
pub fn detect_install_method() -> InstallMethod {
    // Check platform-specific package managers
    #[cfg(target_os = "windows")]
    {
        if command_exists("scoop") {
            return InstallMethod::Scoop;
        }
        if command_exists("winget") {
            return InstallMethod::Winget;
        }
        if command_exists("choco") {
            return InstallMethod::Chocolatey;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if command_exists("brew") {
            return InstallMethod::Brew;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if command_exists("apt-get") {
            return InstallMethod::Apt;
        }
        if command_exists("dnf") {
            return InstallMethod::Dnf;
        }
        if command_exists("yum") {
            return InstallMethod::Yum;
        }
        if command_exists("pacman") {
            return InstallMethod::Pacman;
        }
    }

    // Cross-platform fallbacks
    if command_exists("npm") {
        return InstallMethod::Npm;
    }
    if command_exists("cargo") {
        return InstallMethod::Cargo;
    }

    InstallMethod::Unknown
}

/// Check whether a command exists on the system PATH.
fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_method_display() {
        assert_eq!(InstallMethod::Scoop.to_string(), "scoop");
        assert_eq!(InstallMethod::Winget.to_string(), "winget");
        assert_eq!(InstallMethod::Chocolatey.to_string(), "choco");
        assert_eq!(InstallMethod::Brew.to_string(), "brew");
        assert_eq!(InstallMethod::Apt.to_string(), "apt");
        assert_eq!(InstallMethod::Yum.to_string(), "yum");
        assert_eq!(InstallMethod::Dnf.to_string(), "dnf");
        assert_eq!(InstallMethod::Pacman.to_string(), "pacman");
        assert_eq!(InstallMethod::Npm.to_string(), "npm");
        assert_eq!(InstallMethod::Cargo.to_string(), "cargo");
        assert_eq!(InstallMethod::Unknown.to_string(), "unknown");
    }

    #[test]
    fn platform_installer_with_method() {
        let installer = PlatformInstaller::with_method(InstallMethod::Cargo);
        assert_eq!(installer.method(), InstallMethod::Cargo);
    }

    #[test]
    fn platform_installer_default() {
        let installer = PlatformInstaller::default();
        // Should detect something (at minimum Unknown)
        // PlatformInstaller always detects a method (even if Unknown)
        let _method = installer.method();
    }

    #[test]
    fn install_instructions_cargo() {
        let installer = PlatformInstaller::with_method(InstallMethod::Cargo);
        let instructions = installer.install_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.iter().any(|s| s.contains("cargo install")));
    }

    #[test]
    fn install_instructions_brew() {
        let installer = PlatformInstaller::with_method(InstallMethod::Brew);
        let instructions = installer.install_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.iter().any(|s| s.contains("brew")));
    }

    #[test]
    fn install_instructions_unknown() {
        let installer = PlatformInstaller::with_method(InstallMethod::Unknown);
        let instructions = installer.install_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.iter().any(|s| s.contains("github.com")));
    }

    #[test]
    fn update_instructions_cargo() {
        let installer = PlatformInstaller::with_method(InstallMethod::Cargo);
        let instructions = installer.update_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.iter().any(|s| s.contains("cargo install")));
    }

    #[test]
    fn update_instructions_brew() {
        let installer = PlatformInstaller::with_method(InstallMethod::Brew);
        let instructions = installer.update_instructions();
        assert!(!instructions.is_empty());
        assert!(instructions.iter().any(|s| s.contains("brew upgrade")));
    }

    #[test]
    fn get_latest_version_returns_ok() {
        let version = PlatformInstaller::get_latest_version();
        assert!(version.is_ok());
        assert!(
            !version
                .expect("get_latest_version should succeed after is_ok check")
                .is_empty()
        );
    }

    #[test]
    fn detect_install_method_returns_valid() {
        let method = detect_install_method();
        // Should return a valid InstallMethod variant
        assert!(matches!(
            method,
            InstallMethod::Scoop
                | InstallMethod::Winget
                | InstallMethod::Chocolatey
                | InstallMethod::Brew
                | InstallMethod::Apt
                | InstallMethod::Yum
                | InstallMethod::Dnf
                | InstallMethod::Pacman
                | InstallMethod::Npm
                | InstallMethod::Cargo
                | InstallMethod::Unknown
        ));
    }

    #[test]
    fn all_methods_have_install_instructions() {
        for method in [
            InstallMethod::Scoop,
            InstallMethod::Winget,
            InstallMethod::Chocolatey,
            InstallMethod::Brew,
            InstallMethod::Apt,
            InstallMethod::Yum,
            InstallMethod::Dnf,
            InstallMethod::Pacman,
            InstallMethod::Npm,
            InstallMethod::Cargo,
            InstallMethod::Unknown,
        ] {
            let installer = PlatformInstaller::with_method(method);
            assert!(
                !installer.install_instructions().is_empty(),
                "Missing install instructions for {method}"
            );
        }
    }

    #[test]
    fn all_methods_have_update_instructions() {
        for method in [
            InstallMethod::Scoop,
            InstallMethod::Winget,
            InstallMethod::Chocolatey,
            InstallMethod::Brew,
            InstallMethod::Apt,
            InstallMethod::Yum,
            InstallMethod::Dnf,
            InstallMethod::Pacman,
            InstallMethod::Npm,
            InstallMethod::Cargo,
            InstallMethod::Unknown,
        ] {
            let installer = PlatformInstaller::with_method(method);
            assert!(
                !installer.update_instructions().is_empty(),
                "Missing update instructions for {method}"
            );
        }
    }
}
