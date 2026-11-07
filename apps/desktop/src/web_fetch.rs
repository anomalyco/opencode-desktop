//! A plain web GET for pictures a reply shows from the web (a markdown image's URL, a link's
//! favicon), as Desktop's renderer loads an `<img>`: no credentials, a short timeout, at most
//! 8 MB, and only a successful reply counts.

use std::time::Duration;

pub fn get(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(true).build().into();
    let mut resp = agent.get(url).call().map_err(|e| format!("GET {url}: {e}"))?;
    resp.body_mut().with_config().limit(8 << 20).read_to_vec().map_err(|e| format!("GET {url}: {e}"))
}
