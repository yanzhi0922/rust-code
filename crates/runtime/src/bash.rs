use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub async fn execute_bash(cwd: &Path, command: &str, timeout_ms: u64) -> anyhow::Result<BashResult> {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };

    let child = Command::new(shell)
        .args([flag, command])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let timeout = tokio::time::Duration::from_millis(timeout_ms);

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);
            Ok(BashResult {
                stdout,
                stderr,
                exit_code,
            })
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("Failed to execute command: {e}")),
        Err(_) => Err(anyhow::anyhow!(
            "Command timed out after {}ms",
            timeout_ms
        )),
    }
}

pub async fn execute_bash_stream(
    cwd: &Path,
    command: &str,
    timeout_ms: u64,
    mut on_output: impl FnMut(String) + Send,
) -> anyhow::Result<BashResult> {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let flag = if cfg!(windows) { "/C" } else { "-c" };

    let mut child = Command::new(shell)
        .args([flag, command])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();

    if let Some(mut stdout) = child.stdout.take() {
        let mut buf = vec![0u8; 4096];
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 { break; }
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            on_output(text.clone());
            stdout_buf.push_str(&text);
        }
    }

    if let Some(mut stderr) = child.stderr.take() {
        let mut buf = vec![0u8; 4096];
        loop {
            let n = stderr.read(&mut buf).await?;
            if n == 0 { break; }
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            on_output(text.clone());
            stderr_buf.push_str(&text);
        }
    }

    let timeout = tokio::time::Duration::from_millis(timeout_ms);
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(anyhow::anyhow!("Failed to wait for command: {e}")),
        Err(_) => {
            let _ = child.start_kill();
            return Err(anyhow::anyhow!("Command timed out after {}ms", timeout_ms));
        }
    };

    Ok(BashResult {
        stdout: stdout_buf,
        stderr: stderr_buf,
        exit_code: status.code().unwrap_or(-1),
    })
}
