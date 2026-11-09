//! Rules contract.json must keep before anything is generated from it.

use std::collections::HashSet;

use super::model::{Contract, Member, MemberKind, TypeKind};
use super::sig::{BUILTINS, identifiers, parse_sig};

pub fn validate(c: &Contract) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let known: HashSet<String> = c
        .types
        .iter()
        .map(|t| t.name.clone())
        .chain(c.enums.iter().map(|e| e.name.clone()))
        .chain(c.services.iter().map(|s| s.type_name()))
        .chain(["WindowContext", "AppContext", "UiLibrary", "PluginLibrary"].map(String::from))
        .chain(BUILTINS.iter().map(|b| (*b).to_owned()))
        .collect();

    if c.tokens.len() != 34 {
        errors.push(format!("contract {} has {} semantic tokens; 1.0 fixes 34", c.contract, c.tokens.len()));
    }
    unique(&mut errors, "token", c.tokens.iter().map(|t| t.name.as_str()));
    unique(&mut errors, "type", c.types.iter().map(|t| t.name.as_str()).chain(c.enums.iter().map(|e| e.name.as_str())));
    unique(&mut errors, "node", c.nodes.iter().map(|n| n.name.as_str()));
    unique(&mut errors, "service", c.services.iter().map(|s| s.name.as_str()));
    unique(&mut errors, "contribution", c.contributions.iter().map(|k| k.kind.as_str()));
    for t in &c.tokens {
        if !t.name.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
            errors.push(format!("token `{}` is not kebab-case", t.name));
        }
        if let Some(f) = &t.fallback
            && !c.tokens.iter().any(|o| &o.name == f && o.since.is_none())
        {
            errors.push(format!("token `{}` falls back to `{f}`, which is not a 1.0 token", t.name));
        }
        if t.since.is_some() && t.fallback.is_none() {
            errors.push(format!("token `{}` is newer than 1.0 and needs a fallback", t.name));
        }
    }
    let mut refer = |owner: &str, ty: &str, generics: &[String]| {
        for name in identifiers(ty) {
            if !known.contains(&name) && !generics.contains(&name) {
                errors.push(format!("{owner}: unknown type `{name}` in `{ty}`"));
            }
        }
    };
    for t in &c.types {
        let generics = generic_names(t.generics.as_deref());
        if t.kind == TypeKind::Alias {
            refer(&t.name, t.ty.as_deref().unwrap_or(""), &generics);
        }
        for base in &t.extends {
            refer(&t.name, base, &generics);
        }
        for f in &t.fields {
            refer(&format!("{}.{}", t.name, f.name), &f.ty, &generics);
        }
        for m in &t.members {
            member_types(m, &t.name, &generics, &mut refer);
        }
    }
    for m in &c.context.members {
        member_types(m, "context", &[], &mut refer);
    }
    for s in &c.services {
        for m in &s.members {
            member_types(m, &s.name, &[], &mut refer);
        }
    }
    for k in &c.contributions {
        refer(&k.method, &k.spec, &[]);
        refer(&k.method, k.returns(), &[]);
    }
    for n in &c.nodes {
        refer(&format!("ui.{}", n.name), &n.sig.replace("->", ","), &[]);
        refer(&format!("ui.{}", n.name), &n.props, &[]);
    }
    for e in &c.events {
        refer(&e.name, &e.payload, &[]);
    }
    for g in &c.globals {
        for m in &g.members {
            member_types(m, &g.name, &[], &mut refer);
        }
    }
    for m in c.plugin_module.iter().chain(&c.ui_extras) {
        member_types(m, "module", &[], &mut refer);
    }

    let members = c.types.iter().flat_map(|t| t.members.iter()).chain(c.services.iter().flat_map(|s| s.members.iter())).chain(&c.context.members);
    for m in members.chain(c.globals.iter().flat_map(|g| g.members.iter())) {
        if m.kind != MemberKind::Method && m.kind != MemberKind::Function {
            continue;
        }
        let asynchronous = m.name.ends_with("Async") || m.name == "Wait" || m.name == "wait";
        if m.yields != asynchronous {
            errors.push(format!("`{}`: only methods named `…Async` (and `Wait`) may yield, and they must say `yields`", m.name));
        }
        if let Some(sig) = &m.sig
            && let Err(e) = parse_sig(sig)
        {
            errors.push(format!("`{}`: {e}", m.name));
        }
    }
    for k in &c.contributions {
        if c.type_def(&k.spec).is_none() {
            errors.push(format!("contribution `{}` names a missing spec `{}`", k.kind, k.spec));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn member_types(m: &Member, owner: &str, outer: &[String], refer: &mut impl FnMut(&str, &str, &[String])) {
    let mut generics = outer.to_vec();
    generics.extend(generic_names(m.generics.as_deref()));
    let label = format!("{owner}.{}", m.name);
    for ty in [m.ty.as_deref(), m.payload.as_deref()].into_iter().flatten() {
        refer(&label, ty, &generics);
    }
    if let Some(sig) = &m.sig {
        refer(&label, &sig.replace("->", ","), &generics);
    }
}

pub fn generic_names(generics: Option<&str>) -> Vec<String> {
    generics.map(|g| g.split(',').map(|n| n.trim().trim_end_matches("...").to_owned()).collect()).unwrap_or_default()
}

fn unique<'a>(errors: &mut Vec<String>, what: &str, names: impl Iterator<Item = &'a str>) {
    let mut seen = HashSet::new();
    for n in names {
        if !seen.insert(n) {
            errors.push(format!("duplicate {what} `{n}`"));
        }
    }
}
