//! The Web UI, built from `web/` and embedded in the binary (D-18).
//!
//! A build without `web/dist` still compiles, so the Rust checks need no Node; the service then
//! answers every page with a note on how to build the UI.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct Build;

/// The page every route of the single-page app loads.
const INDEX: &str = "index.html";
/// Vite puts its hashed output here, so these files never change under their names.
const HASHED: &str = "assets/";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";
const NOT_BUILT: &str = "The Web UI is not built. Run `npm ci && npm run build` in web/.\n";

/// One file of the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    pub(crate) body: Cow<'static, [u8]>,
    pub(crate) mime: String,
}

fn embedded(name: &str) -> Option<Asset> {
    Build::get(name).map(|file| Asset {
        mime: file.metadata.mimetype().to_string(),
        body: file.data,
    })
}

/// Answers a path that is not under `/api` from the embedded build.
pub(crate) fn respond(path: &str) -> Response {
    respond_with(path, embedded)
}

/// Serves the file a path names, or `index.html` for a route of the app. A missing hashed file
/// is not found, so a stale page fails plainly instead of loading HTML as a script.
pub(crate) fn respond_with(path: &str, lookup: impl Fn(&str) -> Option<Asset>) -> Response {
    let name = path.trim_start_matches('/');
    let is_safe = !name.split('/').any(|segment| segment == "..");
    if is_safe
        && !name.is_empty()
        && let Some(asset) = lookup(name)
    {
        let cache = if name.starts_with(HASHED) {
            IMMUTABLE
        } else {
            REVALIDATE
        };
        return file(asset, cache);
    }
    if name.starts_with(HASHED) || !is_safe {
        return (StatusCode::NOT_FOUND, "not found\n").into_response();
    }
    match lookup(INDEX) {
        Some(index) => file(index, REVALIDATE),
        None => (StatusCode::SERVICE_UNAVAILABLE, NOT_BUILT).into_response(),
    }
}

fn file(asset: Asset, cache: &'static str) -> Response {
    let headers = [
        (header::CONTENT_TYPE, asset.mime),
        (header::CACHE_CONTROL, cache.to_string()),
    ];
    (headers, asset.body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    const BODY_LIMIT: usize = 1 << 20;

    fn build(name: &str) -> Option<Asset> {
        let (body, mime) = match name {
            "index.html" => ("<!doctype html>", "text/html"),
            "assets/index-abc.js" => ("export {}", "text/javascript"),
            _ => return None,
        };
        Some(Asset {
            body: Cow::Borrowed(body.as_bytes()),
            mime: mime.to_string(),
        })
    }

    fn no_build(_: &str) -> Option<Asset> {
        None
    }

    async fn read(response: Response) -> (StatusCode, String, String, String) {
        let status = response.status();
        let header = |name| {
            response
                .headers()
                .get(name)
                .map(|value| value.to_str().unwrap().to_string())
                .unwrap_or_default()
        };
        let mime = header(header::CONTENT_TYPE);
        let cache = header(header::CACHE_CONTROL);
        let bytes = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
        (
            status,
            mime,
            cache,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
    }

    #[tokio::test]
    async fn the_root_is_the_index() {
        let (status, mime, cache, body) = read(respond_with("/", build)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(mime, "text/html");
        assert_eq!(cache, REVALIDATE);
        assert_eq!(body, "<!doctype html>");
    }

    #[tokio::test]
    async fn a_route_of_the_app_is_the_index() {
        let (status, mime, _, _) = read(respond_with("/changes/7", build)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(mime, "text/html");
    }

    #[tokio::test]
    async fn a_hashed_file_is_kept_for_good() {
        let (status, mime, cache, body) = read(respond_with("/assets/index-abc.js", build)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(mime, "text/javascript");
        assert_eq!(cache, IMMUTABLE);
        assert_eq!(body, "export {}");
    }

    #[tokio::test]
    async fn a_missing_hashed_file_is_not_found() {
        let (status, ..) = read(respond_with("/assets/index-old.js", build)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_path_that_climbs_out_is_not_looked_up() {
        let looked_up = std::cell::Cell::new(false);
        let lookup = |name: &str| {
            looked_up.set(looked_up.get() || name.contains(".."));
            build(name)
        };

        let (status, ..) = read(respond_with("/../Cargo.toml", lookup)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!looked_up.get());
    }

    #[tokio::test]
    async fn without_a_build_it_says_how_to_make_one() {
        let (status, _, _, body) = read(respond_with("/streams", no_build)).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, NOT_BUILT);
    }
}
