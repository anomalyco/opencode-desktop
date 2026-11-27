//! Resolves a picked DOM element's live bounding box, CSS selector, ARIA role/name, and label via
//! read-only CDP (`DOM.resolveNode` -> `Runtime.callFunctionOn`, with `DOM.getBoxModel` fallback)
//! so the floating spotlight and comment editor match official `packages/gui-plugins/src/browser/`
//! even when the engine's `PickedElement` carries placeholder `node#N` metadata and a zero rect.

use browser_pane::PickedElement;
use opencode_kit::model::NoteComment;
use serde_json::{Value, json};

use crate::App;

/// Read-only function evaluated on the resolved DOM node (`chromium.ts` element descriptor).
const DESCRIBE_NODE_FN: &str = r#"function() {
  var el = this && this.nodeType === 1 ? this : (this && this.parentElement ? this.parentElement : null);
  if (!el) return null;
  var r = el.getBoundingClientRect();
  var tag = (el.tagName || '').toLowerCase();
  var esc = function(s) {
    return (typeof CSS !== 'undefined' && CSS.escape) ? CSS.escape(s) : String(s).replace(/([^\w-])/g, '\\$1');
  };
  var part = function(n) {
    var t = (n.tagName || '').toLowerCase();
    if (!t) return '';
    if (n.id) return t + '#' + esc(n.id);
    var cls = typeof n.className === 'string'
      ? n.className.trim().split(/\s+/).filter(Boolean).slice(0, 2).map(function(c){ return '.' + esc(c); }).join('')
      : '';
    var s = t + cls;
    var p = n.parentElement;
    if (p) {
      var same = 0, idx = 1;
      for (var c = p.firstElementChild; c; c = c.nextElementSibling) {
        if (c === n) idx = same + 1;
        if (c.tagName === n.tagName) same++;
      }
      if (same > 1 && !cls) s += ':nth-of-type(' + idx + ')';
    }
    return s;
  };
  var chain = [];
  var cur = el;
  while (cur && cur.nodeType === 1 && chain.length < 4) {
    var seg = part(cur);
    if (seg) chain.unshift(seg);
    if (cur.id) break;
    cur = cur.parentElement;
  }
  var selector = chain.join(' > ') || tag;
  var role = el.getAttribute('role') || (tag === 'button' ? 'button' : tag === 'a' ? 'link' : tag === 'input' ? (el.type || 'textbox') : '');
  var aria = el.getAttribute('aria-label') || '';
  var txt = (el.innerText || el.textContent || '').replace(/\s+/g, ' ').trim().slice(0, 80);
  var name = aria || (tag === 'img' ? (el.getAttribute('alt') || '') : '') || (tag === 'button' || tag === 'a' ? txt : '');
  var label = name ? (tag + ' "' + name.slice(0, 40) + '"') : selector;
  return {
    rect: [r.left || 0, r.top || 0, r.width || 0, r.height || 0],
    selector: selector,
    label: label,
    role: role || null,
    name: name || null,
    text: txt || null
  };
}"#;

#[derive(Default)]
pub struct PendingDescribe {
    resolve: Option<(u64, u64)>,
    call: Option<(u64, u64, String)>,
    box_model: Option<(u64, u64)>,
}

impl PendingDescribe {
    pub fn clear(&mut self) {
        self.resolve = None;
        self.call = None;
        self.box_model = None;
    }

    pub fn start(app: &mut App, tab_id: u64, backend_node_id: i64) {
        app.workspace_panes.browser.overlay.describe.clear();
        if let Ok(req) = app.workspace_panes.browser.host.send_typed_cdp(
            tab_id,
            "DOM.resolveNode",
            json!({ "backendNodeId": backend_node_id }),
            None,
        ) {
            app.workspace_panes.browser.overlay.describe.resolve = Some((tab_id, req));
        } else if let Ok(req) = app.workspace_panes.browser.host.send_typed_cdp(
            tab_id,
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
            None,
        ) {
            app.workspace_panes.browser.overlay.describe.box_model = Some((tab_id, req));
        }
    }

