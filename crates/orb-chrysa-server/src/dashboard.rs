use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "dashboard/dist/"]
struct DashboardAssets;

#[derive(RustEmbed)]
#[folder = "dashboard/static/"]
struct StaticAssets;

/// Serve the dashboard SPA. Exact paths (/, /assets/*.js, /assets/*.css,
/// /brand/*.svg, /favicon.svg) are served explicitly. Unknown paths return a
/// 404 page.
pub fn dashboard_router() -> axum::Router {
    axum::Router::new()
        .route("/", axum::routing::get(serve_index))
        .route("/assets/{*path}", axum::routing::get(serve_asset))
        .route("/brand/{*path}", axum::routing::get(serve_asset))
        .route("/favicon.svg", axum::routing::get(serve_asset))
        .fallback(serve_not_found)
}

async fn serve_index() -> Response {
    match DashboardAssets::get("index.html") {
        Some(file) => {
            let mime = mime_for_path("index.html");
            let mut response = ([(header::CONTENT_TYPE, mime)], file.data).into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            response
        }
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "dashboard not built",
        )
            .into_response(),
    }
}

async fn serve_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    match DashboardAssets::get(path) {
        Some(file) => {
            let mime = mime_for_path(path);
            let mut response = ([(header::CONTENT_TYPE, mime)], file.data).into_response();
            if is_hashed_asset(path) {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=31536000, immutable"),
                );
            }
            response
        }
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "asset not found",
        )
            .into_response(),
    }
}

pub async fn serve_not_found() -> Response {
    match StaticAssets::get("404.html") {
        Some(file) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "404 — page not found",
        )
            .into_response(),
    }
}

fn mime_for_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn is_hashed_asset(path: &str) -> bool {
    // Vite hashed assets look like: assets/index-abc123.js, assets/vendor-def456.css
    let filename = path.rsplit('/').next().unwrap_or(path);
    filename.contains('-')
        && filename
            .rsplit('.')
            .next()
            .map(|ext| matches!(ext, "js" | "css" | "mjs" | "woff2"))
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_for_js_modules() {
        assert_eq!(
            mime_for_path("app.js"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for_path("vendor.mjs"),
            "application/javascript; charset=utf-8"
        );
        assert_eq!(
            mime_for_path("assets/index-abc123.js"),
            "application/javascript; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_styles() {
        assert_eq!(mime_for_path("app.css"), "text/css; charset=utf-8");
        assert_eq!(
            mime_for_path("assets/main-def456.css"),
            "text/css; charset=utf-8"
        );
    }

    #[test]
    fn mime_for_html() {
        assert_eq!(mime_for_path("index.html"), "text/html; charset=utf-8");
    }

    #[test]
    fn mime_for_images_and_fonts() {
        assert_eq!(mime_for_path("favicon.svg"), "image/svg+xml");
        assert_eq!(mime_for_path("logo.png"), "image/png");
        assert_eq!(mime_for_path("icon.ico"), "image/x-icon");
        assert_eq!(mime_for_path("font.woff2"), "font/woff2");
    }

    #[test]
    fn mime_for_wasm() {
        assert_eq!(mime_for_path("module.wasm"), "application/wasm");
    }

    #[test]
    fn mime_for_json() {
        assert_eq!(
            mime_for_path("data.json"),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn mime_fallback_unknown() {
        assert_eq!(mime_for_path("file.bin"), "application/octet-stream");
        assert_eq!(mime_for_path("noext"), "application/octet-stream");
        assert_eq!(mime_for_path("data.xml"), "application/octet-stream");
    }

    #[test]
    fn hashed_asset_detects_vite_fingerprint() {
        assert!(is_hashed_asset("assets/index-BI-1dg8L.css"));
        assert!(is_hashed_asset("assets/EmptyState-9Ay8abur.js"));
        assert!(is_hashed_asset("assets/vendor-def456.mjs"));
        assert!(is_hashed_asset("assets/font-abc123.woff2"));
        assert!(is_hashed_asset("nested/path/chunk-XYZ789.js"));
    }

    #[test]
    fn hashed_asset_rejects_unhashed() {
        assert!(!is_hashed_asset("favicon.svg"));
        assert!(!is_hashed_asset("assets/index.js"));
        assert!(!is_hashed_asset("style.css"));
        assert!(!is_hashed_asset("data.json"));
    }

    #[test]
    fn hashed_asset_requires_asset_extension() {
        // Contains '-' but .json is not in the allowlist
        assert!(!is_hashed_asset("assets/data-foo.json"));
        // Contains '-' but .png is not in the allowlist
        assert!(!is_hashed_asset("assets/logo-abc123.png"));
        // Contains '-' but .xml is not in the allowlist
        assert!(!is_hashed_asset("assets/config-v1.xml"));
    }

    #[test]
    fn hashed_asset_rejects_no_extension() {
        assert!(!is_hashed_asset("assets/hash-abc123"));
        assert!(!is_hashed_asset("just-a-file-with-dashes"));
    }
}
