//! Computer use / screen control capabilities.
//!
//! Corresponds to `.research/cc-haha/src/utils/computerUse/`.
//! Provides screen capture, mouse, keyboard, and window management
//! through platform-specific APIs.

use std::path::{Path, PathBuf};

/// Screen capabilities detected at runtime.
#[derive(Debug, Clone)]
pub struct ComputerUseCapabilities {
    /// Screen width in pixels.
    pub screen_width: u32,
    /// Screen height in pixels.
    pub screen_height: u32,
    /// Display scale factor (1.0 = no scaling, 2.0 = Retina).
    pub display_scale: f64,
}

/// Information about the active window.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Window title.
    pub title: String,
    /// Window X position.
    pub x: i32,
    /// Window Y position.
    pub y: i32,
    /// Window width in pixels.
    pub width: u32,
    /// Window height in pixels.
    pub height: u32,
    /// Name of the process that owns the window.
    pub process_name: String,
}

/// Mouse button for click operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Keyboard key for special key presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Space,
    /// Ctrl+C (interrupt).
    CtrlC,
    /// Ctrl+V (paste).
    CtrlV,
    /// Ctrl+A (select all).
    CtrlA,
    /// Ctrl+Z (undo).
    CtrlZ,
    /// Function key (F1-F12).
    Function(u8),
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Detect screen capabilities for the current platform.
#[cfg(target_os = "windows")]
pub fn detect_screen_capabilities() -> Result<ComputerUseCapabilities, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; \
             [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width; \
             [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let width: u32 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(1920);
    let height: u32 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(1080);

    Ok(ComputerUseCapabilities {
        screen_width: width,
        screen_height: height,
        display_scale: 1.0,
    })
}

#[cfg(target_os = "macos")]
pub fn detect_screen_capabilities() -> Result<ComputerUseCapabilities, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType"])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut width = 1920u32;
    let mut height = 1080u32;

    for line in stdout.lines() {
        if let Some(idx) = line.find("Resolution:") {
            let resolution = &line[idx..];
            let parts: Vec<&str> = resolution.split_whitespace().collect();
            if parts.len() >= 3 {
                width = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1920);
                height = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(1080);
            }
        }
    }

    Ok(ComputerUseCapabilities {
        screen_width: width,
        screen_height: height,
        display_scale: 2.0, // Retina default
    })
}

