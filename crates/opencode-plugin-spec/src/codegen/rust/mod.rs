//! `src/generated/*.rs`: the host's Rust view of the contract. Enums and records decode Luau
//! values through serde (any runtime's serde bridge, such as `mlua::LuaSerdeExt`), and the node
//! tables let the host read `ui.*` props by key without building intermediate values.

mod types;

use std::collections::{BTreeSet, HashSet};

use super::model::{Contract, TypeDef, TypeKind};
use super::sig;
use super::{GENERATED, Out};
use types::{RustField, pascal, snake};

pub fn files(c: &Contract) -> Result<Vec<(String, String)>, String> {
    let specs = spec_records(c);
    let (spec_types, data_types): (Vec<&TypeDef>, Vec<&TypeDef>) =
        c.types.iter().filter(|t| t.rust && t.kind == TypeKind::Record).partition(|t| specs.contains(t.name.as_str()));
    Ok(vec![
        ("mod.rs".to_owned(), module()),
        ("tokens.rs".to_owned(), tokens(c)),
        ("enums.rs".to_owned(), enums(c)),
        ("specs.rs".to_owned(), records(c, "Contribution specs and the records nested in them.", &spec_types)?),
        ("data.rs".to_owned(), records(c, "Data the host and plugins pass each other.", &data_types)?),
        ("nodes.rs".to_owned(), nodes(c)),
        ("registry.rs".to_owned(), registry(c)),
        ("manifest.rs".to_owned(), manifest(c)?),
    ])
}

fn head(o: &mut Out, doc: &str) {
    o.line(&format!("//! {doc}"));
    o.line(&format!("//! {GENERATED}"));
    o.blank();
}

fn module() -> String {
    let mut o = Out::default();
    head(&mut o, "The contract's generated Rust types.");
    for m in ["data", "enums", "manifest", "nodes", "registry", "specs", "tokens"] {
        o.line(&format!("mod {m};"));
    }
    o.blank();
    for m in ["data", "enums", "manifest", "nodes", "registry", "specs", "tokens"] {
        o.line(&format!("pub use {m}::*;"));
    }
    o.finish()
}

