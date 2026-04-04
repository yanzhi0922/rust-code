use anyhow::{anyhow, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const RECORDING_SAMPLE_RATE: u32 = 16000;
const RECORDING_CHANNELS: u8 = 1;
const SILENCE_DURATION_SECS: &str = "2.0";
const SILENCE_THRESHOLD: &str = "3%";

#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub silence_detection: bool,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            sample_rate: RECORDING_SAMPLE_RATE,
            channels: RECORDING_CHANNELS,
            silence_detection: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RecordingAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

pub struct VoiceService {
    config: VoiceConfig,
    active_recorder: Arc<Mutex<Option<Child>>>,
    native_recording_active: Arc<AtomicBool>,
}

impl VoiceService {
    pub fn new() -> Self {
        Self {
            config: VoiceConfig::default(),
            active_recorder: Arc::new(Mutex::new(None)),
            native_recording_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_config(config: VoiceConfig) -> Self {
        Self {
            config,
            active_recorder: Arc::new(Mutex::new(None)),
            native_recording_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn check_recording_availability(&self) -> RecordingAvailability {
        if self.is_remote_environment() {
            return RecordingAvailability {
                available: false,
                reason: Some(
                    "Voice mode requires microphone access, but no audio device is available in this environment.\n\nTo use voice mode, run Claude Code locally instead.".to_string()
                ),
            };
        }

        if cfg!(target_os = "windows") {
            return RecordingAvailability {
                available: false,
                reason: Some(
                    "Voice recording requires the native audio module, which could not be loaded.".to_string()
                ),
            };
        }

        if cfg!(target_os = "linux") && self.has_arecord().await {
            if self.probe_arecord().await {
                return RecordingAvailability {
                    available: true,
                    reason: None,
                };
            }
            
            if self.is_wsl() {
                return RecordingAvailability {
                    available: false,
                    reason: Some(
                        "Voice mode could not access an audio device in WSL.\n\nWSL2 with WSLg (Windows 11) provides audio via PulseAudio — if you are on Windows 10 or WSL1, run Claude Code in native Windows instead.".to_string()
                    ),
                };
            }
        }

        if !self.has_rec().await {
            if self.is_wsl() {
                return RecordingAvailability {
                    available: false,
                    reason: Some(
                        "Voice mode could not access an audio device in WSL.\n\nWSL2 with WSLg (Windows 11) provides audio via PulseAudio — if you are on Windows 10 or WSL1, run Claude Code in native Windows instead.".to_string()
                    ),
                };
            }

            let pm = self.detect_package_manager();
            return RecordingAvailability {
                available: false,
                reason: pm.map(|p| format!("Voice mode requires SoX for audio recording. Install it with: {}", p))
                    .or_else(|| Some("Voice mode requires SoX for audio recording. Install SoX manually:\n  macOS: brew install sox\n  Ubuntu/Debian: sudo apt-get install sox\n  Fedora: sudo dnf install sox".to_string())),
            };
        }

        RecordingAvailability {
            available: true,
            reason: None,
        }
    }

    fn is_remote_environment(&self) -> bool {
        std::env::var("CLAUDE_CODE_REMOTE").is_ok()
            || std::env::var("HOME").ok().as_deref() == Some("/home/claude")
    }

    fn is_wsl(&self) -> bool {
        if cfg!(target_os = "linux") {
            if let Ok(contents) = std::fs::read_to_string("/proc/version") {
                return contents.to_lowercase().contains("microsoft");
            }
        }
        false
    }

    async fn has_command(&self, cmd: &str) -> bool {
        #[cfg(target_os = "windows")]
        {
            Command::new("where")
                .arg(cmd)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            Command::new("which")
                .arg(cmd)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    async fn has_arecord(&self) -> bool {
        self.has_command("arecord").await
    }

    async fn has_rec(&self) -> bool {
        self.has_command("rec").await
    }

    async fn probe_arecord(&self) -> bool {
        let result = Command::new("arecord")
            .args([
                "-f", "S16_LE",
                "-r", &self.config.sample_rate.to_string(),
                "-c", &self.config.channels.to_string(),
                "-t", "raw",
                "/dev/null",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        match result {
            Ok(status) => status.success(),
            Err(_) => false,
        }
    }

    fn detect_package_manager(&self) -> Option<String> {
        #[cfg(target_os = "macos")]
        {
            if which::which("brew").is_ok() {
                return Some("brew install sox".to_string());
            }
        }

        #[cfg(target_os = "linux")]
        {
            if which::which("apt-get").is_ok() {
                return Some("sudo apt-get install sox".to_string());
            }
            if which::which("dnf").is_ok() {
                return Some("sudo dnf install sox".to_string());
            }
            if which::which("pacman").is_ok() {
                return Some("sudo pacman -S sox".to_string());
            }
        }

        None
    }

    pub async fn start_recording<F>(
        &self,
        on_data: F,
        on_end: impl FnMut() + Send + 'static,
    ) -> Result<bool>
    where
        F: FnMut(Vec<u8>) + Send + 'static,
    {
        if cfg!(target_os = "windows") {
            return Err(anyhow!("Windows native recording not yet implemented"));
        }

        if cfg!(target_os = "linux") && self.has_arecord().await && self.probe_arecord().await {
            return self.start_arecord_recording(on_data, on_end).await;
        }

        self.start_sox_recording(on_data, on_end).await
    }

    async fn start_sox_recording<F>(
        &self,
        mut on_data: F,
        mut on_end: impl FnMut() + Send + 'static,
    ) -> Result<bool>
    where
        F: FnMut(Vec<u8>) + Send + 'static,
    {
        let mut args = vec![
            "-q".to_string(),
            "--buffer".to_string(),
            "1024".to_string(),
            "-t".to_string(),
            "raw".to_string(),
            "-r".to_string(),
            self.config.sample_rate.to_string(),
            "-e".to_string(),
            "signed".to_string(),
            "-b".to_string(),
            "16".to_string(),
            "-c".to_string(),
            self.config.channels.to_string(),
            "-".to_string(),
        ];

        if self.config.silence_detection {
            args.extend([
                "silence".to_string(),
                "1".to_string(),
                "0.1".to_string(),
                SILENCE_THRESHOLD.to_string(),
                "1".to_string(),
                SILENCE_DURATION_SECS.to_string(),
                SILENCE_THRESHOLD.to_string(),
            ]);
        }

        let mut child = Command::new("rec")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buffer = [0u8; 1024];

        *self.active_recorder.lock().await = Some(child);

        let active_recorder = self.active_recorder.clone();
        let native_active = self.native_recording_active.clone();

        tokio::spawn(async move {
            native_active.store(true, Ordering::SeqCst);

            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        on_data(buffer[..n].to_vec());
                    }
                }
            }

            native_active.store(false, Ordering::SeqCst);
            *active_recorder.lock().await = None;
            on_end();
        });

        Ok(true)
    }

    async fn start_arecord_recording<F>(
        &self,
        mut on_data: F,
        mut on_end: impl FnMut() + Send + 'static,
    ) -> Result<bool>
    where
        F: FnMut(Vec<u8>) + Send + 'static,
    {
        let args = [
            "-f", "S16_LE",
            "-r", &self.config.sample_rate.to_string(),
            "-c", &self.config.channels.to_string(),
            "-t", "raw",
            "-q",
            "-",
        ];

        let mut child = Command::new("arecord")
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut buffer = [0u8; 1024];

        *self.active_recorder.lock().await = Some(child);

        let active_recorder = self.active_recorder.clone();
        let native_active = self.native_recording_active.clone();

        tokio::spawn(async move {
            native_active.store(true, Ordering::SeqCst);

            loop {
                match reader.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        on_data(buffer[..n].to_vec());
                    }
                }
            }

            native_active.store(false, Ordering::SeqCst);
            *active_recorder.lock().await = None;
            on_end();
        });

        Ok(true)
    }

    pub async fn stop_recording(&self) {
        self.native_recording_active.store(false, Ordering::SeqCst);

        let mut recorder = self.active_recorder.lock().await;
        if let Some(mut child) = recorder.take() {
            let _ = child.kill().await;
        }
    }

    pub fn is_recording(&self) -> bool {
        self.native_recording_active.load(Ordering::SeqCst)
    }
}

impl Default for VoiceService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_config_default() {
        let config = VoiceConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert!(config.silence_detection);
    }

    #[test]
    fn test_voice_service_creation() {
        let service = VoiceService::new();
        assert!(!service.is_recording());
    }

    #[tokio::test]
    async fn test_remote_environment_detection() {
        std::env::set_var("CLAUDE_CODE_REMOTE", "1");
        let service = VoiceService::new();
        let availability = service.check_recording_availability().await;
        assert!(!availability.available);
        std::env::remove_var("CLAUDE_CODE_REMOTE");
    }
}