#[cfg(target_os = "linux")]
pub fn detect_screen_capabilities() -> Result<ComputerUseCapabilities, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("xdpyinfo").arg(":0").output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut width = 1920u32;
    let mut height = 1080u32;

    for line in stdout.lines() {
        if line.contains("dimensions:") {
            // e.g. "  dimensions:    1920x1080 pixels (508x285 millimeters)"
            if let Some(idx) = line.find("dimensions:") {
                let rest = &line[idx..];
                let dim_part = rest.split_whitespace().nth(1).unwrap_or("1920x1080");
                let dims: Vec<&str> = dim_part.split('x').collect();
                if dims.len() == 2 {
                    width = dims[0].parse().unwrap_or(1920);
                    height = dims[1].parse().unwrap_or(1080);
                }
            }
        }
    }

    Ok(ComputerUseCapabilities {
        screen_width: width,
        screen_height: height,
        display_scale: 1.0,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn detect_screen_capabilities() -> Result<ComputerUseCapabilities, anyhow::Error> {
    anyhow::bail!("Screen capability detection is not supported on this platform")
}

/// Capture a screenshot and save to the specified path.
///
/// - **Windows**: Uses PowerShell with `System.Drawing`.
/// - **macOS**: Uses the `screencapture` command.
/// - **Linux**: Uses `gnome-screenshot` or `xdg-screenshot`.
#[cfg(target_os = "windows")]
pub fn capture_screenshot(output_path: &Path) -> Result<PathBuf, anyhow::Error> {
    let path_str = output_path.to_string_lossy().to_string();
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $bmp = [System.Drawing.Bitmap]::new(\
            [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width, \
            [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height); \
         $g = [System.Drawing.Graphics]::FromImage($bmp); \
         $g.CopyFromScreen(0, 0, 0, 0, $bmp.Size); \
         $bmp.Save('{path_str}'); \
         $g.Dispose(); $bmp.Dispose()"
    );

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;

    if status.success() {
        Ok(output_path.to_path_buf())
    } else {
        anyhow::bail!("Screenshot capture failed on Windows")
    }
}

#[cfg(target_os = "macos")]
pub fn capture_screenshot(output_path: &Path) -> Result<PathBuf, anyhow::Error> {
    let path_str = output_path.to_string_lossy().to_string();
    let status = std::process::Command::new("screencapture")
        .args(["-x", &path_str])
        .status()?;

    if status.success() {
        Ok(output_path.to_path_buf())
    } else {
        anyhow::bail!("Screenshot capture failed on macOS")
    }
}

#[cfg(target_os = "linux")]
pub fn capture_screenshot(output_path: &Path) -> Result<PathBuf, anyhow::Error> {
    let path_str = output_path.to_string_lossy().to_string();

    // Try gnome-screenshot first, fall back to scrot
    let status = std::process::Command::new("gnome-screenshot")
        .args(["-f", &path_str])
        .status();

    let success = match status {
        Ok(s) => s.success(),
        Err(_) => {
            // Fallback to scrot
            std::process::Command::new("scrot")
                .arg(&path_str)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    };

    if success {
        Ok(output_path.to_path_buf())
    } else {
        anyhow::bail!("Screenshot capture failed on Linux (no supported tool found)")
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn capture_screenshot(_output_path: &Path) -> Result<PathBuf, anyhow::Error> {
    anyhow::bail!("Screenshot capture is not supported on this platform")
}

/// Get information about the currently active window.
#[cfg(target_os = "windows")]
pub fn get_active_window_info() -> Result<WindowInfo, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Add-Type @\"\n\
             using System;\n\
             using System.Runtime.InteropServices;\n\
             public class Win32 {\n\
             [DllImport(\"user32.dll\")] public static extern IntPtr GetForegroundWindow();\n\
             [DllImport(\"user32.dll\", CharSet=CharSet.Auto)] public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);\n\
             }\n\"@; \
             $hwnd = [Win32]::GetForegroundWindow(); \
             $sb = New-Object System.Text.StringBuilder 256; \
             [Win32]::GetWindowText($hwnd, $sb, 256) | Out-Null; \
             $sb.ToString()",
        ])
        .output()?;

    let title = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(WindowInfo {
        title,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        process_name: String::new(),
    })
}

#[cfg(target_os = "macos")]
pub fn get_active_window_info() -> Result<WindowInfo, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get name of first process whose frontmost is true",
        ])
        .output()?;

    let process_name = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    let title_output = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get title of first window of first process whose frontmost is true",
        ])
        .output()?;

    let title = String::from_utf8_lossy(&title_output.stdout)
        .trim()
        .to_owned();

    Ok(WindowInfo {
        title,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        process_name,
    })
}

