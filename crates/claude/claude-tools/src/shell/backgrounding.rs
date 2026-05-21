use super::readonly::ShellKind;

#[must_use]
pub fn requested_background(input_background: bool, kind: ShellKind, command: &str) -> bool {
    if input_background {
        return true;
    }
    let normalized = command.trim().to_ascii_lowercase();
    match kind {
        ShellKind::Bash => normalized.ends_with('&') || normalized.contains(" nohup "),
        ShellKind::PowerShell => {
            normalized.contains("start-job")
                || normalized.contains("start-process")
                || normalized.contains("start-threadjob")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::requested_background;
    use crate::shell::readonly::ShellKind;

    #[test]
    fn background_requests_are_detected() {
        assert!(requested_background(false, ShellKind::Bash, "npm start &"));
        assert!(requested_background(
            false,
            ShellKind::PowerShell,
            "Start-Job { Get-Process }"
        ));
        assert!(requested_background(true, ShellKind::Bash, "cargo test"));
        assert!(!requested_background(false, ShellKind::Bash, "cargo test"));
    }
}
