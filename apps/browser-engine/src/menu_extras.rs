//! Engine half of the page menu's Copy image and Spellcheck / Languages items (official
//! `page-menu.ts:102, 119-150`, `native-page-menu.ts` `copyImageAt`, `setSpellCheckerEnabled`,
//! `setSpellCheckerLanguages`). WS-6 draws the items; the wire shape is agreed in the inbox.
//!
//! CEF has no `copyImageAt`, so Copy image reads the image's bytes from the page's resources
//! (`Page.getResourceContent`, which needs no CORS) and has Chromium re-encode them as PNG from a
//! same-origin `Blob` (`createImageBitmap` + `OffscreenCanvas`), so every format Chromium decodes
//! reaches the app as PNG for its clipboard (`EngineMsg::CopiedImage`).

use std::sync::atomic::{AtomicI32, Ordering};

use cef::{CefString, ImplListValue, ImplRequestContext, ImplValue, RequestContext};
use serde_json::{Value, json};

/// CDP ids for the second step of Copy image, in `cdp_id_map`'s range (below the passkey and
/// frame-authority internal ranges) so their replies reach `CdpPending::CopyImageEncode`.
const COPY_IMAGE_ID_BASE: i32 = 1_450_000_000;
static COPY_IMAGE_SEQ: AtomicI32 = AtomicI32::new(0);
/// The largest PNG (as base64) sent to the app: inside the wire's 32 MiB line bound.
pub const MAX_COPIED_IMAGE_B64: usize = 24 * 1024 * 1024;

const SPELLCHECK_PREF: &str = "browser.enable_spellchecking";
const DICTIONARIES_PREF: &str = "spellcheck.dictionaries";
const ACCEPT_LANGUAGES_PREF: &str = "intl.accept_languages";

fn string_list(ctx: &RequestContext, name: &str) -> Vec<String> {
    let Some(v) = ctx.preference(Some(&CefString::from(name))) else { return Vec::new() };
    if let Some(list) = v.list() {
        return (0..list.size())
            .map(|i| CefString::from(&list.string(i)).to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    CefString::from(&v.string())
        .to_string()
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `(spellcheck on, enabled languages, offerable languages)` for an editable field's menu. The
/// offer is the enabled dictionaries plus the profile's accept-languages; empty on macOS, where
/// the system's checker picks languages itself (official offers none there either).
pub fn spellcheck_state(ctx: &RequestContext) -> (bool, Vec<String>, Vec<String>) {
    let on = ctx
        .preference(Some(&CefString::from(SPELLCHECK_PREF)))
        .is_none_or(|v| v.bool() != 0);
    let languages = string_list(ctx, DICTIONARIES_PREF);
    if cfg!(target_os = "macos") {
        return (on, languages, Vec::new());
    }
    let mut offer = languages.clone();
    for lang in string_list(ctx, ACCEPT_LANGUAGES_PREF) {
        if !offer.contains(&lang) {
            offer.push(lang);
        }
    }
    offer.truncate(32);
    (on, languages, offer)
}

fn set_pref(ctx: &RequestContext, name: &str, mut val: cef::Value) -> bool {
    let key = CefString::from(name);
    if ctx.can_set_preference(Some(&key)) == 0 {
        return false;
    }
    // `CefString::default()` maps to NULL in `cef-rs`, which `SetPreference` rejects.
    let mut err = CefString::from(" ");
    ctx.set_preference(Some(&key), Some(&mut val), Some(&mut err)) != 0
}

pub fn toggle_spellcheck(ctx: &RequestContext) {
    let (on, _, _) = spellcheck_state(ctx);
    if let Some(mut val) = cef::value_create() {
        val.set_bool(i32::from(!on));
        set_pref(ctx, SPELLCHECK_PREF, val);
    }
}

/// Adds or removes one language; refuses a tag the menu did not offer and removing the last one
/// (official `page-menu.ts:143`).
pub fn toggle_language(ctx: &RequestContext, value: &str) {
    let (_, languages, offer) = spellcheck_state(ctx);
    if !offer.iter().any(|o| o == value) {
        return;
    }
    let next: Vec<String> = if languages.iter().any(|l| l == value) {
        languages.into_iter().filter(|l| l != value).collect()
    } else {
        languages.into_iter().chain([value.to_owned()]).collect()
    };
    if next.is_empty() {
        return;
    }
    let (Some(mut list), Some(mut val)) = (cef::list_value_create(), cef::value_create()) else { return };
    for (i, lang) in next.iter().enumerate() {
        list.set_string(i, Some(&CefString::from(lang.as_str())));
    }
    val.set_list(Some(&mut list));
    set_pref(ctx, DICTIONARIES_PREF, val);
}

/// Step 1 of Copy image: the image's bytes from the frame's resources.
pub fn resource_request(id: i32, frame_id: &str, url: &str) -> String {
    json!({ "id": id, "method": "Page.getResourceContent", "params": { "frameId": frame_id, "url": url } }).to_string()
}

/// Step 2: re-encode the bytes as PNG in the page's own main world from a same-origin `Blob`.
/// Returns `(cdp id, message)`; `None` when step 1 failed or found nothing.
pub fn encode_request(result: &Value) -> Option<(i32, String)> {
    let content = result.get("content").and_then(Value::as_str)?;
    if content.is_empty() || content.len() > MAX_COPIED_IMAGE_B64 {
        return None;
    }
    let b64 = if result.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
        content.to_owned()
    } else {
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content.as_bytes())
    };
    let b64_json = serde_json::to_string(&b64).ok()?;
    let expression = format!(
        r#"(async () => {{
  const raw = atob({b64_json}); const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  const bitmap = await createImageBitmap(new Blob([bytes]));
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  canvas.getContext("2d").drawImage(bitmap, 0, 0);
  const png = new Uint8Array(await (await canvas.convertToBlob({{ type: "image/png" }})).arrayBuffer());
  let s = ""; for (let i = 0; i < png.length; i += 32768) s += String.fromCharCode.apply(null, png.subarray(i, i + 32768));
  return btoa(s);
}})()"#
    );
    let id = COPY_IMAGE_ID_BASE + COPY_IMAGE_SEQ.fetch_add(1, Ordering::Relaxed).rem_euclid(40_000_000);
    let msg = json!({ "id": id, "method": "Runtime.evaluate", "params": { "expression": expression, "awaitPromise": true, "returnByValue": true } });
    Some((id, msg.to_string()))
}

/// The PNG (base64) step 2 produced, when it is one and fits the wire.
pub fn png_result(result: &Value) -> Option<String> {
    let png = result.get("result")?.get("value")?.as_str()?;
    (png.len() <= MAX_COPIED_IMAGE_B64 && png.starts_with("iVBORw0KGgo")).then(|| png.to_owned())
}
