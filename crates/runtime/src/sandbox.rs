pub fn is_sandboxed() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/.dockerenv").exists()
            || std::fs::read_to_string("/proc/1/cgroup")
                .map(|c| c.contains("docker") || c.contains("containerd"))
                .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

pub fn build_sandbox_command(command: &str) -> String {
    if is_sandboxed() {
        command.to_string()
    } else {
        command.to_string()
    }
}
