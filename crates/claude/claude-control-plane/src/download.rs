//! Authenticated download page for mobile app binaries.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

use crate::state::ControlPlaneService;

/// Serve the download page listing all available files.
pub(crate) async fn download_page(State(service): State<ControlPlaneService>) -> impl IntoResponse {
    let Some(dir) = &service.downloads_dir else {
        return Html(render_empty_page("Downloads not configured."));
    };

    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Html(render_empty_page("No downloads available yet.")),
    };

    let mut files: Vec<FileEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let icon = match name.rsplit('.').next() {
            Some("apk") => "📱",
            Some("ipa") => "🍎",
            Some("dmg") | Some("app") => "💻",
            Some("exe") | Some("msi") => "🖥️",
            _ => "📦",
        };
        files.push(FileEntry { name, size, icon });
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    Html(render_download_page(&files))
}

/// Serve a single file from the downloads directory.
pub(crate) async fn download_file(
    State(service): State<ControlPlaneService>,
    Path(filename): Path<String>,
) -> Response {
    let dir = match &service.downloads_dir {
        Some(d) => d,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    // Prevent path traversal.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let path = dir.join(&filename);
    if !path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mime = guess_mime(&filename);
    let disposition = format!("attachment; filename=\"{}\"", sanitize_filename(&filename));

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

struct FileEntry {
    name: String,
    size: u64,
    icon: &'static str,
}

fn guess_mime(filename: &str) -> &'static str {
    match filename.rsplit('.').next() {
        Some("apk") => "application/vnd.android.package-archive",
        Some("ipa") => "application/octet-stream",
        Some("dmg") => "application/x-apple-diskimage",
        Some("exe") => "application/x-msdownload",
        Some("msi") => "application/x-msi",
        Some("zip") => "application/zip",
        Some("tar" | "gz" | "tgz") => "application/gzip",
        _ => "application/octet-stream",
    }
}

fn sanitize_filename(name: &str) -> String {
    name.replace('"', "\\\"")
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn render_empty_page(msg: &str) -> String {
    render_page_body(&format!(r#"<div class="empty"><p>{msg}</p></div>"#))
}

fn render_download_page(files: &[FileEntry]) -> String {
    if files.is_empty() {
        return render_empty_page("No files available for download.");
    }

    let rows: String = files
        .iter()
        .map(|f| {
            let size = human_size(f.size);
            format!(
                r#"<a href="/downloads/{name}" class="file-row">
                    <span class="icon">{icon}</span>
                    <span class="info">
                        <span class="name">{name}</span>
                        <span class="size">{size}</span>
                    </span>
                    <span class="dl-arrow">⬇</span>
                </a>"#,
                name = html_escape(&f.name),
                icon = f.icon,
                size = size,
            )
        })
        .collect();

    render_page_body(&format!(r#"<div class="files">{rows}</div>"#))
}

fn render_page_body(body: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Remote Code — Download</title>
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: #0f0f13; color: #e4e4e7;
    min-height: 100vh; display: flex; align-items: center; justify-content: center;
}}
.container {{
    max-width: 480px; width: 100%; padding: 32px 20px;
}}
h1 {{
    font-size: 1.5rem; font-weight: 600; text-align: center;
    margin-bottom: 8px; color: #fafafa;
}}
.subtitle {{
    text-align: center; color: #71717a; font-size: 0.875rem;
    margin-bottom: 32px;
}}
.files {{ display: flex; flex-direction: column; gap: 8px; }}
.file-row {{
    display: flex; align-items: center; gap: 14px;
    padding: 16px; border-radius: 12px;
    background: #1c1c24; border: 1px solid #27272a;
    text-decoration: none; color: inherit;
    transition: background 0.15s, border-color 0.15s;
}}
.file-row:hover {{ background: #27272a; border-color: #3f3f46; }}
.icon {{ font-size: 1.75rem; flex-shrink: 0; }}
.info {{ flex: 1; display: flex; flex-direction: column; gap: 2px; }}
.name {{ font-size: 0.9375rem; font-weight: 500; word-break: break-all; }}
.size {{ font-size: 0.8125rem; color: #71717a; }}
.dl-arrow {{ font-size: 1.25rem; color: #a1a1aa; }}
.empty {{ text-align: center; color: #71717a; padding: 48px 0; }}
</style>
</head>
<body>
<div class="container">
    <h1>Remote Code</h1>
    <p class="subtitle">Download the mobile app</p>
    {body}
</div>
</body>
</html>"##
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