    pub fn poll(app: &mut App) -> bool {
        let mut changed = false;
        if let Some((tab_id, req_id)) = app.workspace_panes.browser.overlay.describe.resolve
            && let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req_id)
        {
            app.workspace_panes.browser.overlay.describe.resolve = None;
            let obj_id = resp
                .ok()
                .and_then(|v| v.pointer("/object/objectId").and_then(Value::as_str).map(str::to_owned));
            if let Some(oid) = obj_id
                && let Ok(call_req) = app.workspace_panes.browser.host.send_typed_cdp(
                    tab_id,
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": oid,
                        "functionDeclaration": DESCRIBE_NODE_FN,
                        "returnByValue": true,
                    }),
                    None,
                )
            {
                app.workspace_panes.browser.overlay.describe.call = Some((tab_id, call_req, oid));
            } else {
                request_box_fallback(app, tab_id);
            }
        }

        if let Some((tab_id, req_id, oid)) = app.workspace_panes.browser.overlay.describe.call.clone()
            && let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req_id)
        {
            app.workspace_panes.browser.overlay.describe.call = None;
            let _ = app.workspace_panes.browser.host.send_typed_cdp(
                tab_id,
                "Runtime.releaseObject",
                json!({ "objectId": oid }),
                None,
            );
            let mut got_rect = false;
            if let Ok(val) = resp
                && let Some(obj) = val.pointer("/result/value").and_then(Value::as_object)
            {
                got_rect = app.workspace_panes.browser.overlay.apply_described(tab_id, obj);
                changed = true;
            }
            if !got_rect {
                request_box_fallback(app, tab_id);
            }
        }

        if let Some((tab_id, req_id)) = app.workspace_panes.browser.overlay.describe.box_model
            && let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req_id)
        {
            app.workspace_panes.browser.overlay.describe.box_model = None;
            if let Ok(val) = resp
                && let Some(rect) = parse_box_model(&val)
                && app.workspace_panes.browser.overlay.apply_rect(tab_id, rect)
            {
                changed = true;
            }
        }
        changed
    }
}

fn request_box_fallback(app: &mut App, tab_id: u64) {
    let Some(backend_id) = app.workspace_panes.browser.overlay.commenting_backend_id(tab_id) else {
        return;
    };
    if let Ok(req) = app.workspace_panes.browser.host.send_typed_cdp(
        tab_id,
        "DOM.getBoxModel",
        json!({ "backendNodeId": backend_id }),
        None,
    ) {
        app.workspace_panes.browser.overlay.describe.box_model = Some((tab_id, req));
    }
}

pub fn apply_object(el: &mut PickedElement, obj: &serde_json::Map<String, Value>) -> bool {
    if let Some(s) = obj.get("selector").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        el.selector = s.to_owned();
    }
    if let Some(s) = obj.get("label").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        el.label = s.to_owned();
    }
    if let Some(s) = obj.get("role").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        el.role = Some(s.to_owned());
    }
    if let Some(s) = obj.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        el.name = Some(s.to_owned());
    }
    if let Some(s) = obj.get("text").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        el.text = Some(s.to_owned());
    }
    if let Some(arr) = obj.get("rect").and_then(Value::as_array)
        && arr.len() == 4
    {
        let r = [
            arr[0].as_f64().unwrap_or(0.0) as f32,
            arr[1].as_f64().unwrap_or(0.0) as f32,
            arr[2].as_f64().unwrap_or(0.0) as f32,
            arr[3].as_f64().unwrap_or(0.0) as f32,
        ];
        if r[2] > 0.0 && r[3] > 0.0 {
            el.rect = r;
            return true;
        }
    }
    false
}

