//! JSON as the server wrote it: an object's members and an array's items as their own text, and
//! `JSON.stringify(value, null, 2)`'s layout, both keeping the server's key order (a parsed map
//! would sort it).

/// Where the value starting at `i` (after whitespace) ends.
fn value_end(s: &[u8], mut i: usize) -> usize {
    i = skip_ws(s, i);
    match s.get(i) {
        Some(b'"') => string_end(s, i),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            while i < s.len() {
                match s[i] {
                    b'"' => {
                        i = string_end(s, i);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return i + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            i
        }
        _ => {
            while i < s.len() && !matches!(s[i], b',' | b'}' | b']') && !s[i].is_ascii_whitespace() {
                i += 1;
            }
            i
        }
    }
}

/// The index after the string starting at `i` (its opening quote).
fn string_end(s: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < s.len() {
        match s[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

fn skip_ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// The text of member `key` of the object `obj`.
pub fn member<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let s = obj.as_bytes();
    let mut i = skip_ws(s, 0);
    if s.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(s, i);
        if s.get(i) != Some(&b'"') {
            return None;
        }
        let end = string_end(s, i);
        let name = &obj[i + 1..end - 1];
        i = skip_ws(s, end);
        if s.get(i) != Some(&b':') {
            return None;
        }
        let start = skip_ws(s, i + 1);
        let stop = value_end(s, start);
        if name == key {
            return Some(&obj[start..stop]);
        }
        i = skip_ws(s, stop);
        if s.get(i) != Some(&b',') {
            return None;
        }
        i += 1;
    }
}

/// The text of each item of the array `arr`.
pub fn items(arr: &str) -> Vec<&str> {
    let s = arr.as_bytes();
    let mut out = Vec::new();
    let mut i = skip_ws(s, 0);
    if s.get(i) != Some(&b'[') {
        return out;
    }
    i = skip_ws(s, i + 1);
    if s.get(i) == Some(&b']') {
        return out;
    }
    while i < s.len() {
        let start = skip_ws(s, i);
        let stop = value_end(s, start);
        out.push(&arr[start..stop]);
        i = skip_ws(s, stop);
        if s.get(i) != Some(&b',') {
            break;
        }
        i += 1;
    }
    out
}

/// `JSON.stringify(JSON.parse(raw), null, 2)`'s layout, in the order `raw` has.
pub fn pretty(raw: &str) -> String {
    let s = raw.as_bytes();
    let mut out = String::with_capacity(raw.len() * 2);
    let mut depth = 0usize;
    let mut i = 0;
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        (0..depth).for_each(|_| out.push_str("  "));
    };
    while i < s.len() {
        let c = s[i];
        match c {
            b'"' => {
                let end = string_end(s, i);
                out.push_str(&raw[i..end]);
                i = end;
                continue;
            }
            b'{' | b'[' => {
                let next = skip_ws(s, i + 1);
                let close = if c == b'{' { b'}' } else { b']' };
                if s.get(next) == Some(&close) {
                    out.push(c as char);
                    out.push(close as char);
                    i = next + 1;
                    continue;
                }
                out.push(c as char);
                depth += 1;
                newline(&mut out, depth);
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c as char);
            }
            b',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            b':' => out.push_str(": "),
            c if c.is_ascii_whitespace() => {}
            _ => {
                let end = value_end(s, i);
                out.push_str(&raw[i..end]);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}