fn tokens(c: &Contract) -> String {
    let mut o = Out::default();
    head(&mut o, "The semantic token palette.");
    o.line("use crate::string_enum;");
    o.blank();
    o.line("string_enum! {");
    o.line("    /// A theme color by meaning. The host maps each token to its theme once per theme change,");
    o.line("    /// and decodes one with a single `match` (`SemanticToken::parse`).");
    o.line("    pub enum SemanticToken {");
    for t in &c.tokens {
        o.line(&format!("        /// {}", t.doc));
        o.line(&format!("        {} = \"{}\",", pascal(&t.name), t.name));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("impl SemanticToken {");
    o.line("    /// The token's group, such as `surface` or `status`.");
    o.line("    pub fn group(self) -> &'static str {");
    o.line("        match self {");
    let groups: BTreeSet<&str> = c.tokens.iter().map(|t| t.group.as_str()).collect();
    for g in groups {
        let names: Vec<String> = c.tokens.iter().filter(|t| t.group == g).map(|t| format!("Self::{}", pascal(&t.name))).collect();
        o.line(&format!("            {} => \"{g}\",", names.join(" | ")));
    }
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.finish()
}

fn enums(c: &Contract) -> String {
    let mut o = Out::default();
    head(&mut o, "The contract's string enums.");
    o.line("use crate::string_enum;");
    for e in c.enums.iter().filter(|e| e.rust) {
        o.blank();
        o.line("string_enum! {");
        o.line(&format!("    /// {}", e.doc));
        o.line(&format!("    pub enum {} {{", e.name));
        for v in &e.values {
            o.line(&format!("        /// {}", v.doc));
            o.line(&format!("        {} = \"{}\",", pascal(&v.value), v.value));
        }
        o.line("    }");
        o.line("}");
    }
    o.finish()
}

/// Contribution specs, and the records only they hold.
fn spec_records(c: &Contract) -> HashSet<&str> {
    let mut set: HashSet<&str> = c.contributions.iter().map(|k| k.spec.as_str()).collect();
    let mut grew = true;
    while grew {
        grew = false;
        let nested: Vec<&str> = set
            .iter()
            .filter_map(|n| c.type_def(n))
            .flat_map(|t| t.fields.iter())
            .filter(|f| !sig::is_function(&f.ty))
            .flat_map(|f| sig::identifiers(&f.ty))
            .filter_map(|n| c.type_def(&n).filter(|t| t.kind == TypeKind::Record).map(|t| t.name.as_str()))
            .collect();
        for n in nested {
            grew |= set.insert(n);
        }
    }
    set
}

fn records(c: &Contract, doc: &str, list: &[&TypeDef]) -> Result<String, String> {
    let mut o = Out::default();
    head(&mut o, doc);
    o.line("#![allow(unused_imports)]");
    o.blank();
    o.line("use std::collections::BTreeMap;");
    o.blank();
    o.line("use serde::{Deserialize, Serialize};");
    o.line("use serde_json::Value;");
    o.blank();
    o.line("use super::*;");
    o.line("use crate::{ColorSpec, impl_record};");
    for t in list {
        o.blank();
        o.line(&format!("/// {}", t.doc));
        o.line("#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]");
        o.line("#[serde(rename_all = \"camelCase\")]");
        o.line("#[non_exhaustive]");
        o.line(&format!("pub struct {} {{", t.name));
        let (mut data, mut callbacks, mut handles) = (Vec::new(), Vec::new(), Vec::new());
        for f in c.fields_of(t) {
            match types::field(c, &f.ty).map_err(|e| format!("{}.{}: {e}", t.name, f.name))? {
                RustField::Data(ty) => {
                    o.line(&format!("    /// {}", f.doc));
                    // An empty Luau table arrives as an empty map: list fields take it.
                    if ty.starts_with("Vec<") {
                        o.line("    #[serde(deserialize_with = \"crate::lenient::seq\")]");
                    } else if ty.starts_with("Option<Vec<") {
                        o.line("    #[serde(default, deserialize_with = \"crate::lenient::opt_seq\")]");
                    }
                    o.line(&format!("    pub {}: {ty},", snake(&f.name)));
                    data.push(format!("\"{}\"", f.name));
                }
                RustField::Callback => callbacks.push(format!("\"{}\"", f.name)),
                RustField::Handle => handles.push(format!("\"{}\"", f.name)),
            }
        }
        o.line("}");
        o.line(&format!("impl_record!({}, [{}], [{}], [{}]);", t.name, data.join(", "), callbacks.join(", "), handles.join(", ")));
    }
    Ok(o.finish())
}

fn nodes(c: &Contract) -> String {
    let mut o = Out::default();
    head(&mut o, "The `ui.*` node vocabulary and each node's props, for decoding props tables by key.");
    o.line("use crate::{PropSpec, PropType, PropsSpec, string_enum};");
    o.blank();
    o.line("string_enum! {");
    o.line("    /// A `ui.*` node constructor.");
    o.line("    pub enum NodeKind {");
    for n in &c.nodes {
        o.line(&format!("        /// {}", n.doc));
        o.line(&format!("        {} = \"{}\",", pascal(&n.name), n.name));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("string_enum! {");
    o.line("    /// What a node is for.");
    o.line("    pub enum NodeCategory {");
    let categories: Vec<&str> = c.nodes.iter().map(|n| n.category.as_str()).fold(Vec::new(), |mut v, x| {
        if !v.contains(&x) {
            v.push(x);
        }
        v
    });
    for cat in &categories {
        o.line(&format!("        /// `{cat}` nodes."));
        o.line(&format!("        {} = \"{cat}\",", pascal(cat)));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("impl NodeKind {");
    o.line("    pub fn category(self) -> NodeCategory {");
    o.line("        match self {");
    for cat in &categories {
        let names: Vec<String> = c.nodes.iter().filter(|n| n.category == *cat).map(|n| format!("Self::{}", pascal(&n.name))).collect();
        o.line(&format!("            {} => NodeCategory::{},", names.join(" | "), pascal(cat)));
    }
    o.line("        }");
    o.line("    }");
    o.blank();
    o.line("    /// The host renders it with a native engine.");
    o.line("    pub fn is_native(self) -> bool {");
    o.line("        self.category() == NodeCategory::Native");
    o.line("    }");
    o.blank();
    o.line("    /// Its props, by key.");
    o.line("    pub fn props(self) -> &'static PropsSpec {");
    o.line("        match self {");
    let props: Vec<&str> = c.nodes.iter().map(|n| n.props.as_str()).fold(Vec::new(), |mut v, x| {
        if !v.contains(&x) {
            v.push(x);
        }
        v
    });
    for p in &props {
        let names: Vec<String> = c.nodes.iter().filter(|n| n.props == *p).map(|n| format!("Self::{}", pascal(&n.name))).collect();
        o.line(&format!("            {} => &{},", names.join(" | "), const_name(p)));
    }
    o.line("        }");
    o.line("    }");
    o.line("}");
    let mut emitted: Vec<&str> = Vec::new();
    let mut queue: Vec<&str> = props.clone();
    while let Some(p) = queue.pop() {
        if emitted.contains(&p) {
            continue;
        }
        emitted.push(p);
        if let Some(t) = c.type_def(p) {
            queue.extend(t.extends.iter().map(String::as_str));
        }
    }
    emitted.sort_unstable();
    for p in emitted {
        let Some(t) = c.type_def(p) else { continue };
        let base = t.extends.first().map(|b| format!("Some(&{})", const_name(b))).unwrap_or_else(|| "None".to_owned());
        o.blank();
        o.line(&format!("/// `{p}`."));
        o.line(&format!("pub static {}: PropsSpec = PropsSpec::new(\"{p}\", {base}, &[", const_name(p)));
        for f in &t.fields {
            let required = !f.ty.trim_end().ends_with('?');
            o.line(&format!("    PropSpec::new(\"{}\", {}, {required}),", f.name, types::prop(c, &f.ty)));
        }
        o.line("]);");
    }
    o.finish()
}

fn const_name(props: &str) -> String {
    snake(props).trim_start_matches("r#").to_ascii_uppercase()
}

fn registry(c: &Contract) -> String {
    let mut o = Out::default();
    head(&mut o, "Contribution kinds, services and context events, with the halves that offer them.");
    o.line("use crate::{Half, string_enum};");
    o.blank();
    let (major, minor) = c.contract.split_once('.').unwrap_or((&c.contract, "0"));
    o.line("/// The contract version this crate describes.");
    o.line(&format!("pub const CONTRACT: &str = \"{}\";", c.contract));
    o.line(&format!("pub const CONTRACT_MAJOR: u16 = {major};"));
    o.line(&format!("pub const CONTRACT_MINOR: u16 = {minor};"));
    let halves = |list: &[String]| -> String {
        let names: Vec<String> = list.iter().map(|h| format!("Half::{}", pascal(h))).collect();
        format!("&[{}]", names.join(", "))
    };
    o.blank();
    o.line("string_enum! {");
    o.line("    /// A contribution kind, by its contract name.");
    o.line("    pub enum ContributionKind {");
    for k in &c.contributions {
        o.line(&format!("        /// {}", k.doc));
        o.line(&format!("        {} = \"{}\",", pascal(&k.kind), k.kind));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("impl ContributionKind {");
    for (what, doc, value) in [
        ("method", "The context method that adds it, such as `AddCommand`.", 0),
        ("spec", "The spec record it takes.", 1),
    ] {
        o.line(&format!("    /// {doc}"));
        o.line(&format!("    pub fn {what}(self) -> &'static str {{"));
        o.line("        match self {");
        for k in &c.contributions {
            let v = if value == 0 { &k.method } else { &k.spec };
            o.line(&format!("            Self::{} => \"{v}\",", pascal(&k.kind)));
        }
        o.line("        }");
        o.line("    }");
        o.blank();
    }
    o.line("    /// The halves that may add it.");
    o.line("    pub fn halves(self) -> &'static [Half] {");
    o.line("        match self {");
    for k in &c.contributions {
        o.line(&format!("            Self::{} => {},", pascal(&k.kind), halves(&k.halves)));
    }
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("string_enum! {");
    o.line("    /// A host service, by the name `ctx:GetService` takes.");
    o.line("    pub enum ServiceName {");
    for s in &c.services {
        o.line(&format!("        /// {}", s.doc));
        o.line(&format!("        {} = \"{}\",", s.name, s.name));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("impl ServiceName {");
    o.line("    /// The halves that may get it.");
    o.line("    pub fn halves(self) -> &'static [Half] {");
    o.line("        match self {");
    for s in &c.services {
        o.line(&format!("            Self::{} => {},", s.name, halves(&s.halves)));
    }
    o.line("        }");
    o.line("    }");
    o.blank();
    o.line("    /// Its methods, signals and fields, by name.");
    o.line("    pub fn members(self) -> &'static [&'static str] {");
    o.line("        match self {");
    for s in &c.services {
        let names: Vec<String> = s.members.iter().map(|m| format!("\"{}\"", m.name)).collect();
        o.line(&format!("            Self::{} => &[{}],", s.name, names.join(", ")));
    }
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("string_enum! {");
    o.line("    /// A signal on a plugin context.");
    o.line("    pub enum ContextEvent {");
    for e in &c.events {
        o.line(&format!("        /// {}", e.doc));
        o.line(&format!("        {} = \"{}\",", e.name, e.name));
    }
    o.line("    }");
    o.line("}");
    o.blank();
    o.line("impl ContextEvent {");
    o.line("    /// The halves whose context has it.");
    o.line("    pub fn halves(self) -> &'static [Half] {");
    o.line("        match self {");
    for e in &c.events {
        o.line(&format!("            Self::{} => {},", e.name, halves(&e.halves)));
    }
    o.line("        }");
    o.line("    }");
    o.line("}");
    o.finish()
}

fn manifest(c: &Contract) -> Result<String, String> {
    let mut o = Out::default();
    head(&mut o, "plugin.toml, decoded with any serde TOML parser.");
    o.line("use serde::{Deserialize, Serialize};");
    o.blank();
    o.line("use super::*;");
    manifest_struct(&mut o, c, "Manifest", &c.manifest.doc, &c.manifest.fields)?;
    for f in c.manifest.fields.iter().filter(|f| f.ty == "table[]") {
        manifest_struct(&mut o, c, &manifest_table(&f.name), &f.doc, &f.fields)?;
    }
    Ok(o.finish())
}

fn manifest_struct(o: &mut Out, c: &Contract, name: &str, doc: &str, fields: &[super::model::ManifestField]) -> Result<(), String> {
    o.blank();
    o.line(&format!("/// {doc}"));
    o.line("#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]");
    o.line("#[non_exhaustive]");
    o.line(&format!("pub struct {name} {{"));
    let mut defaults = Vec::new();
    for f in fields {
        let ty = match (f.ty.as_str(), &f.enum_name) {
            ("string", _) => "String".to_owned(),
            ("string[]", _) => "Vec<String>".to_owned(),
            ("enum", Some(e)) => e.clone(),
            ("enum[]", Some(e)) => format!("Vec<{e}>"),
            ("table[]", _) => format!("Vec<{}>", manifest_table(&f.name)),
            (other, _) => return Err(format!("manifest field `{}`: no Rust type for `{other}`", f.name)),
        };
        o.line(&format!("    /// {}", f.doc));
        match (&f.default, f.required, f.ty.as_str()) {
            (Some(d), _, "enum") => {
                let first = f.enum_name.as_deref().and_then(|e| c.enum_def(e)).and_then(|e| e.values.first()).map(|v| v.value.as_str());
                if first != Some(d.as_str()) {
                    return Err(format!("manifest field `{}`: an enum default must be the enum's first value", f.name));
                }
                o.line("    #[serde(default)]");
                o.line(&format!("    pub {}: {ty},", snake(&f.name)));
            }
            (Some(d), _, _) => {
                let func = format!("default_{}_{}", snake(name), snake(&f.name));
                o.line(&format!("    #[serde(default = \"{func}\")]"));
                o.line(&format!("    pub {}: {ty},", snake(&f.name)));
                defaults.push((func, ty, d.clone()));
            }
            (None, true, _) => o.line(&format!("    pub {}: {ty},", snake(&f.name))),
            (None, false, t) if t.ends_with("[]") => {
                o.line("    #[serde(default, skip_serializing_if = \"Vec::is_empty\")]");
                o.line(&format!("    pub {}: {ty},", snake(&f.name)));
            }
            (None, false, _) => {
                o.line("    #[serde(default, skip_serializing_if = \"Option::is_none\")]");
                o.line(&format!("    pub {}: Option<{ty}>,", snake(&f.name)));
            }
        }
    }
    o.line("}");
    for (func, ty, value) in defaults {
        o.blank();
        o.line(&format!("fn {func}() -> {ty} {{"));
        o.line(&format!("    \"{value}\".to_owned()"));
        o.line("}");
    }
    Ok(())
}

/// The struct for one entry of a `[[table]]` key: `stores` is `ManifestStore`.
fn manifest_table(key: &str) -> String {
    let name = format!("Manifest{}", pascal(key));
    name.strip_suffix('s').map(str::to_owned).unwrap_or(name)
}
