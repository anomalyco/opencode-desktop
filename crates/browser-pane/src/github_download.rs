//! Opens the engine archive: the public release URL first, then the GitHub API with the user's
//! GitHub login when the release is private (a 401, 403 or 404 from the public URL).

use std::path::PathBuf;
use std::process::{Command, Stdio};

use ureq::config::RedirectAuthHeaders;

pub const ERR_DOWNLOAD: &str =
    "Couldn't download the browser. Check your connection and try again.";
pub const ERR_SIGN_IN: &str =
    "Sign in to GitHub to download the browser.\nRun gh auth login in Terminal, then retry.";
pub(crate) const ERR_LOGIN_EXPIRED: &str =
    "Your GitHub login has expired.\nRun gh auth login in Terminal, then retry.";
pub(crate) const ERR_MISSING: &str = "The browser download is missing from its GitHub release.";

pub fn err_no_access(owner: &str, repo: &str) -> String {
    format!("Your GitHub account can't see the browser download.\nAsk for access to {owner}/{repo}.")
}

fn err_server(code: u16) -> String {
    format!("The download server returned error {code}. Try again later.")
}

/// Where the private-release fallback asks, and how it finds the user's GitHub token.
pub(crate) struct GithubAuth {
    pub api_base: String,
    pub token: Box<dyn Fn() -> Option<String> + Send + Sync>,
}

impl GithubAuth {
    pub(crate) fn user() -> Self {
        Self {
            api_base: "https://api.github.com".into(),
            token: Box::new(user_token),
        }
    }
}

/// `<owner>/<repo>/releases/download/<tag>/<file>`, read from a release download URL.
#[derive(Debug, PartialEq)]
pub(crate) struct ReleaseAsset {
    pub owner: String,
    pub repo: String,
    pub tag: String,
    pub file: String,
}

impl ReleaseAsset {
    pub(crate) fn parse(url: &str) -> Option<Self> {
        let url = url.split(['?', '#']).next()?;
        let (_, rest) = url.split_once("://")?;
        let (_, path) = rest.split_once('/')?;
        match path.split('/').collect::<Vec<_>>().as_slice() {
            [owner, repo, "releases", "download", tag, file]
                if [owner, repo, tag, file].iter().all(|s| !s.is_empty()) =>
            {
                Some(Self {
                    owner: (*owner).into(),
                    repo: (*repo).into(),
                    tag: (*tag).into(),
                    file: (*file).into(),
                })
            }
            _ => None,
        }
    }
}

pub(crate) fn open(url: &str, auth: &GithubAuth) -> Result<ureq::http::Response<ureq::Body>, String> {
    match ureq::get(url).call() {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::StatusCode(code @ (401 | 403 | 404))) => match ReleaseAsset::parse(url) {
            Some(asset) => open_private(&asset, auth),
            None => Err(err_server(code)),
        },
        Err(ureq::Error::StatusCode(code)) => Err(err_server(code)),
        Err(_) => Err(ERR_DOWNLOAD.into()),
    }
}

fn open_private(
    asset: &ReleaseAsset,
    auth: &GithubAuth,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let token = (auth.token)().ok_or_else(|| ERR_SIGN_IN.to_owned())?;
    // The asset URL redirects to a signed storage host; the token must never follow it there.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .redirect_auth_headers(RedirectAuthHeaders::Never)
        .build()
        .into();
    let api_error = |err: ureq::Error| match err {
        ureq::Error::StatusCode(401) => ERR_LOGIN_EXPIRED.to_owned(),
        ureq::Error::StatusCode(403 | 404) => err_no_access(&asset.owner, &asset.repo),
        ureq::Error::StatusCode(code) => err_server(code),
        _ => ERR_DOWNLOAD.to_owned(),
    };
    let bearer = format!("Bearer {token}");

    let release_url = format!(
        "{}/repos/{}/{}/releases/tags/{}",
        auth.api_base.trim_end_matches('/'),
        asset.owner,
        asset.repo,
        asset.tag
    );
    let mut release = agent
        .get(&release_url)
        .header("Authorization", &bearer)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(api_error)?;
    let json: serde_json::Value = release
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .ok_or_else(|| ERR_DOWNLOAD.to_owned())?;
    let asset_url = json["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a["name"].as_str() == Some(asset.file.as_str()))
        })
        .and_then(|a| a["url"].as_str())
        .ok_or_else(|| ERR_MISSING.to_owned())?;

    agent
        .get(asset_url)
        .header("Authorization", &bearer)
        .header("Accept", "application/octet-stream")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(api_error)
}

/// The user's GitHub token from `GH_TOKEN`/`GITHUB_TOKEN`, else `gh auth token`. GUI apps don't
/// inherit a shell `PATH`, so `gh` is also looked for where Homebrew installs it.
fn user_token() -> Option<String> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    let mut candidates: Vec<PathBuf> =
        vec!["/opt/homebrew/bin/gh".into(), "/usr/local/bin/gh".into()];
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("gh")));
    }
    candidates.into_iter().filter(|gh| gh.is_file()).find_map(|gh| {
        let out = Command::new(gh)
            .args(["auth", "token", "--hostname", "github.com"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let token = String::from_utf8(out.stdout).ok()?.trim().to_owned();
        (out.status.success() && !token.is_empty()).then_some(token)
    })
}