fn parse_box_model(val: &Value) -> Option<[f32; 4]> {
    let quad = val.pointer("/model/border").and_then(Value::as_array)?;
    if quad.len() < 8 {
        return None;
    }
    let xs = [0, 2, 4, 6].map(|i| quad[i].as_f64().unwrap_or(0.0) as f32);
    let ys = [1, 3, 5, 7].map(|i| quad[i].as_f64().unwrap_or(0.0) as f32);
    let min_x = xs.into_iter().fold(f32::INFINITY, f32::min);
    let max_x = xs.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let min_y = ys.into_iter().fold(f32::INFINITY, f32::min);
    let max_y = ys.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let w = (max_x - min_x).max(0.0);
    let h = (max_y - min_y).max(0.0);
    (w > 0.0 && h > 0.0).then_some([min_x, min_y, w, h])
}

pub fn element_note(tab_id: u64, url: &str, element: &PickedElement, live: bool, comment: &str) -> NoteComment {
    let label = if element.label.is_empty() {
        element.selector.clone()
    } else {
        element.label.clone()
    };
    let title = if element.selector.is_empty() {
        label.clone()
    } else {
        element.selector.clone()
    };
    let durable = format_element_subject(tab_id, url, element, None);
    let live_subject = (live && !element.eref.is_empty())
        .then(|| format_element_subject(tab_id, url, element, Some(&element.eref)));
    let live_href = (live && !element.eref.is_empty()).then(|| format!("{tab_id}#{}", element.eref));
    NoteComment {
        origin: "browser".into(),
        label,
        title: Some(title),
        subtitle: Some(opencode_i18n::t!("plugin.browser.comment.annotation").to_owned()),
        icon: Some("select-element".into()),
        subject: durable,
        comment: comment.to_owned(),
        href: Some(tab_id.to_string()),
        live_subject,
        live_href,
    }
}

pub fn page_note(tab_id: u64, url: &str, title: &str, comment: &str) -> NoteComment {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or("");
    let label = if !title.is_empty() {
        title.to_owned()
    } else if !host.is_empty() {
        host.to_owned()
    } else {
        url.to_owned()
    };
    let titled = if title.is_empty() {
        String::new()
    } else {
        format!(" titled {}", serde_json::to_string(title).unwrap_or_default())
    };
    NoteComment {
        origin: "browser".into(),
        label,
        title: None,
        subtitle: None,
        icon: Some("globe".into()),
        subject: format!("the page in browser tab {tab_id} at {url}{titled}"),
        comment: comment.to_owned(),
        href: Some(tab_id.to_string()),
        live_subject: None,
        live_href: None,
    }
}

fn format_element_subject(tab_id: u64, url: &str, el: &PickedElement, eref: Option<&str>) -> String {
    let mut details: Vec<String> = Vec::new();
    if let Some(role) = el.role.as_deref().filter(|s| !s.is_empty()) {
        details.push(format!("role {role}"));
    }
    if let Some(name) = el.name.as_deref().filter(|s| !s.is_empty()) {
        details.push(format!("accessible name {}", serde_json::to_string(name).unwrap_or_default()));
    }
    if let Some(txt) = el.text.as_deref().filter(|s| !s.is_empty() && Some(*s) != el.name.as_deref()) {
        let short: String = txt.chars().take(80).collect();
        details.push(format!("text {}", serde_json::to_string(&short).unwrap_or_default()));
    }
    if !el.selector.is_empty() {
        let shadow = if el.selector.contains(" >>> ") {
            " (\">>>\" enters a shadow root)"
        } else {
            ""
        };
        details.push(format!("selector {}{shadow}", serde_json::to_string(&el.selector).unwrap_or_default()));
    }
    if let Some(r) = eref {
        let trimmed = r.strip_prefix('@').unwrap_or(r);
        details.push(format!(
            "browser ref @{trimmed}, usable as ref in any browser tool including browser.evaluate until the page navigates"
        ));
    }
    let label_q = serde_json::to_string(&el.label).unwrap_or_else(|_| "\"element\"".into());
    if details.is_empty() {
        format!("the {label_q} element in browser tab {tab_id} at {url}")
    } else {
        format!("the {label_q} element in browser tab {tab_id} at {url} ({})", details.join("; "))
    }
}
