//! Tauri v2 auto-updater endpoint and shared GitHub desktop release cache.
//!
//! Serves `https://modelrelay.io/updater/desktop/{target}/{arch}/{current_version}`
//! and shares a single 5-minute GitHub release cache with
//! `/download/desktop/{platform}`. Supports optional `GITHUB_TOKEN`, requests
//! `per_page=100`, skips drafts/prereleases, and falls back to stale cache data
//! when GitHub is rate-limited or transiently unavailable.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::header};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::state::CloudState;

/// GitHub releases API URL used to look up desktop release assets.
const RELEASES_URL: &str = "https://api.github.com/repos/ericflo/modelrelay/releases?per_page=100";

/// How long to cache the GitHub release lookup before refetching.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Cached latest release info.
#[derive(Clone)]
pub(super) struct LatestRelease {
    pub(super) version: String,
    pub(super) published_at: Option<String>,
    pub(super) body: Option<String>,
    pub(super) assets: Vec<Asset>,
}

#[derive(Clone)]
pub(super) struct Asset {
    pub(super) name: String,
    pub(super) url: String,
}

static LATEST_CACHE: LazyLock<RwLock<Option<(Instant, LatestRelease)>>> =
    LazyLock::new(|| RwLock::new(None));

/// Tauri v2 dynamic updater JSON response.
#[derive(Serialize)]
struct UpdateManifest {
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub_date: Option<String>,
    url: String,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
}

/// Return the cached latest desktop release version without performing a network request.
pub(super) async fn cached_release_version() -> Option<String> {
    LATEST_CACHE
        .read()
        .await
        .as_ref()
        .map(|(_, release)| release.version.clone())
}

/// Shared release lookup used by both `/updater/desktop/...` and `/download/desktop/...`.
/// Falls back to a previously cached release if GitHub is rate-limited or unreachable.
pub(super) async fn latest_release(client: &reqwest::Client) -> Option<LatestRelease> {
    let stale_fallback = {
        let guard = LATEST_CACHE.read().await;
        if let Some((fetched_at, release)) = guard.as_ref() {
            if fetched_at.elapsed() < CACHE_TTL {
                return Some(release.clone());
            }
            Some(release.clone())
        } else {
            None
        }
    };

    if let Some(release) = fetch_latest_release_from_github(client).await {
        *LATEST_CACHE.write().await = Some((Instant::now(), release.clone()));
        Some(release)
    } else {
        if stale_fallback.is_some() {
            tracing::warn!("GitHub releases lookup failed; serving stale cached release");
        }
        stale_fallback
    }
}

async fn fetch_latest_release_from_github(client: &reqwest::Client) -> Option<LatestRelease> {
    let mut req = client
        .get(RELEASES_URL)
        .header(header::ACCEPT, "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }

    let releases: Vec<Value> = req
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;

    let latest_value = releases.into_iter().find(|r| {
        let is_draft = r.get("draft").and_then(Value::as_bool).unwrap_or(false);
        let is_prerelease = r
            .get("prerelease")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        !is_draft
            && !is_prerelease
            && r.get("tag_name")
                .and_then(Value::as_str)
                .is_some_and(|tag| tag.starts_with("desktop-v"))
    })?;

    let tag = latest_value.get("tag_name").and_then(Value::as_str)?;
    let version = tag.strip_prefix("desktop-v").unwrap_or(tag).to_owned();
    let published_at = latest_value
        .get("published_at")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let body = latest_value
        .get("body")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let assets = latest_value
        .get("assets")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|a| {
            Some(Asset {
                name: a.get("name").and_then(Value::as_str)?.to_owned(),
                url: a
                    .get("browser_download_url")
                    .and_then(Value::as_str)?
                    .to_owned(),
            })
        })
        .collect();

    Some(LatestRelease {
        version,
        published_at,
        body,
        assets,
    })
}

/// Decide whether the server version strictly beats the client version using
/// `SemVer` 2.0.0 precedence (including pre-release ordering: `0.1.1 > 0.1.1-alpha.1`,
/// `0.1.1-alpha.2 > 0.1.1-alpha.1`).
fn is_newer(server: &str, client: &str) -> bool {
    type ParsedSemver<'a> = ((u32, u32, u32), Option<&'a str>);

    fn parse_semver(v: &str) -> Option<ParsedSemver<'_>> {
        let v = v.strip_prefix('v').unwrap_or(v);
        // Strip build metadata (+...)
        let without_build = v.split('+').next().unwrap_or(v);
        let (core, pre) = match without_build.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p)),
            _ => (without_build, None),
        };
        let mut it = core.split('.');
        let a = it.next()?.parse().ok()?;
        let b = it.next()?.parse().ok()?;
        let c = it.next()?.parse().ok()?;
        if it.next().is_some() {
            return None;
        }
        Some(((a, b, c), pre))
    }

    fn compare_pre(a: &str, b: &str) -> Ordering {
        for (pa, pb) in a.split('.').zip(b.split('.')) {
            let ord = match (pa.parse::<u64>(), pb.parse::<u64>()) {
                (Ok(na), Ok(nb)) => na.cmp(&nb),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => pa.cmp(pb),
            };
            if ord != Ordering::Equal {
                return ord;
            }
        }
        a.split('.').count().cmp(&b.split('.').count())
    }

    match (parse_semver(server), parse_semver(client)) {
        (Some((s_core, s_pre)), Some((c_core, c_pre))) => match s_core.cmp(&c_core) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => match (s_pre, c_pre) {
                (None, Some(_)) => true, // stable beats prerelease of same core
                (Some(_) | None, None) => false,
                (Some(sp), Some(cp)) => compare_pre(sp, cp) == Ordering::Greater,
            },
        },
        _ => server > client,
    }
}

