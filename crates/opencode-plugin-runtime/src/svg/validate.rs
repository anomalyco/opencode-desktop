//! Attribute and value validators for [`super::sanitize`].
//!
//! Boundary policy for plain-geometry plugin icons (Figma, SVGO, Lucide, Heroicons, Illustrator):
//! - **Allowed & re-emitted (value-validated):** shape geometry (`d`, `cx`, `cy`, `r`, `rx`, `ry`,
//!   `x`, `y`, `x1`, `y1`, `x2`, `y2`, `width`, `height`, `points`), `transform` (finite
//!   `matrix`/`translate`/`scale`/`rotate`/`skewX`/`skewY`, <= 8 ops), SMIL animation attributes,
//!   and presentation attributes (`fill`, `stroke`, `stroke-width`, `stroke-linecap`,
//!   `stroke-linejoin`, `stroke-miterlimit`, `stroke-dasharray`, `stroke-dashoffset`, `fill-rule`,
//!   `clip-rule`, `opacity`, `fill-opacity`, `stroke-opacity`, `paint-order`, `shape-rendering`,
//!   `vector-effect`, `visibility`, `display="none"`, `color`).
//! - **Allowed metadata & stripped (value-validated):** root `xmlns="http://www.w3.org/2000/svg"`,
//!   `xmlns:xlink="http://www.w3.org/1999/xlink"`, `version`, `xml:space`, `overflow`,
//!   `preserveAspectRatio`, `image-rendering`, `text-rendering`, root `x`/`y` and percentage
//!   `width`/`height` (when `viewBox` is present), `pathLength`, `display="block"|"inline"|"initial"`,
//!   `style="color: var(--ident)"`, and non-rendering authoring metadata (`id`, `class`, `role`,
//!   `aria-hidden`, `aria-label`, `focusable`, `tabindex`, `data-*`).
//! - **Refused everywhere (with exact tag/attribute/property in the error):** `on*` handlers,
//!   `href`, `xlink:href`, `src`, `filter`, `clip-path`, `clipPath`, `mask`, `marker-*`, any
//!   `url(...)`/`javascript:`/`vbscript:`/`data:`/`http(s):`/`file:`/`ftp:`/`expression(`/
//!   `@import` reference, arbitrary CSS properties in `style`, and unknown attributes.

use super::{
    AnimKind, MAX_CLOCK_SECS, MAX_COORD, MAX_DASH_ENTRIES, MAX_KEYFRAMES, MAX_PATH_BYTES, MAX_PATH_COMMANDS, MAX_POINTS_NUMBERS, MAX_REPEAT_COUNT, MAX_SCALE, MAX_SKEW_DEG, MAX_SPLINE_Y,
    MAX_STROKE_WIDTH, MAX_TRANSFORM_OPS, MAX_VIEW_DIM, MIN_DASH_SUM, MIN_DUR_SECS, MIN_SPLINE_Y, MIN_VIEW_DIM, PRESENTATION_ATTRS, ShapeKind,
};

enum Pres {
    Unknown,
    Drop,
    Emit(&'static str, String),
}

fn check_forbidden_attr(tag: &str, name: &str, val: &str) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("on")
        || matches!(
            lower.as_str(),
            "href" | "xlink:href" | "src" | "filter" | "clip-path" | "clippath" | "mask" | "marker-start" | "marker-mid" | "marker-end"
        )
    {
        return Err(format!("disallowed attribute {name:?} on <{tag}>"));
    }
    let vlower = val.to_ascii_lowercase();
    for bad in ["url(", "javascript:", "vbscript:", "data:", "http:", "https:", "file:", "ftp:", "expression(", "@import"] {
        if vlower.contains(bad) {
            return Err(format!("disallowed reference {bad:?} in attribute {name:?} on <{tag}>"));
        }
    }
    Ok(())
}