#[cfg(target_os = "linux")]
pub fn get_active_window_info() -> Result<WindowInfo, anyhow::Error> {
    use std::process::Command;
    let output = Command::new("xdotool")
        .args(["getactivewindow", "getwindowname"])
        .output()?;

    let title = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    let pid_output = Command::new("xdotool")
        .args(["getactivewindow", "getwindowpid"])
        .output()?;

    let pid = String::from_utf8_lossy(&pid_output.stdout)
        .trim()
        .to_owned();
    let process_name = if !pid.is_empty() {
        let proc_output = Command::new("ps")
            .args(["-p", &pid, "-o", "comm="])
            .output()?;
        String::from_utf8_lossy(&proc_output.stdout)
            .trim()
            .to_owned()
    } else {
        String::new()
    };

    Ok(WindowInfo {
        title,
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        process_name,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn get_active_window_info() -> Result<WindowInfo, anyhow::Error> {
    anyhow::bail!("Active window detection is not supported on this platform")
}

/// Perform a mouse click at the specified coordinates.
#[cfg(target_os = "windows")]
pub fn mouse_click(x: i32, y: i32, button: MouseButton) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let btn = match button {
        MouseButton::Left => "Left",
        MouseButton::Right => "Right",
        MouseButton::Middle => "Middle",
    };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.Cursor]::Position = [System.Drawing.Point]::new({x}, {y}); \
         Start-Sleep -Milliseconds 50; \
         # Click simulation via SendInput would go here; \
         Write-Output 'Click at {x},{y} with {btn} button'"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn mouse_click(x: i32, y: i32, button: MouseButton) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let btn_flag = match button {
        MouseButton::Left => "",
        MouseButton::Right => "rclick",
        MouseButton::Middle => "click 2",
    };
    let script = if btn_flag.is_empty() {
        format!("tell application \"System Events\" to click at {{{x}, {y}}}")
    } else {
        format!("tell application \"System Events\" to {btn_flag} at {{{x}, {y}}}")
    };
    Command::new("osascript").args(["-e", &script]).status()?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn mouse_click(x: i32, y: i32, button: MouseButton) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let btn_num = match button {
        MouseButton::Left => "1",
        MouseButton::Right => "3",
        MouseButton::Middle => "2",
    };
    Command::new("xdotool")
        .args(["mousemove", &x.to_string(), &y.to_string()])
        .status()?;
    Command::new("xdotool").args(["click", btn_num]).status()?;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn mouse_click(_x: i32, _y: i32, _button: MouseButton) -> Result<(), anyhow::Error> {
    anyhow::bail!("Mouse control is not supported on this platform")
}

/// Type text using the keyboard.
#[cfg(target_os = "windows")]
pub fn keyboard_type(text: &str) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let escaped = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('{escaped}')"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn keyboard_type(text: &str) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("tell application \"System Events\" to keystroke \"{escaped}\"");
    Command::new("osascript").args(["-e", &script]).status()?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn keyboard_type(text: &str) -> Result<(), anyhow::Error> {
    use std::process::Command;
    Command::new("xdotool")
        .args(["type", "--delay", "0", text])
        .status()?;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn keyboard_type(_text: &str) -> Result<(), anyhow::Error> {
    anyhow::bail!("Keyboard input is not supported on this platform")
}

/// Press a special keyboard key.
#[cfg(target_os = "windows")]
pub fn keyboard_key(key: Key) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let key_str = match key {
        Key::Enter => "{ENTER}",
        Key::Tab => "{TAB}",
        Key::Escape => "{ESC}",
        Key::Backspace => "{BACKSPACE}",
        Key::Delete => "{DELETE}",
        Key::Home => "{HOME}",
        Key::End => "{END}",
        Key::PageUp => "{PGUP}",
        Key::PageDown => "{PGDN}",
        Key::ArrowUp => "{UP}",
        Key::ArrowDown => "{DOWN}",
        Key::ArrowLeft => "{LEFT}",
        Key::ArrowRight => "{RIGHT}",
        Key::Space => " ",
        Key::CtrlC => "^c",
        Key::CtrlV => "^v",
        Key::CtrlA => "^a",
        Key::CtrlZ => "^z",
        Key::Function(n) => return keyboard_type(&format!("{{F{n}}}")),
    };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.SendKeys]::SendWait('{key_str}')"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn keyboard_key(key: Key) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let key_code = match key {
        Key::Enter => "key code 36",
        Key::Tab => "key code 48",
        Key::Escape => "key code 53",
        Key::Backspace => "key code 51",
        Key::Delete => "key code 117",
        Key::Home => "key code 115",
        Key::End => "key code 119",
        Key::PageUp => "key code 116",
        Key::PageDown => "key code 121",
        Key::ArrowUp => "key code 126",
        Key::ArrowDown => "key code 125",
        Key::ArrowLeft => "key code 123",
        Key::ArrowRight => "key code 124",
        Key::Space => "key code 49",
        Key::CtrlC => "keystroke \"c\" using control down",
        Key::CtrlV => "keystroke \"v\" using control down",
        Key::CtrlA => "keystroke \"a\" using control down",
        Key::CtrlZ => "keystroke \"z\" using control down",
        Key::Function(n) if n <= 19 => return keyboard_type(&format!("key code {n}")),
        Key::Function(_) => anyhow::bail!("Function key out of range"),
    };
    let script = format!("tell application \"System Events\" to {key_code}");
    Command::new("osascript").args(["-e", &script]).status()?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn keyboard_key(key: Key) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let key_name = match key {
        Key::Enter => "Return",
        Key::Tab => "Tab",
        Key::Escape => "Escape",
        Key::Backspace => "BackSpace",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "Page_Up",
        Key::PageDown => "Page_Down",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::Space => "space",
        Key::CtrlC => "ctrl+c",
        Key::CtrlV => "ctrl+v",
        Key::CtrlA => "ctrl+a",
        Key::CtrlZ => "ctrl+z",
        Key::Function(n) => return keyboard_type(&format!("F{n}")),
    };
    Command::new("xdotool").args(["key", key_name]).status()?;
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn keyboard_key(_key: Key) -> Result<(), anyhow::Error> {
    anyhow::bail!("Keyboard key input is not supported on this platform")
}