/// For a given Tauri `{target}/{arch}` pair, return the filename suffix of the
/// updater-signed artifact we expect the release workflow to upload.
fn artifact_suffixes(target: &str, arch: &str) -> Option<&'static [&'static str]> {
    match (target, arch) {
        ("darwin", "aarch64") => Some(&["_aarch64.app.tar.gz", "-aarch64.app.tar.gz"]),
        ("darwin", "x86_64") => Some(&["_x64.app.tar.gz", "-x64.app.tar.gz"]),
        ("linux", "x86_64") => Some(&["_amd64.AppImage", "-amd64.AppImage", "_x86_64.AppImage"]),
        ("windows", "x86_64") => Some(&["_x64-setup.exe", "-x64-setup.exe"]),
        _ => None,
    }
}

/// Find the signed bundle URL and the matching `.sig` URL for a given target.
fn find_pair(assets: &[Asset], suffixes: &[&str]) -> Option<(String, String)> {
    let bundle = assets
        .iter()
        .find(|a| suffixes.iter().any(|s| a.name.ends_with(s)))?;
    let sig_name = format!("{}.sig", bundle.name);
    let sig = assets.iter().find(|a| a.name == sig_name)?;
    Some((bundle.url.clone(), sig.url.clone()))
}

/// Download the raw base64 contents of a `.sig` file from a GitHub release asset URL.
async fn fetch_signature(client: &reqwest::Client, url: &str) -> Option<String> {
    let resp = client
        .get(url)
        .header(header::ACCEPT, "application/octet-stream")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let text = resp.text().await.ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Signature cache keyed by the asset's download URL.
static SIGNATURE_CACHE: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

async fn fetch_signature_cached(client: &reqwest::Client, url: &str) -> Option<String> {
    if let Some(sig) = SIGNATURE_CACHE.read().await.get(url) {
        return Some(sig.clone());
    }
    let sig = fetch_signature(client, url).await?;
    let mut cache = SIGNATURE_CACHE.write().await;
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(url.to_owned(), sig.clone());
    Some(sig)
}

/// Route handler: `GET /updater/desktop/{target}/{arch}/{current_version}`.
pub async fn desktop_update_check(
    State(state): State<Arc<CloudState>>,
    Path((target, arch, current_version)): Path<(String, String, String)>,
) -> Response {
    let Some(suffixes) = artifact_suffixes(&target, &arch) else {
        return StatusCode::NO_CONTENT.into_response();
    };

    let Some(release) = latest_release(&state.http_client).await else {
        return StatusCode::NO_CONTENT.into_response();
    };

    if !is_newer(&release.version, &current_version) {
        return StatusCode::NO_CONTENT.into_response();
    }

    let Some((bundle_url, sig_url)) = find_pair(&release.assets, suffixes) else {
        return StatusCode::NO_CONTENT.into_response();
    };

    let Some(signature) = fetch_signature_cached(&state.http_client, &sig_url).await else {
        return StatusCode::NO_CONTENT.into_response();
    };

    let manifest = UpdateManifest {
        version: release.version,
        pub_date: release.published_at,
        url: bundle_url,
        signature,
        notes: release.body,
    };

    (
        StatusCode::OK,
        [(
            header::CACHE_CONTROL,
            "public, max-age=60, stale-while-revalidate=300",
        )],
        Json(manifest),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{Asset, artifact_suffixes, find_pair, is_newer};

    #[test]
    fn newer_semver_wins() {
        assert!(is_newer("0.1.2", "0.1.1"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn prerelease_precedence_follows_semver() {
        // Stable release beats prerelease of the same core version.
        assert!(is_newer("0.1.1", "0.1.1-alpha.1"));
        assert!(!is_newer("0.1.1-alpha.1", "0.1.1"));
        assert!(is_newer("0.1.1-alpha.2", "0.1.1-alpha.1"));
        assert!(is_newer("0.1.1-rc.1", "0.1.1-beta.9"));
        assert!(is_newer("0.1.2", "0.1.1-rc.5"));
    }

    #[test]
    fn malformed_versions_fall_back_to_string_compare() {
        assert!(!is_newer("abc", "abc"));
        assert!(is_newer("z", "a"));
    }

    #[test]
    fn platform_suffixes_cover_expected_targets() {
        assert!(artifact_suffixes("darwin", "aarch64").is_some());
        assert!(artifact_suffixes("darwin", "x86_64").is_some());
        assert!(artifact_suffixes("linux", "x86_64").is_some());
        assert!(artifact_suffixes("windows", "x86_64").is_some());
        assert!(artifact_suffixes("linux", "aarch64").is_none());
        assert!(artifact_suffixes("bogus", "x86_64").is_none());
    }

    fn a(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            url: format!("https://example.com/{name}"),
        }
    }

    #[test]
    fn find_pair_matches_bundle_and_sig() {
        let assets = vec![
            a("ModelRelay_0.1.2_aarch64.app.tar.gz"),
            a("ModelRelay_0.1.2_aarch64.app.tar.gz.sig"),
            a("ModelRelay_0.1.2_x64-setup.exe"),
            a("ModelRelay_0.1.2_x64-setup.exe.sig"),
        ];
        let suffixes = artifact_suffixes("darwin", "aarch64").unwrap();
        let (bundle, sig) = find_pair(&assets, suffixes).expect("match");
        assert!(bundle.ends_with("_aarch64.app.tar.gz"));
        assert!(sig.ends_with("_aarch64.app.tar.gz.sig"));
    }

    #[test]
    fn find_pair_requires_a_matching_sig() {
        let assets = vec![a("ModelRelay_0.1.2_aarch64.app.tar.gz")];
        let suffixes = artifact_suffixes("darwin", "aarch64").unwrap();
        assert!(find_pair(&assets, suffixes).is_none());
    }
}