pub(super) fn validate_root(attrs: &[(&str, String)]) -> Result<(f32, f32, f32, Vec<(&'static str, String)>), String> {
    let mut view_box: Option<[f32; 4]> = None;
    let mut width: Option<f32> = None;
    let mut height: Option<f32> = None;
    let mut pres = Vec::new();
    for (name, val) in attrs {
        if *name == "xmlns" {
            if val != "http://www.w3.org/2000/svg" {
                return Err(format!("invalid xmlns {val:?} on <svg>"));
            }
            continue;
        }
        if *name == "xmlns:xlink" {
            if val != "http://www.w3.org/1999/xlink" {
                return Err(format!("invalid xmlns:xlink {val:?} on <svg>"));
            }
            continue;
        }
        check_forbidden_attr("svg", name, val)?;
        match *name {
            "viewBox" => {
                let nums = parse_numbers(val, 8)?;
                if nums.len() != 4 {
                    return Err(format!("invalid viewBox {val:?} on <svg>"));
                }
                let [x, y, w, h] = [nums[0], nums[1], nums[2], nums[3]];
                if !(-MAX_VIEW_DIM..=MAX_VIEW_DIM).contains(&x)
                    || !(-MAX_VIEW_DIM..=MAX_VIEW_DIM).contains(&y)
                    || !(MIN_VIEW_DIM..=MAX_VIEW_DIM).contains(&w)
                    || !(MIN_VIEW_DIM..=MAX_VIEW_DIM).contains(&h)
                {
                    return Err(format!("viewBox {val:?} out of bounds on <svg>"));
                }
                view_box = Some([x, y, w, h]);
            }
            "width" => width = parse_root_dim("width", val)?,
            "height" => height = parse_root_dim("height", val)?,
            "x" | "y" => {
                let _ = parse_length(val, -MAX_VIEW_DIM, MAX_VIEW_DIM).map_err(|e| format!("invalid {name} on <svg>: {e}"))?;
            }
            "version" => {
                if !matches!(val.trim(), "1.0" | "1.1" | "2.0") {
                    return Err(format!("invalid version {val:?} on <svg>"));
                }
            }
            "xml:space" => {
                if !matches!(val.trim(), "default" | "preserve") {
                    return Err(format!("invalid xml:space {val:?} on <svg>"));
                }
            }
            "overflow" => {
                if !matches!(val.trim(), "visible" | "hidden" | "auto" | "scroll") {
                    return Err(format!("invalid overflow {val:?} on <svg>"));
                }
            }
            "preserveAspectRatio" => validate_preserve_aspect_ratio(val)?,
            "image-rendering" => {
                if !matches!(val.trim(), "auto" | "optimizeSpeed" | "optimizeQuality") {
                    return Err(format!("invalid image-rendering {val:?} on <svg>"));
                }
            }
            "text-rendering" => {
                if !matches!(val.trim(), "auto" | "optimizeSpeed" | "optimizeLegibility" | "geometricPrecision") {
                    return Err(format!("invalid text-rendering {val:?} on <svg>"));
                }
            }
            "style" => validate_safe_style("svg", val, &mut pres)?,
            "id" | "class" | "role" | "aria-hidden" | "aria-label" | "focusable" | "tabindex" => {}
            _ if name.starts_with("data-") => {}
            other => match validate_presentation("svg", other, val)? {
                Pres::Emit(k, v) => pres.push((k, v)),
                Pres::Drop => {}
                Pres::Unknown => return Err(format!("disallowed attribute {other:?} on <svg>")),
            },
        }
    }
    let [min_x, min_y, w, h] = match (view_box, width, height) {
        (Some(vb), _, _) => vb,
        (None, Some(w), Some(h)) => [0.0, 0.0, w, h],
        _ => return Err("SVG needs viewBox or numeric width and height".into()),
    };
    let side = w.max(h);
    let tx = -min_x + (side - w) * 0.5;
    let ty = -min_y + (side - h) * 0.5;
    Ok((side, tx, ty, pres))
}

pub(super) fn validate_group(attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, String> {
    let mut out = Vec::new();
    for (name, val) in attrs {
        check_forbidden_attr("g", name, val)?;
        match *name {
            "transform" => out.push(("transform", validate_transform("g", val)?)),
            "style" => validate_safe_style("g", val, &mut out)?,
            "id" | "class" | "role" | "aria-hidden" | "aria-label" => {}
            _ if name.starts_with("data-") => {}
            other => match validate_presentation("g", other, val)? {
                Pres::Emit(k, v) => out.push((k, v)),
                Pres::Drop => {}
                Pres::Unknown => return Err(format!("disallowed attribute {other:?} on <g>")),
            },
        }
    }
    Ok(out)
}

pub(super) fn validate_shape(kind: ShapeKind, attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, String> {
    let tag = kind.tag();
    let mut out = Vec::new();
    let mut has_d = false;
    let mut has_points = false;
    for (name, val) in attrs {
        if *name == "xmlns" {
            if val != "http://www.w3.org/2000/svg" {
                return Err(format!("invalid xmlns {val:?} on <{tag}>"));
            }
            continue;
        }
        check_forbidden_attr(tag, name, val)?;
        match (kind, *name) {
            (ShapeKind::Path, "d") => {
                out.push(("d", validate_path_data(val)?));
                has_d = true;
            }
            (ShapeKind::Circle, "cx") => out.push(("cx", fmt_num(parse_coord(tag, "cx", val)?))),
            (ShapeKind::Circle, "cy") => out.push(("cy", fmt_num(parse_coord(tag, "cy", val)?))),
            (ShapeKind::Circle, "r") => out.push(("r", fmt_num(parse_nonneg(tag, "r", val)?))),
            (ShapeKind::Ellipse, "cx") => out.push(("cx", fmt_num(parse_coord(tag, "cx", val)?))),
            (ShapeKind::Ellipse, "cy") => out.push(("cy", fmt_num(parse_coord(tag, "cy", val)?))),
            (ShapeKind::Ellipse, "rx") => out.push(("rx", fmt_num(parse_nonneg(tag, "rx", val)?))),
            (ShapeKind::Ellipse, "ry") => out.push(("ry", fmt_num(parse_nonneg(tag, "ry", val)?))),
            (ShapeKind::Rect, "x") => out.push(("x", fmt_num(parse_coord(tag, "x", val)?))),
            (ShapeKind::Rect, "y") => out.push(("y", fmt_num(parse_coord(tag, "y", val)?))),
            (ShapeKind::Rect, "width") => out.push(("width", fmt_num(parse_nonneg(tag, "width", val)?))),
            (ShapeKind::Rect, "height") => out.push(("height", fmt_num(parse_nonneg(tag, "height", val)?))),
            (ShapeKind::Rect, "rx") => out.push(("rx", fmt_num(parse_nonneg(tag, "rx", val)?))),
            (ShapeKind::Rect, "ry") => out.push(("ry", fmt_num(parse_nonneg(tag, "ry", val)?))),
            (ShapeKind::Line, "x1") => out.push(("x1", fmt_num(parse_coord(tag, "x1", val)?))),
            (ShapeKind::Line, "y1") => out.push(("y1", fmt_num(parse_coord(tag, "y1", val)?))),
            (ShapeKind::Line, "x2") => out.push(("x2", fmt_num(parse_coord(tag, "x2", val)?))),
            (ShapeKind::Line, "y2") => out.push(("y2", fmt_num(parse_coord(tag, "y2", val)?))),
            (ShapeKind::Polyline | ShapeKind::Polygon, "points") => {
                out.push(("points", validate_points(tag, val)?));
                has_points = true;
            }
            (_, "transform") => out.push(("transform", validate_transform(tag, val)?)),
            (_, "pathLength") => {
                let n = parse_finite(val).map_err(|e| format!("invalid pathLength on <{tag}>: {e}"))?;
                if !(0.0..=1e6).contains(&n) || n == 0.0 {
                    return Err(format!("pathLength {val:?} out of bounds on <{tag}>"));
                }
            }
            (_, "style") => validate_safe_style(tag, val, &mut out)?,
            (_, "id" | "class" | "role" | "aria-hidden" | "aria-label") => {}
            (_, n) if n.starts_with("data-") => {}
            (_, other) => match validate_presentation(tag, other, val)? {
                Pres::Emit(k, v) => out.push((k, v)),
                Pres::Drop => {}
                Pres::Unknown => return Err(format!("disallowed attribute {other:?} on <{tag}>")),
            },
        }
    }
    if kind == ShapeKind::Path && !has_d {
        return Err("<path> missing d attribute".into());
    }
    if matches!(kind, ShapeKind::Polyline | ShapeKind::Polygon) && !has_points {
        return Err(format!("<{tag}> missing points attribute"));
    }
    Ok(out)
}

pub(super) fn validate_anim(kind: AnimKind, attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, String> {
    let tag = kind.tag();
    let mut out = Vec::new();
    let mut has_attr = false;
    let mut has_dur = false;
    let mut has_values_or_to = false;
    for (name, val) in attrs {
        check_forbidden_attr(tag, name, val)?;
        match *name {
            "attributeName" => {
                let ok = match kind {
                    AnimKind::AnimateTransform => val == "transform",
                    _ => matches!(
                        val.as_str(),
                        "r" | "cx"
                            | "cy"
                            | "rx"
                            | "ry"
                            | "x"
                            | "y"
                            | "width"
                            | "height"
                            | "x1"
                            | "y1"
                            | "x2"
                            | "y2"
                            | "opacity"
                            | "fill-opacity"
                            | "stroke-opacity"
                            | "stroke-width"
                            | "stroke-dashoffset"
                    ),
                };
                if !ok {
                    return Err(format!("disallowed attributeName {val:?} on <{tag}>"));
                }
                has_attr = true;
                out.push(("attributeName", val.clone()));
            }
            "type" if kind == AnimKind::AnimateTransform => {
                if !matches!(val.as_str(), "translate" | "scale" | "rotate" | "skewX" | "skewY") {
                    return Err(format!("disallowed type {val:?} on <{tag}>"));
                }
                out.push(("type", val.clone()));
            }
            "dur" => {
                let secs = parse_clock(val)?;
                if !(MIN_DUR_SECS..=MAX_CLOCK_SECS).contains(&secs) {
                    return Err(format!("dur {val:?} out of bounds on <{tag}>"));
                }
                has_dur = true;
                out.push(("dur", format!("{secs}s")));
            }
            "begin" => {
                let secs = parse_clock(val)?;
                if !(-MAX_CLOCK_SECS..=MAX_CLOCK_SECS).contains(&secs) {
                    return Err(format!("begin {val:?} out of bounds on <{tag}>"));
                }
                out.push(("begin", format!("{secs}s")));
            }
            "repeatCount" => {
                let t = val.trim();
                if t == "indefinite" {
                    out.push(("repeatCount", "indefinite".to_owned()));
                } else {
                    let n = parse_finite(t)?;
                    if !(0.001..=MAX_REPEAT_COUNT).contains(&n) {
                        return Err(format!("invalid repeatCount {val:?} on <{tag}>"));
                    }
                    out.push(("repeatCount", fmt_num(n)));
                }
            }
            "calcMode" => {
                if !matches!(val.as_str(), "linear" | "discrete" | "spline" | "paced") {
                    return Err(format!("invalid calcMode {val:?} on <{tag}>"));
                }
                out.push(("calcMode", val.clone()));
            }
            "fill" => {
                if !matches!(val.as_str(), "freeze" | "remove") {
                    return Err(format!("invalid fill {val:?} on <{tag}>"));
                }
                out.push(("fill", val.clone()));
            }
            "additive" => {
                if !matches!(val.as_str(), "replace" | "sum") {
                    return Err(format!("invalid additive {val:?} on <{tag}>"));
                }
                out.push(("additive", val.clone()));
            }
            "values" => {
                let mut clean = Vec::new();
                for (raw_count, p) in val.split(';').enumerate() {
                    if raw_count > MAX_KEYFRAMES {
                        return Err(format!("values count out of bounds on <{tag}>"));
                    }
                    let p = p.trim();
                    if p.is_empty() {
                        continue;
                    }
                    if clean.len() >= MAX_KEYFRAMES {
                        return Err(format!("values count out of bounds on <{tag}>"));
                    }
                    let nums = parse_numbers(p, 3)?;
                    if nums.is_empty() || nums.iter().any(|&n| n.abs() > 36_000.0) {
                        return Err(format!("invalid keyframe value {p:?} on <{tag}>"));
                    }
                    clean.push(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "));
                }
                if clean.is_empty() {
                    return Err(format!("empty values on <{tag}>"));
                }
                has_values_or_to = true;
                out.push(("values", clean.join(";")));
            }
            "from" | "to" | "by" => {
                let nums = parse_numbers(val, 3)?;
                if nums.is_empty() || nums.iter().any(|&n| n.abs() > 36_000.0) {
                    return Err(format!("invalid {name} {val:?} on <{tag}>"));
                }
                let key = match *name {
                    "from" => "from",
                    "to" => "to",
                    _ => "by",
                };
                if key != "from" {
                    has_values_or_to = true;
                }
                out.push((key, nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" ")));
            }
            "keyTimes" => {
                let parts: Vec<&str> = val.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
                if parts.is_empty() || parts.len() > MAX_KEYFRAMES {
                    return Err(format!("keyTimes count out of bounds on <{tag}>"));
                }
                let mut clean = Vec::with_capacity(parts.len());
                for p in parts {
                    let n = parse_finite(p)?;
                    if !(0.0..=1.0).contains(&n) {
                        return Err(format!("keyTimes value {p:?} out of [0, 1] on <{tag}>"));
                    }
                    clean.push(fmt_num(n));
                }
                out.push(("keyTimes", clean.join(";")));
            }
            "keySplines" => {
                let parts: Vec<&str> = val.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
                if parts.is_empty() || parts.len() > MAX_KEYFRAMES {
                    return Err(format!("keySplines count out of bounds on <{tag}>"));
                }
                let mut clean = Vec::with_capacity(parts.len());
                for p in parts {
                    let nums = parse_numbers(p, 4)?;
                    if nums.len() != 4
                        || !(0.0..=1.0).contains(&nums[0])
                        || !(0.0..=1.0).contains(&nums[2])
                        || !(MIN_SPLINE_Y..=MAX_SPLINE_Y).contains(&nums[1])
                        || !(MIN_SPLINE_Y..=MAX_SPLINE_Y).contains(&nums[3])
                    {
                        return Err(format!("invalid keySplines segment {p:?} on <{tag}>"));
                    }
                    clean.push(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "));
                }
                out.push(("keySplines", clean.join(";")));
            }
            "id" | "attributeType" => {}
            other => return Err(format!("disallowed attribute {other:?} on <{tag}>")),
        }
    }
    if !has_attr || !has_values_or_to || (kind != AnimKind::Set && !has_dur) {
        return Err(format!("incomplete <{tag}> element"));
    }
    Ok(out)
}

fn validate_presentation(tag: &str, name: &str, val: &str) -> Result<Pres, String> {
    let Some(&canon) = PRESENTATION_ATTRS.iter().find(|&&k| k == name) else {
        return Ok(Pres::Unknown);
    };
    let v = match canon {
        "fill" | "stroke" | "color" => validate_paint(tag, canon, val)?,
        "stroke-width" => fmt_num(parse_length(val, 0.0, MAX_STROKE_WIDTH).map_err(|e| format!("invalid stroke-width on <{tag}>: {e}"))?),
        "stroke-miterlimit" => fmt_num(parse_length(val, 1.0, 100.0).map_err(|e| format!("invalid stroke-miterlimit on <{tag}>: {e}"))?),
        "stroke-linecap" => match val.trim() {
            "butt" | "round" | "square" => val.trim().to_owned(),
            _ => return Err(format!("invalid stroke-linecap {val:?} on <{tag}>")),
        },
        "stroke-linejoin" => match val.trim() {
            "miter" | "round" | "bevel" => val.trim().to_owned(),
            _ => return Err(format!("invalid stroke-linejoin {val:?} on <{tag}>")),
        },
        "stroke-dasharray" => validate_dasharray(tag, val)?,
        "stroke-dashoffset" => fmt_num(parse_length(val, -1024.0, 1024.0).map_err(|e| format!("invalid stroke-dashoffset on <{tag}>: {e}"))?),
        "fill-rule" => match val.trim() {
            "nonzero" | "evenodd" => val.trim().to_owned(),
            _ => return Err(format!("invalid fill-rule {val:?} on <{tag}>")),
        },
        "clip-rule" => match val.trim() {
            "nonzero" | "evenodd" => val.trim().to_owned(),
            _ => return Err(format!("invalid clip-rule {val:?} on <{tag}>")),
        },
        "opacity" | "fill-opacity" | "stroke-opacity" => fmt_num(parse_opacity(val).map_err(|e| format!("invalid {canon} on <{tag}>: {e}"))?),
        "paint-order" => validate_paint_order(tag, val)?,
        "shape-rendering" => match val.trim() {
            "auto" | "optimizeSpeed" | "crispEdges" | "geometricPrecision" => val.trim().to_owned(),
            _ => return Err(format!("invalid shape-rendering {val:?} on <{tag}>")),
        },
        "vector-effect" => match val.trim() {
            "none" | "non-scaling-stroke" => val.trim().to_owned(),
            _ => return Err(format!("invalid vector-effect {val:?} on <{tag}>")),
        },
        "visibility" => match val.trim() {
            "visible" | "hidden" | "collapse" => val.trim().to_owned(),
            _ => return Err(format!("invalid visibility {val:?} on <{tag}>")),
        },
        "display" => match val.trim() {
            "none" => "none".to_owned(),
            "inline" | "block" | "initial" => return Ok(Pres::Drop),
            _ => return Err(format!("invalid display {val:?} on <{tag}>")),
        },
        _ => return Ok(Pres::Unknown),
    };
    Ok(Pres::Emit(canon, v))
}

fn validate_safe_style(tag: &str, style: &str, out: &mut Vec<(&'static str, String)>) -> Result<(), String> {
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((prop, val)) = decl.split_once(':') else {
            return Err(format!("malformed style declaration {decl:?} on <{tag}>"));
        };
        let prop = prop.trim();
        let val = val.trim();
        // CSS custom properties (such as `color: var(--v2-icon-icon-accent)`) are stripped safely:
        // the host tints `currentColor` via the element's tone/color.
        if prop == "color" && is_safe_css_var(val) {
            continue;
        }
        if tag == "svg" && prop == "overflow" && matches!(val, "visible" | "hidden" | "auto" | "scroll") {
            continue;
        }
        match validate_presentation(tag, prop, val)? {
            Pres::Emit(k, v) => {
                if !out.iter().any(|(existing, _)| *existing == k) {
                    out.push((k, v));
                }
            }
            Pres::Drop => {}
            Pres::Unknown => return Err(format!("disallowed CSS property {prop:?} in style on <{tag}>")),
        }
    }
    Ok(())
}

fn is_safe_css_var(val: &str) -> bool {
    let Some(inner) = val.strip_prefix("var(--").and_then(|s| s.strip_suffix(')')) else {
        return false;
    };
    !inner.is_empty() && inner.len() <= 64 && inner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn validate_paint(tag: &str, attr: &str, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    match t {
        "none" | "currentColor" | "inherit" => Ok(t.to_owned()),
        "transparent" => Ok("none".to_owned()),
        "black" => Ok("#000000".to_owned()),
        "white" => Ok("#ffffff".to_owned()),
        _ if t.starts_with('#') => {
            let hex = &t[1..];
            if matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(t.to_owned())
            } else {
                Err(format!("invalid hex color {t:?} in {attr} on <{tag}>"))
            }
        }
        _ if t.starts_with("rgb(") && t.ends_with(')') => parse_rgb_hex(tag, attr, &t[4..t.len() - 1], false),
        _ if t.starts_with("rgba(") && t.ends_with(')') => parse_rgb_hex(tag, attr, &t[5..t.len() - 1], true),
        _ => Err(format!("disallowed paint value {t:?} in {attr} on <{tag}>")),
    }
}

fn parse_rgb_hex(tag: &str, attr: &str, args: &str, with_alpha: bool) -> Result<String, String> {
    let nums = parse_numbers(args, 4)?;
    let want = if with_alpha { 4 } else { 3 };
    if nums.len() != want {
        return Err(format!("invalid rgb/rgba color in {attr} on <{tag}>"));
    }
    let ch = |v: f32| -> Result<u8, String> {
        if !(0.0..=255.0).contains(&v) {
            return Err(format!("rgb channel {v} out of [0, 255] in {attr} on <{tag}>"));
        }
        Ok(v.round() as u8)
    };
    let (r, g, b) = (ch(nums[0])?, ch(nums[1])?, ch(nums[2])?);
    if with_alpha {
        if !(0.0..=1.0).contains(&nums[3]) {
            return Err(format!("rgba alpha out of [0, 1] in {attr} on <{tag}>"));
        }
        let a = (nums[3] * 255.0).round() as u8;
        Ok(format!("#{r:02x}{g:02x}{b:02x}{a:02x}"))
    } else {
        Ok(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

fn validate_dasharray(tag: &str, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t == "none" {
        return Ok("none".to_owned());
    }
    let mut vals = Vec::new();
    let mut sum = 0.0f32;
    for part in t.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|s| !s.is_empty()) {
        if vals.len() >= MAX_DASH_ENTRIES {
            return Err(format!("stroke-dasharray exceeds {MAX_DASH_ENTRIES} entries on <{tag}>"));
        }
        let v = parse_length(part, 0.0, 1024.0).map_err(|e| format!("invalid stroke-dasharray on <{tag}>: {e}"))?;
        sum += v;
        vals.push(fmt_num(v));
    }
    if vals.is_empty() || sum < MIN_DASH_SUM {
        return Err(format!("stroke-dasharray {raw:?} invalid or too fine on <{tag}>"));
    }
    Ok(vals.join(" "))
}

fn validate_paint_order(tag: &str, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    if t == "normal" {
        return Ok("normal".to_owned());
    }
    let mut kept: Vec<&str> = Vec::new();
    for tok in t.split_ascii_whitespace() {
        if !matches!(tok, "fill" | "stroke" | "markers") || kept.contains(&tok) {
            return Err(format!("invalid paint-order {raw:?} on <{tag}>"));
        }
        kept.push(tok);
    }
    if kept.is_empty() {
        return Err(format!("empty paint-order on <{tag}>"));
    }
    Ok(kept.join(" "))
}

fn validate_preserve_aspect_ratio(raw: &str) -> Result<(), String> {
    let parts: Vec<&str> = raw.split_ascii_whitespace().collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err(format!("invalid preserveAspectRatio {raw:?} on <svg>"));
    }
    if parts[0] == "none" {
        return if parts.len() == 1 { Ok(()) } else { Err(format!("invalid preserveAspectRatio {raw:?} on <svg>")) };
    }
    let align_ok = matches!(
        parts[0],
        "xMinYMin" | "xMidYMin" | "xMaxYMin" | "xMinYMid" | "xMidYMid" | "xMaxYMid" | "xMinYMax" | "xMidYMax" | "xMaxYMax"
    );
    let meet_ok = parts.get(1).is_none_or(|&m| matches!(m, "meet" | "slice"));
    if align_ok && meet_ok {
        Ok(())
    } else {
        Err(format!("invalid preserveAspectRatio {raw:?} on <svg>"))
    }
}

fn parse_root_dim(attr: &str, raw: &str) -> Result<Option<f32>, String> {
    let t = raw.trim();
    if let Some(pct) = t.strip_suffix('%') {
        let p = parse_finite(pct).map_err(|e| format!("invalid {attr} on <svg>: {e}"))?;
        if !(0.0..=1000.0).contains(&p) || p == 0.0 {
            return Err(format!("{attr} percentage {raw:?} out of bounds on <svg>"));
        }
        return Ok(None);
    }
    parse_length(t, MIN_VIEW_DIM, MAX_VIEW_DIM).map(Some).map_err(|e| format!("invalid {attr} on <svg>: {e}"))
}

fn validate_path_data(d: &str) -> Result<String, String> {
    if d.len() > MAX_PATH_BYTES {
        return Err(format!("SVG path data exceeds {MAX_PATH_BYTES} byte limit"));
    }
    let mut commands = 0usize;
    for b in d.bytes() {
        if matches!(b, b'M' | b'm' | b'L' | b'l' | b'H' | b'h' | b'V' | b'v' | b'C' | b'c' | b'S' | b's' | b'Q' | b'q' | b'T' | b't' | b'A' | b'a' | b'Z' | b'z') {
            commands += 1;
            if commands > MAX_PATH_COMMANDS {
                return Err(format!("SVG path exceeds {MAX_PATH_COMMANDS} command limit"));
            }
        } else if !matches!(b, b'0'..=b'9' | b'e' | b'E' | b'.' | b'+' | b'-' | b',' | b' ' | b'\t' | b'\r' | b'\n') {
            return Err("invalid character in SVG path d attribute".into());
        }
    }
    if commands == 0 {
        return Err("empty SVG path d attribute".into());
    }
    Ok(d.trim().to_owned())
}

fn validate_points(tag: &str, raw: &str) -> Result<String, String> {
    if raw.len() > MAX_PATH_BYTES {
        return Err(format!("<{tag}> points exceed {MAX_PATH_BYTES} byte limit"));
    }
    let nums = parse_numbers(raw, MAX_POINTS_NUMBERS)?;
    if nums.len() < 4 || nums.len() % 2 != 0 {
        return Err(format!("invalid points list on <{tag}>"));
    }
    Ok(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "))
}

fn validate_transform(tag: &str, raw: &str) -> Result<String, String> {
    let mut ops = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        if ops.len() >= MAX_TRANSFORM_OPS {
            return Err(format!("too many transform operations on <{tag}>"));
        }
        let Some((name, after)) = rest.split_once('(') else {
            return Err(format!("malformed transform {raw:?} on <{tag}>"));
        };
        let Some((args, tail)) = after.split_once(')') else {
            return Err(format!("unclosed transform {raw:?} on <{tag}>"));
        };
        let op = name.trim().trim_start_matches(',').trim();
        let nums = parse_numbers(args, 6)?;
        let valid_arity = match op {
            "matrix" => nums.len() == 6 && nums[..4].iter().all(|v| (-MAX_SCALE..=MAX_SCALE).contains(v)) && nums[4..].iter().all(|v| (-MAX_COORD..=MAX_COORD).contains(v)),
            "translate" => matches!(nums.len(), 1 | 2) && nums.iter().all(|v| (-MAX_COORD..=MAX_COORD).contains(v)),
            "scale" => matches!(nums.len(), 1 | 2) && nums.iter().all(|v| (-MAX_SCALE..=MAX_SCALE).contains(v)),
            "rotate" => matches!(nums.len(), 1 | 3) && nums[0].abs() <= 36_000.0 && nums[1..].iter().all(|v| (-MAX_COORD..=MAX_COORD).contains(v)),
            "skewX" | "skewY" => nums.len() == 1 && (-MAX_SKEW_DEG..=MAX_SKEW_DEG).contains(&nums[0]),
            _ => return Err(format!("disallowed transform function {op:?} on <{tag}>")),
        };
        if !valid_arity {
            return Err(format!("invalid transform {op:?} arguments on <{tag}>"));
        }
        let formatted = nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" ");
        ops.push(format!("{op}({formatted})"));
        rest = tail.trim().trim_start_matches(',').trim();
    }
    Ok(ops.join(" "))
}

fn parse_numbers(s: &str, max: usize) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = ' ';
    for c in s.chars() {
        let starts = (c == '-' || c == '+') && !matches!(prev, 'e' | 'E');
        if c == ',' || c.is_whitespace() || (starts && !cur.is_empty()) {
            if !cur.is_empty() {
                if out.len() >= max {
                    return Err("too many numeric arguments".into());
                }
                out.push(parse_finite(&cur)?);
                cur.clear();
            }
        }
        if !(c == ',' || c.is_whitespace()) {
            cur.push(c);
        }
        prev = c;
    }
    if !cur.is_empty() {
        if out.len() >= max {
            return Err("too many numeric arguments".into());
        }
        out.push(parse_finite(&cur)?);
    }
    Ok(out)
}

fn parse_finite(s: &str) -> Result<f32, String> {
    let v = s.trim().parse::<f32>().map_err(|_| format!("invalid number {s:?}"))?;
    if !v.is_finite() {
        return Err(format!("non-finite number {s:?}"));
    }
    Ok(v)
}

fn parse_coord(tag: &str, attr: &str, s: &str) -> Result<f32, String> {
    let v = parse_finite(s.trim().trim_end_matches("px")).map_err(|e| format!("invalid {attr} on <{tag}>: {e}"))?;
    if !(-MAX_COORD..=MAX_COORD).contains(&v) {
        return Err(format!("{attr} coordinate {s:?} out of bounds on <{tag}>"));
    }
    Ok(v)
}

fn parse_nonneg(tag: &str, attr: &str, s: &str) -> Result<f32, String> {
    let v = parse_coord(tag, attr, s)?;
    if v < 0.0 {
        return Err(format!("negative {attr} {s:?} on <{tag}>"));
    }
    Ok(v)
}

fn parse_length(s: &str, min: f32, max: f32) -> Result<f32, String> {
    let v = parse_finite(s.trim().trim_end_matches("px"))?;
    if !(min..=max).contains(&v) {
        return Err(format!("dimension {s:?} out of [{min}, {max}]"));
    }
    Ok(v)
}

fn parse_opacity(s: &str) -> Result<f32, String> {
    let t = s.trim();
    let v = if let Some(p) = t.strip_suffix('%') { parse_finite(p)? / 100.0 } else { parse_finite(t)? };
    Ok(v.clamp(0.0, 1.0))
}

fn parse_clock(s: &str) -> Result<f32, String> {
    let t = s.trim();
    if let Some(ms) = t.strip_suffix("ms") {
        Ok(parse_finite(ms)? / 1000.0)
    } else if let Some(min) = t.strip_suffix("min") {
        Ok(parse_finite(min)? * 60.0)
    } else if let Some(h) = t.strip_suffix('h') {
        Ok(parse_finite(h)? * 3600.0)
    } else if let Some(sec) = t.strip_suffix('s') {
        parse_finite(sec)
    } else {
        parse_finite(t)
    }
}

fn fmt_num(v: f32) -> String {
    if v == 0.0 { "0".to_owned() } else { format!("{v}") }
}
