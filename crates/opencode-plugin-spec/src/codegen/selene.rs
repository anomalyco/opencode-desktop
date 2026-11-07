//! `plugins/opencode-selene.yml`: the selene standard library for the contract's globals, on top
//! of selene's `luau` base. Services come from `ctx:GetService`, so only globals live here.

use super::model::{Contract, Member, MemberKind};
use super::sig::{self, parse_sig};
use super::{GENERATED, Out};

pub fn std(c: &Contract) -> Result<String, String> {
    let mut o = Out::default();
    o.line(&format!("# {GENERATED}"));
    o.line("---");
    o.line("base: luau");
    o.line("name: opencode");
    o.line("globals:");
    for g in &c.globals {
        match g.kind {
            MemberKind::Library => {
                for m in &g.members {
                    function(&mut o, &format!("{}.{}", g.name, m.name), m.sig.as_deref(), m)?;
                }
            }
            _ => {
                let m = Member {
                    name: g.name.clone(),
                    kind: g.kind,
                    doc: g.doc.clone(),
                    ty: None,
                    sig: g.sig.clone(),
                    payload: None,
                    generics: None,
                    yields: false,
                    halves: Vec::new(),
                    since: None,
                    deprecated: None,
                };
                function(&mut o, &g.name, g.sig.as_deref(), &m)?;
            }
        }
    }
    Ok(o.finish())
}

fn function(o: &mut Out, name: &str, signature: Option<&str>, m: &Member) -> Result<(), String> {
    let s = parse_sig(signature.unwrap_or("() -> ()"))?;
    o.line(&format!("  {name}:"));
    if let Some(reason) = &m.deprecated {
        o.line("    deprecated:");
        o.line(&format!("      message: \"{}\"", reason.replace('"', "'")));
    }
    if s.params.is_empty() {
        o.line("    args: []");
        return Ok(());
    }
    o.line("    args:");
    for p in &s.params {
        let (inner, optional) = sig::strip_optional(&p.ty);
        let pack = p.name.is_none() && (inner.starts_with("...") || inner.ends_with("..."));
        let ty = if pack {
            "\"...\""
        } else if sig::is_function(inner) {
            "function"
        } else {
            match inner {
                "number" | "integer" => "number",
                "string" => "string",
                "boolean" => "bool",
                _ => "any",
            }
        };
        if optional || pack {
            o.line("      - required: false");
            o.line(&format!("        type: {ty}"));
        } else {
            o.line(&format!("      - type: {ty}"));
        }
    }
    Ok(())
}