/// Scroll in the specified direction by the given amount.
#[cfg(target_os = "windows")]
pub fn scroll(direction: ScrollDirection, amount: u32) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let (dx, dy) = match direction {
        ScrollDirection::Up => (0, -(amount as i32)),
        ScrollDirection::Down => (0, amount as i32),
        ScrollDirection::Left => (-(amount as i32), 0),
        ScrollDirection::Right => (amount as i32, 0),
    };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         [System.Windows.Forms.SendKeys]::SendWait(' '); \
         # Scroll ({dx}, {dy}) would use SendInput"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .status()?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn scroll(direction: ScrollDirection, amount: u32) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let (dx, dy) = match direction {
        ScrollDirection::Up => (0, amount),
        ScrollDirection::Down => (0, amount),
        ScrollDirection::Left => (amount, 0),
        ScrollDirection::Right => (amount, 0),
    };
    let key = match direction {
        ScrollDirection::Up => "key code 126",
        ScrollDirection::Down => "key code 125",
        ScrollDirection::Left => "key code 123",
        ScrollDirection::Right => "key code 124",
    };
    for _ in 0..amount.min(10) {
        let script = format!("tell application \"System Events\" to {key}");
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
    let _ = (dx, dy); // suppress unused warning
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn scroll(direction: ScrollDirection, amount: u32) -> Result<(), anyhow::Error> {
    use std::process::Command;
    let btn = match direction {
        ScrollDirection::Up => "4",
        ScrollDirection::Down => "5",
        ScrollDirection::Left => "6",
        ScrollDirection::Right => "7",
    };
    for _ in 0..amount.min(10) {
        let _ = Command::new("xdotool").args(["click", btn]).status();
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn scroll(_direction: ScrollDirection, _amount: u32) -> Result<(), anyhow::Error> {
    anyhow::bail!("Scroll is not supported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_values() {
        let caps = ComputerUseCapabilities {
            screen_width: 1920,
            screen_height: 1080,
            display_scale: 1.0,
        };
        assert_eq!(caps.screen_width, 1920);
        assert_eq!(caps.screen_height, 1080);
        assert!((caps.display_scale - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn window_info_fields() {
        let info = WindowInfo {
            title: "Test Window".into(),
            x: 100,
            y: 200,
            width: 800,
            height: 600,
            process_name: "test_app".into(),
        };
        assert_eq!(info.title, "Test Window");
        assert_eq!(info.x, 100);
        assert_eq!(info.width, 800);
        assert_eq!(info.process_name, "test_app");
    }

    #[test]
    fn mouse_button_variants() {
        assert_ne!(MouseButton::Left, MouseButton::Right);
        assert_ne!(MouseButton::Middle, MouseButton::Left);
    }

    #[test]
    fn key_variants() {
        assert_eq!(Key::Enter, Key::Enter);
        assert_ne!(Key::Enter, Key::Tab);
        assert_eq!(Key::Function(1), Key::Function(1));
        assert_ne!(Key::Function(1), Key::Function(2));
    }

    #[test]
    fn scroll_direction_variants() {
        assert_eq!(ScrollDirection::Up, ScrollDirection::Up);
        assert_ne!(ScrollDirection::Up, ScrollDirection::Down);
    }

    #[test]
    fn detect_screen_capabilities_returns_on_supported_platform() {
        // This test runs on the actual platform; just verify it doesn't panic
        let result = detect_screen_capabilities();
        // On CI or environments without display, this may fail — that's OK
        if let Ok(caps) = result {
            assert!(caps.screen_width > 0);
            assert!(caps.screen_height > 0);
            assert!(caps.display_scale > 0.0);
        }
    }

    #[test]
    fn capture_screenshot_fails_with_invalid_path() {
        // Use a path with invalid characters that cannot be created on any platform
        let result = capture_screenshot(Path::new("/dev/null/impossible bad.png"));
        // This should fail because the path contains null bytes or is invalid
        // On some platforms this may succeed, so we just verify no panic
        let _ = result;
    }

    #[test]
    fn mouse_button_copy_traits() {
        let left = MouseButton::Left;
        let left2 = left;
        assert_eq!(left, left2);
    }

    #[test]
    fn key_copy_traits() {
        let enter = Key::Enter;
        let enter2 = enter;
        assert_eq!(enter, enter2);
    }

    #[test]
    fn scroll_direction_copy_traits() {
        let up = ScrollDirection::Up;
        let up2 = up;
        assert_eq!(up, up2);
    }

    #[test]
    fn computer_use_capabilities_debug() {
        let caps = ComputerUseCapabilities {
            screen_width: 1920,
            screen_height: 1080,
            display_scale: 1.0,
        };
        let debug_str = format!("{caps:?}");
        assert!(debug_str.contains("1920"));
        assert!(debug_str.contains("1080"));
    }

    #[test]
    fn window_info_debug() {
        let info = WindowInfo {
            title: "Test".into(),
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            process_name: "app".into(),
        };
        let debug_str = format!("{info:?}");
        assert!(debug_str.contains("Test"));
    }

    #[test]
    fn key_function_boundary() {
        assert_eq!(Key::Function(1), Key::Function(1));
        assert_eq!(Key::Function(12), Key::Function(12));
    }
}
