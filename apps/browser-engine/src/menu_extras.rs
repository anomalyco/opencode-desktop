//! Engine half of the page menu's Copy image and Spellcheck / Languages items (official
//! `page-menu.ts:102, 119-150`, `native-page-menu.ts` `copyImageAt`, `setSpellCheckerEnabled`,
//! `setSpellCheckerLanguages`). The app draws the items once `Ready.menu_extras` says the engine
//! runs them (#516).
//!
//! CEF has no `copyImageAt`, so Copy image takes three DevTools steps: the image's bytes from the
//! frame's resources (`Page.getResourceContent`, which needs no CORS), a fresh isolated world in
//! that frame, and there a PNG re-encode from a `Blob` (`createImageBitmap` + `OffscreenCanvas`),
//! so every format Chromium decodes reaches the app as PNG for its clipboard
//! (`EngineMsg::CopiedImage`). The isolated world keeps the page's own scripts from replacing the
//! functions the re-encode uses.

use cef::{CefString, ImplListValue, ImplPreferenceManager, ImplValue, RequestContext};
use serde_json::{Value, json};

/// The largest image (as base64) the engine reads or sends: inside the wire's 32 MiB line bound.
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
    if let Some(val) = cef::value_create() {
        val.set_bool(i32::from(!on));
        set_pref(ctx, SPELLCHECK_PREF, val);
    }
}

/// Adds or removes one language; refuses a tag the menu did not offer, and removing the last one
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
    let (Some(mut list), Some(val)) = (cef::list_value_create(), cef::value_create()) else { return };
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

/// The image's bytes (base64) from step 1's reply, when it found any that fit.
pub fn resource_base64(result: &Value) -> Option<String> {
    let content = result.get("content").and_then(Value::as_str)?;
    if content.is_empty() || content.len() > MAX_COPIED_IMAGE_B64 {
        return None;
    }
    Some(if result.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
        content.to_owned()
    } else {
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content.as_bytes())
    })
}

/// Step 2: a fresh isolated world in the image's frame, so page script cannot touch step 3.
pub fn world_request(id: i32, frame_id: &str) -> String {
    json!({ "id": id, "method": "Page.createIsolatedWorld", "params": { "frameId": frame_id, "worldName": "opencode-copy-image" } })
        .to_string()
}

/// Step 3: re-encode the bytes as PNG in that world, from step 2's reply.
pub fn encode_request(id: i32, world: &Value, b64: &str) -> Option<String> {
    let context_id = world.get("executionContextId").and_then(Value::as_i64)?;
    let b64_json = serde_json::to_string(b64).ok()?;
    let expression = format!(
        r#"(async () => {{
  const raw = atob({b64_json}); const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  let image;
  try {{ image = await createImageBitmap(new Blob([bytes])); }} catch {{
    // `createImageBitmap` takes no SVG from a Blob; an image element does.
    image = new Image(); image.src = URL.createObjectURL(new Blob([bytes], {{ type: "image/svg+xml" }})); await image.decode(); URL.revokeObjectURL(image.src);
  }}
  const width = image.naturalWidth || image.width, height = image.naturalHeight || image.height;
  const canvas = new OffscreenCanvas(width, height);
  canvas.getContext("2d").drawImage(image, 0, 0, width, height);
  const png = new Uint8Array(await (await canvas.convertToBlob({{ type: "image/png" }})).arrayBuffer());
  let s = ""; for (let i = 0; i < png.length; i += 32768) s += String.fromCharCode.apply(null, png.subarray(i, i + 32768));
  return btoa(s);
}})()"#
    );
    let msg = json!({
        "id": id,
        "method": "Runtime.evaluate",
        "params": { "expression": expression, "contextId": context_id, "awaitPromise": true, "returnByValue": true },
    });
    Some(msg.to_string())
}

/// The PNG (base64) step 3 produced, when it is one and fits the wire.
pub fn png_result(result: &Value) -> Option<String> {
    let png = result.get("result")?.get("value")?.as_str()?;
    (png.len() <= MAX_COPIED_IMAGE_B64 && png.starts_with("iVBORw0KGgo")).then(|| png.to_owned())
}
