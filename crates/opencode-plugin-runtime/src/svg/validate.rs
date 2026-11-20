//! Attribute and value validators for [super::sanitize].

use super::{AnimKind, MAX_CLOCK_SECS, MAX_KEYFRAMES, MAX_PATH_BYTES, MAX_PATH_COMMANDS, MAX_POINTS_NUMBERS, MAX_TRANSFORM_OPS, MAX_VIEW_DIM, MIN_DUR_SECS, MIN_VIEW_DIM, PRESENTATION_ATTRS, ShapeKind};

fn check_forbidden_attr(name: &str, val: &str) -> Result<(), &'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("on") || matches!(lower.as_str(), "href" | "xlink:href" | "src" | "filter" | "clip-path" | "mask") {
        return Err("disallowed SVG attribute");
    }
    let vlower = val.to_ascii_lowercase();
    for bad in ["url(", "javascript:", "data:", "http:", "https:", "file:", "expression(", "@import"] {
        if vlower.contains(bad) {
            return Err("disallowed reference in SVG attribute");
        }
    }
    Ok(())
}

pub(super) fn validate_root(attrs: &[(&str, String)]) -> Result<(f32, f32, f32, Vec<(&'static str, String)>), &'static str> {
    let mut view_box: Option<[f32; 4]> = None;
    let mut width: Option<f32> = None;
    let mut height: Option<f32> = None;
    let mut pres = Vec::new();
    for (name, val) in attrs {
        if *name == "xmlns" {
            if val != "http://www.w3.org/2000/svg" {
                return Err("invalid SVG xmlns");
            }
            continue;
        }
        if *name == "xmlns:xlink" {
            continue;
        }
        check_forbidden_attr(name, val)?;
        match *name {
            "viewBox" => {
                let nums = parse_numbers(val, 8)?;
                if nums.len() != 4 {
                    return Err("invalid viewBox");
                }
                let [x, y, w, h] = [nums[0], nums[1], nums[2], nums[3]];
                if !(-MAX_VIEW_DIM..=MAX_VIEW_DIM).contains(&x)
                    || !(-MAX_VIEW_DIM..=MAX_VIEW_DIM).contains(&y)
                    || !(MIN_VIEW_DIM..=MAX_VIEW_DIM).contains(&w)
                    || !(MIN_VIEW_DIM..=MAX_VIEW_DIM).contains(&h)
                {
                    return Err("viewBox out of bounds");
                }
                view_box = Some([x, y, w, h]);
            }
            "width" => width = Some(parse_length(val, MIN_VIEW_DIM, MAX_VIEW_DIM)?),
            "height" => height = Some(parse_length(val, MIN_VIEW_DIM, MAX_VIEW_DIM)?),
            "id" | "class" | "role" | "aria-hidden" | "focusable" | "preserveAspectRatio" => {}
            _ if name.starts_with("data-") => {}
            other => {
                if let Some((k, v)) = validate_presentation(other, val)? {
                    pres.push((k, v));
                } else {
                    return Err("disallowed attribute on <svg>");
                }
            }
        }
    }
    let [min_x, min_y, w, h] = match (view_box, width, height) {
        (Some(vb), _, _) => vb,
        (None, Some(w), Some(h)) => [0.0, 0.0, w, h],
        _ => return Err("SVG needs viewBox or width and height"),
    };
    let side = w.max(h);
    let tx = -min_x + (side - w) * 0.5;
    let ty = -min_y + (side - h) * 0.5;
    Ok((side, tx, ty, pres))
}

pub(super) fn validate_group(attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, &'static str> {
    let mut out = Vec::new();
    for (name, val) in attrs {
        check_forbidden_attr(name, val)?;
        match *name {
            "transform" => out.push(("transform", validate_transform(val)?)),
            "display" => {
                if val == "none" {
                    out.push(("display", "none".to_owned()));
                } else if !matches!(val.as_str(), "inline" | "block" | "initial") {
                    return Err("invalid display value");
                }
            }
            "style" => validate_safe_style(val, &mut out)?,
            "id" | "class" => {}
            _ if name.starts_with("data-") => {}
            other => {
                if let Some(pair) = validate_presentation(other, val)? {
                    out.push(pair);
                } else {
                    return Err("disallowed attribute on <g>");
                }
            }
        }
    }
    Ok(out)
}

pub(super) fn validate_shape(kind: ShapeKind, attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, &'static str> {
    let mut out = Vec::new();
    let mut has_d = false;
    let mut has_points = false;
    for (name, val) in attrs {
        check_forbidden_attr(name, val)?;
        match (kind, *name) {
            (ShapeKind::Path, "d") => {
                out.push(("d", validate_path_data(val)?));
                has_d = true;
            }
            (ShapeKind::Circle, "cx") => out.push(("cx", fmt_num(parse_coord(val)?))),
            (ShapeKind::Circle, "cy") => out.push(("cy", fmt_num(parse_coord(val)?))),
            (ShapeKind::Circle, "r") => out.push(("r", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Ellipse, "cx") => out.push(("cx", fmt_num(parse_coord(val)?))),
            (ShapeKind::Ellipse, "cy") => out.push(("cy", fmt_num(parse_coord(val)?))),
            (ShapeKind::Ellipse, "rx") => out.push(("rx", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Ellipse, "ry") => out.push(("ry", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Rect, "x") => out.push(("x", fmt_num(parse_coord(val)?))),
            (ShapeKind::Rect, "y") => out.push(("y", fmt_num(parse_coord(val)?))),
            (ShapeKind::Rect, "width") => out.push(("width", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Rect, "height") => out.push(("height", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Rect, "rx") => out.push(("rx", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Rect, "ry") => out.push(("ry", fmt_num(parse_nonneg(val)?))),
            (ShapeKind::Line, "x1") => out.push(("x1", fmt_num(parse_coord(val)?))),
            (ShapeKind::Line, "y1") => out.push(("y1", fmt_num(parse_coord(val)?))),
            (ShapeKind::Line, "x2") => out.push(("x2", fmt_num(parse_coord(val)?))),
            (ShapeKind::Line, "y2") => out.push(("y2", fmt_num(parse_coord(val)?))),
            (ShapeKind::Polyline | ShapeKind::Polygon, "points") => {
                out.push(("points", validate_points(val)?));
                has_points = true;
            }
            (_, "transform") => out.push(("transform", validate_transform(val)?)),
            (_, "shape-rendering") => {
                if matches!(val.as_str(), "auto" | "optimizeSpeed" | "crispEdges" | "geometricPrecision") {
                    out.push(("shape-rendering", val.clone()));
                } else {
                    return Err("invalid shape-rendering");
                }
            }
            (_, "style") => validate_safe_style(val, &mut out)?,
            (_, "id" | "class" | "vector-effect" | "pathLength") => {}
            (_, n) if n.starts_with("data-") => {}
            (_, other) => {
                if let Some(pair) = validate_presentation(other, val)? {
                    out.push(pair);
                } else {
                    return Err("disallowed attribute on SVG shape");
                }
            }
        }
    }
    if kind == ShapeKind::Path && !has_d {
        return Err("<path> missing d attribute");
    }
    if matches!(kind, ShapeKind::Polyline | ShapeKind::Polygon) && !has_points {
        return Err("poly shape missing points attribute");
    }
    Ok(out)
}

pub(super) fn validate_anim(kind: AnimKind, attrs: &[(&str, String)]) -> Result<Vec<(&'static str, String)>, &'static str> {
    let mut out = Vec::new();
    let mut has_attr = false;
    let mut has_dur = false;
    let mut has_values_or_to = false;
    for (name, val) in attrs {
        check_forbidden_attr(name, val)?;
        match *name {
            "attributeName" => {
                let ok = match kind {
                    AnimKind::AnimateTransform => val == "transform",
                    _ => matches!(
                        val.as_str(),
                        "r" | "cx" | "cy" | "rx" | "ry" | "x" | "y" | "width" | "height"
                            | "x1" | "y1" | "x2" | "y2" | "opacity" | "fill-opacity" | "stroke-opacity" | "stroke-width"
                    ),
                };
                if !ok {
                    return Err("disallowed animated attributeName");
                }
                has_attr = true;
                out.push(("attributeName", val.clone()));
            }
            "type" if kind == AnimKind::AnimateTransform => {
                if !matches!(val.as_str(), "translate" | "scale" | "rotate" | "skewX" | "skewY") {
                    return Err("disallowed animateTransform type");
                }
                out.push(("type", val.clone()));
            }
            "dur" => {
                let secs = parse_clock(val)?;
                if !(MIN_DUR_SECS..=MAX_CLOCK_SECS).contains(&secs) {
                    return Err("animation dur out of bounds");
                }
                has_dur = true;
                out.push(("dur", format!("{secs}s")));
            }
            "begin" => {
                let secs = parse_clock(val)?;
                if !(0.0..=MAX_CLOCK_SECS).contains(&secs) {
                    return Err("animation begin out of bounds");
                }
                out.push(("begin", format!("{secs}s")));
            }
            "repeatCount" => {
                let t = val.trim();
                if t == "indefinite" {
                    out.push(("repeatCount", "indefinite".to_owned()));
                } else {
                    let n = parse_finite(t)?;
                    if !(0.001..=10_000.0).contains(&n) {
                        return Err("invalid repeatCount");
                    }
                    out.push(("repeatCount", fmt_num(n)));
                }
            }
            "calcMode" => {
                if !matches!(val.as_str(), "linear" | "discrete" | "spline" | "paced") {
                    return Err("invalid calcMode");
                }
                out.push(("calcMode", val.clone()));
            }
            "fill" => {
                if !matches!(val.as_str(), "freeze" | "remove") {
                    return Err("invalid animation fill");
                }
                out.push(("fill", val.clone()));
            }
            "additive" => {
                if !matches!(val.as_str(), "replace" | "sum") {
                    return Err("invalid animation additive");
                }
                out.push(("additive", val.clone()));
            }
            "values" => {
                let parts: Vec<&str> = val.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
                if parts.is_empty() || parts.len() > MAX_KEYFRAMES {
                    return Err("animation values count out of bounds");
                }
                let mut clean = Vec::with_capacity(parts.len());
                for p in parts {
                    let nums = parse_numbers(p, 3)?;
                    if nums.is_empty() {
                        return Err("invalid animation keyframe value");
                    }
                    clean.push(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "));
                }
                has_values_or_to = true;
                out.push(("values", clean.join(";")));
            }
            "from" | "to" => {
                let nums = parse_numbers(val, 3)?;
                if nums.is_empty() {
                    return Err("invalid animation endpoint");
                }
                let key = if *name == "from" { "from" } else { "to" };
                if key == "to" {
                    has_values_or_to = true;
                }
                out.push((key, nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" ")));
            }
            "keyTimes" => {
                let parts: Vec<&str> = val.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
                if parts.is_empty() || parts.len() > MAX_KEYFRAMES {
                    return Err("keyTimes count out of bounds");
                }
                let mut clean = Vec::with_capacity(parts.len());
                for p in parts {
                    let n = parse_finite(p)?;
                    if !(0.0..=1.0).contains(&n) {
                        return Err("keyTimes value out of [0, 1]");
                    }
                    clean.push(fmt_num(n));
                }
                out.push(("keyTimes", clean.join(";")));
            }
            "keySplines" => {
                let parts: Vec<&str> = val.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
                if parts.is_empty() || parts.len() > MAX_KEYFRAMES {
                    return Err("keySplines count out of bounds");
                }
                let mut clean = Vec::with_capacity(parts.len());
                for p in parts {
                    let nums = parse_numbers(p, 4)?;
                    if nums.len() != 4 || !(0.0..=1.0).contains(&nums[0]) || !(0.0..=1.0).contains(&nums[2]) {
                        return Err("invalid keySplines segment");
                    }
                    clean.push(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "));
                }
                out.push(("keySplines", clean.join(";")));
            }
            "id" | "attributeType" => {}
            _ => return Err("disallowed attribute on animation element"),
        }
    }
    if !has_attr || !has_values_or_to || (kind != AnimKind::Set && !has_dur) {
        return Err("incomplete animation element");
    }
    Ok(out)
}

fn validate_presentation(name: &str, val: &str) -> Result<Option<(&'static str, String)>, &'static str> {
    let Some(&canon) = PRESENTATION_ATTRS.iter().find(|&&k| k == name) else {
        return Ok(None);
    };
    let v = match canon {
        "fill" | "stroke" => validate_paint(val)?,
        "stroke-width" => fmt_num(parse_length(val, 0.0, 256.0)?),
        "stroke-miterlimit" => fmt_num(parse_length(val, 1.0, 100.0)?),
        "stroke-linecap" => match val.trim() {
            "butt" | "round" | "square" => val.trim().to_owned(),
            _ => return Err("invalid stroke-linecap"),
        },
        "stroke-linejoin" => match val.trim() {
            "miter" | "round" | "bevel" => val.trim().to_owned(),
            _ => return Err("invalid stroke-linejoin"),
        },
        "fill-rule" => match val.trim() {
            "nonzero" | "evenodd" => val.trim().to_owned(),
            _ => return Err("invalid fill-rule"),
        },
        "opacity" | "fill-opacity" | "stroke-opacity" => fmt_num(parse_opacity(val)?),
        _ => return Ok(None),
    };
    Ok(Some((canon, v)))
}

fn validate_safe_style(style: &str, out: &mut Vec<(&'static str, String)>) -> Result<(), &'static str> {
    for decl in style.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((prop, val)) = decl.split_once(':') else {
            return Err("malformed style attribute");
        };
        let prop = prop.trim();
        let val = val.trim();
        // CSS custom properties (such as `color: var(--v2-icon-icon-accent)`) are ignored safely:
        // the host tints `currentColor` via the element's tone/color.
        if prop == "color" && val.starts_with("var(") && val.ends_with(')') {
            continue;
        }
        if let Some(pair) = validate_presentation(prop, val)? {
            if !out.iter().any(|(k, _)| *k == pair.0) {
                out.push(pair);
            }
        }
    }
    Ok(())
}

fn validate_paint(raw: &str) -> Result<String, &'static str> {
    let t = raw.trim();
    match t {
        "none" | "currentColor" => Ok(t.to_owned()),
        "transparent" => Ok("none".to_owned()),
        "black" => Ok("#000000".to_owned()),
        "white" => Ok("#ffffff".to_owned()),
        _ if t.starts_with('#') => {
            let hex = &t[1..];
            if matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(t.to_owned())
            } else {
                Err("invalid hex paint color")
            }
        }
        _ => Err("disallowed SVG paint value"),
    }
}

fn validate_path_data(d: &str) -> Result<String, &'static str> {
    if d.len() > MAX_PATH_BYTES {
        return Err("SVG path data exceeds byte limit");
    }
    let mut commands = 0usize;
    for b in d.bytes() {
        if matches!(b, b'M' | b'm' | b'L' | b'l' | b'H' | b'h' | b'V' | b'v' | b'C' | b'c' | b'S' | b's' | b'Q' | b'q' | b'T' | b't' | b'A' | b'a' | b'Z' | b'z') {
            commands += 1;
            if commands > MAX_PATH_COMMANDS {
                return Err("SVG path exceeds command limit");
            }
        } else if !matches!(b, b'0'..=b'9' | b'e' | b'E' | b'.' | b'+' | b'-' | b',' | b' ' | b'\t' | b'\r' | b'\n') {
            return Err("invalid character in SVG path data");
        }
    }
    if commands == 0 {
        return Err("empty SVG path data");
    }
    Ok(d.trim().to_owned())
}

fn validate_points(raw: &str) -> Result<String, &'static str> {
    if raw.len() > MAX_PATH_BYTES {
        return Err("SVG points exceed byte limit");
    }
    let nums = parse_numbers(raw, MAX_POINTS_NUMBERS)?;
    if nums.len() < 4 || nums.len() % 2 != 0 {
        return Err("invalid SVG points list");
    }
    Ok(nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" "))
}

fn validate_transform(raw: &str) -> Result<String, &'static str> {
    let mut ops = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        if ops.len() >= MAX_TRANSFORM_OPS {
            return Err("too many transform operations");
        }
        let Some((name, after)) = rest.split_once('(') else {
            return Err("malformed transform");
        };
        let Some((args, tail)) = after.split_once(')') else {
            return Err("unclosed transform");
        };
        let op = name.trim().trim_start_matches(',').trim();
        let nums = parse_numbers(args, 6)?;
        let valid_arity = match op {
            "matrix" => nums.len() == 6,
            "translate" | "scale" => matches!(nums.len(), 1 | 2),
            "rotate" => matches!(nums.len(), 1 | 3),
            "skewX" | "skewY" => nums.len() == 1,
            _ => return Err("disallowed transform function"),
        };
        if !valid_arity {
            return Err("invalid transform argument count");
        }
        let formatted = nums.iter().map(|&n| fmt_num(n)).collect::<Vec<_>>().join(" ");
        ops.push(format!("{op}({formatted})"));
        rest = tail.trim().trim_start_matches(',').trim();
    }
    Ok(ops.join(" "))
}

fn parse_numbers(s: &str, max: usize) -> Result<Vec<f32>, &'static str> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = ' ';
    for c in s.chars() {
        let starts = (c == '-' || c == '+') && !matches!(prev, 'e' | 'E');
        if c == ',' || c.is_whitespace() || (starts && !cur.is_empty()) {
            if !cur.is_empty() {
                if out.len() >= max {
                    return Err("too many numeric arguments");
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
            return Err("too many numeric arguments");
        }
        out.push(parse_finite(&cur)?);
    }
    Ok(out)
}

fn parse_finite(s: &str) -> Result<f32, &'static str> {
    let v = s.trim().parse::<f32>().map_err(|_| "invalid number")?;
    if !v.is_finite() {
        return Err("non-finite number");
    }
    Ok(v)
}

fn parse_coord(s: &str) -> Result<f32, &'static str> {
    let v = parse_finite(s.trim().trim_end_matches("px"))?;
    if !(-1e6..=1e6).contains(&v) {
        return Err("coordinate out of bounds");
    }
    Ok(v)
}

fn parse_nonneg(s: &str) -> Result<f32, &'static str> {
    let v = parse_coord(s)?;
    if v < 0.0 {
        return Err("negative SVG dimension");
    }
    Ok(v)
}

fn parse_length(s: &str, min: f32, max: f32) -> Result<f32, &'static str> {
    let v = parse_finite(s.trim().trim_end_matches("px"))?;
    if !(min..=max).contains(&v) {
        return Err("SVG dimension out of bounds");
    }
    Ok(v)
}

fn parse_opacity(s: &str) -> Result<f32, &'static str> {
    let t = s.trim();
    let v = if let Some(p) = t.strip_suffix('%') { parse_finite(p)? / 100.0 } else { parse_finite(t)? };
    Ok(v.clamp(0.0, 1.0))
}

fn parse_clock(s: &str) -> Result<f32, &'static str> {
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
