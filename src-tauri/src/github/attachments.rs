//! Images pasted into a PR description or comment.
//!
//! GitHub stores them at `github.com/user-attachments/assets/<id>` (older
//! ones at `github.com/<owner>/<repo>/assets/<user>/<id>`), and for a
//! private repo that address serves a sign-in page to anything without a
//! browser session — a PAT in the Authorization header included, so the
//! webview's `<img>` shows a broken icon. What does work: GitHub's markdown
//! renderer, asked to render the link with the repo as context, returns a
//! signed `private-user-images` URL that anyone can fetch for five minutes.
//! The bytes behind it never change, so they're cached on disk by attachment
//! id and handed to the page as a data URL — one render call per image, ever.

use std::path::PathBuf;

use base64::Engine;
use tauri::{AppHandle, Manager};

use crate::error::{AppError, AppResult};
use crate::secrets;

/// The attachment id: the last path segment, a UUID in either URL shape.
fn attachment_id(url: &str) -> Option<&str> {
    let id = url.split('?').next()?.trim_end_matches('/').rsplit('/').next()?;
    let ok = id.len() == 36
        && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    ok.then_some(id)
}

/// The REST `/markdown` endpoint beside the configured GraphQL one:
/// `https://api.github.com/graphql` → `https://api.github.com/markdown`,
/// GHE's `https://host/api/graphql` → `https://host/api/v3/markdown`.
fn markdown_endpoint(graphql_url: &str) -> String {
    match graphql_url.strip_suffix("/graphql") {
        Some(base) if base.ends_with("/api") => format!("{base}/v3/markdown"),
        Some(base) => format!("{base}/markdown"),
        None => "https://api.github.com/markdown".into(),
    }
}

fn cache_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Other(e.to_string()))?
        .join("attachments");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn cached(dir: &PathBuf, id: &str) -> Option<(String, Vec<u8>)> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if let Some(ext) = name.strip_prefix(id).and_then(|r| r.strip_prefix('.')) {
            let bytes = std::fs::read(e.path()).ok()?;
            return Some((mime_of(ext).to_string(), bytes));
        }
    }
    None
}

fn mime_of(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

fn ext_of(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        _ => "bin",
    }
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The signed URL for an attachment, via the markdown renderer.
async fn sign(http: &reqwest::Client, endpoint: &str, token: &str, repo: &str, url: &str, id: &str) -> AppResult<String> {
    let html: String = http
        .post(endpoint)
        .bearer_auth(token)
        .json(&serde_json::json!({ "text": format!("![]({url})"), "mode": "gfm", "context": repo }))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    // The one `src="…"` that names this attachment; entities decoded, since
    // the signed query string is `&amp;`-joined in HTML.
    html.split("src=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .find(|src| src.contains(id) && !src.starts_with("https://github.com/"))
        .map(|src| src.replace("&amp;", "&"))
        .ok_or_else(|| AppError::Other("GitHub did not sign the attachment".into()))
}

/// A private attachment as a data URL, from the disk cache or fetched once.
#[tauri::command]
pub async fn github_attachment(app: AppHandle, repo: String, url: String) -> AppResult<String> {
    let id = attachment_id(&url)
        .ok_or_else(|| AppError::Other("not a GitHub attachment URL".into()))?
        .to_string();
    let dir = cache_dir(&app)?;
    if let Some((mime, bytes)) = cached(&dir, &id) {
        return Ok(data_url(&mime, &bytes));
    }
    let token = secrets::github_pat()?
        .ok_or_else(|| AppError::Other("no GitHub token configured".into()))?;
    let settings = app.state::<crate::orgs::Orgs>().active().settings()?;
    let http = reqwest::Client::builder()
        .user_agent("cora-pr-review")
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let signed = sign(&http, &markdown_endpoint(&settings.github_graphql_url), &token, &repo, &url, &id).await?;
    let resp = http.get(&signed).send().await?.error_for_status()?;
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_string();
    let bytes = resp.bytes().await?.to_vec();
    // Written whole-or-not: a partial file would be served forever.
    let path = dir.join(format!("{id}.{}", ext_of(&mime)));
    let tmp = dir.join(format!("{id}.part"));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(data_url(&mime, &bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_from_either_url_shape() {
        assert_eq!(
            attachment_id("https://github.com/user-attachments/assets/0b044607-1307-48ad-8c1f-7e17be01d2c2"),
            Some("0b044607-1307-48ad-8c1f-7e17be01d2c2")
        );
        assert_eq!(
            attachment_id("https://github.com/o/r/assets/12345/0b044607-1307-48ad-8c1f-7e17be01d2c2"),
            Some("0b044607-1307-48ad-8c1f-7e17be01d2c2")
        );
        assert_eq!(attachment_id("https://github.com/o/r/pull/1"), None);
    }

    #[test]
    fn markdown_endpoint_sits_beside_graphql() {
        assert_eq!(markdown_endpoint("https://api.github.com/graphql"), "https://api.github.com/markdown");
        assert_eq!(markdown_endpoint("https://ghe.example.com/api/graphql"), "https://ghe.example.com/api/v3/markdown");
    }
}
