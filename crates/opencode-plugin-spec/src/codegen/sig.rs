//! Reads the Luau type strings contract.json uses: signatures, optionals, function types and
//! the names a type refers to.

pub const BUILTINS: [&str; 10] = ["string", "number", "boolean", "any", "nil", "thread", "buffer", "integer", "unknown", "never"];

pub struct Param {
    /// None for a bare type or a pack (`T...`, `...any`).
    pub name: Option<String>,
    pub ty: String,
}

pub struct Sig {
    pub params: Vec<Param>,
    pub ret: String,
}

/// Splits `s` on `sep` outside brackets and string literals.
pub fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quoted = false;
    let mut current = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => quoted = !quoted,
            '(' | '{' | '<' | '[' if !quoted => depth += 1,
            ')' | '}' | ']' if !quoted => depth -= 1,
            // `->` is not a closing bracket.
            '>' if !quoted && i > 0 && chars[i - 1] != '-' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 && !quoted {
            parts.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    parts
}

/// The index of the bracket that closes the one at `open`.
fn closing(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i < open) {
        match c {
            '(' | '{' => depth += 1,
            ')' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

pub fn parse_sig(sig: &str) -> Result<Sig, String> {
    let sig = sig.trim();
    if !sig.starts_with('(') {
        return Err(format!("signature must start with `(`: {sig}"));
    }
    let close = closing(sig, 0).ok_or_else(|| format!("unbalanced signature: {sig}"))?;
    let rest = sig[close + 1..].trim();
    let ret = rest.strip_prefix("->").ok_or_else(|| format!("signature needs `-> returns`: {sig}"))?.trim().to_owned();
    let params = split_top(&sig[1..close], ',')
        .into_iter()
        .map(|p| match p.split_once(':') {
            Some((name, ty)) if !name.contains(['(', '{', '"', '.']) => Param { name: Some(name.trim().to_owned()), ty: ty.trim().to_owned() },
            _ => Param { name: None, ty: p },
        })
        .collect();
    Ok(Sig { params, ret })
}

/// Strips one layer of wrapping parentheses: `((a) -> b)` is `(a) -> b`.
pub fn unwrap_parens(ty: &str) -> &str {
    let ty = ty.trim();
    if ty.starts_with('(') && closing(ty, 0) == Some(ty.len() - 1) {
        return unwrap_parens(&ty[1..ty.len() - 1]);
    }
    ty
}

/// `T?` is (`T`, true).
pub fn strip_optional(ty: &str) -> (&str, bool) {
    let ty = ty.trim();
    match ty.strip_suffix('?') {
        Some(inner) => (unwrap_parens(inner), true),
        None => (ty, false),
    }
}

pub fn is_function(ty: &str) -> bool {
    let inner = unwrap_parens(strip_optional(ty).0);
    inner.starts_with('(') && closing(inner, 0).is_some_and(|c| inner[c + 1..].trim_start().starts_with("->"))
}

/// The named types a type string refers to, without parameter names or string literals.
pub fn identifiers(ty: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quoted = false;
    let chars: Vec<char> = ty.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if !quoted && (c.is_ascii_alphabetic() || c == '_') {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars[i..].iter().find(|c| !c.is_whitespace());
            let pack = chars[i..].starts_with(&['.', '.', '.']);
            if next != Some(&':') && !pack {
                out.push(word);
            }
            continue;
        }
        i += 1;
    }
    out
}

/// The type in Luau: `integer` becomes `number`.
pub fn luau(ty: &str) -> String {
    let mut out = String::with_capacity(ty.len());
    let mut quoted = false;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(if word == "integer" { "number" } else { word });
        word.clear();
    };
    for c in ty.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        if !quoted && (c.is_ascii_alphanumeric() || c == '_') {
            word.push(c);
            continue;
        }
        flush(&mut word, &mut out);
        out.push(c);
    }
    flush(&mut word, &mut out);
    out
}
