use anyhow::Result;
use rustyline::error::ReadlineError;
use std::path::PathBuf;

pub struct InputHandler {
    rl: rustyline::DefaultEditor,
}

impl InputHandler {
    pub fn new() -> Result<Self> {
        let history_path = Self::history_path();

        let mut rl = rustyline::DefaultEditor::new()?;
        if history_path.exists() {
            let _ = rl.load_history(&history_path);
        }

        Ok(Self { rl })
    }

    pub fn read_line(&mut self) -> Result<Option<String>> {
        match self.rl.readline("> ") {
            Ok(line) => {
                let line = line.trim().to_string();
                if !line.is_empty() {
                    self.rl.add_history_entry(&line)?;
                }
                Ok(Some(line))
            }
            Err(ReadlineError::Eof) => Ok(None),
            Err(ReadlineError::Interrupted) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("Readline error: {e}")),
        }
    }

    pub fn save_history(&mut self) -> Result<()> {
        let history_path = Self::history_path();
        if let Some(parent) = history_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.rl.save_history(&history_path)?;
        Ok(())
    }

    fn history_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("claude").join("history.txt")
    }
}
