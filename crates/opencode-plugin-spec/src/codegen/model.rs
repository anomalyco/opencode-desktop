//! contract.json as data. Type strings stay in Luau syntax; [`super::sig`] reads them.

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    #[serde(rename = "$comment")]
    pub _comment: Option<String>,
    pub contract: String,
    pub versioning: Versioning,
    pub tokens: Vec<Token>,
    pub enums: Vec<EnumDef>,
    pub types: Vec<TypeDef>,
    pub context: Context,
    pub contributions: Vec<ContributionDef>,
    pub services: Vec<Service>,
    pub events: Vec<Event>,
    pub nodes: Vec<Node>,
    #[serde(rename = "uiExtras")]
    pub ui_extras: Vec<Member>,
    #[serde(rename = "pluginModule")]
    pub plugin_module: Vec<Member>,
    pub globals: Vec<Global>,
    pub manifest: Manifest,
    pub leaks: Vec<Leak>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Versioning {
    pub format: String,
    pub rules: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Token {
    pub name: String,
    pub group: String,
    pub doc: String,
    pub since: Option<String>,
    pub fallback: Option<String>,
    pub deprecated: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumDef {
    pub name: String,
    pub doc: String,
    #[serde(default)]
    pub rust: bool,
    pub values: Vec<EnumValue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumValue {
    pub value: String,
    pub doc: String,
    pub since: Option<String>,
    pub deprecated: Option<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TypeKind {
    /// The semantic token union, filled from `tokens`.
    Tokens,
    /// A nominal type the host owns; Luau sees no structure.
    Opaque,
    /// `export type Name = <type>`.
    Alias,
    /// Plain data: fields only.
    Record,
    /// A host object: fields, methods and signals.
    Object,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
    pub doc: String,
    #[serde(rename = "type")]
    pub ty: Option<String>,
    pub generics: Option<String>,
    #[serde(default)]
    pub rust: bool,
    /// The official Desktop plugin API's name for the same thing.
    pub official: Option<String>,
    #[serde(default)]
    pub extends: Vec<String>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub doc: String,
    pub since: Option<String>,
    pub deprecated: Option<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    Field,
    Method,
    Signal,
    Function,
    Library,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub name: String,
    pub kind: MemberKind,
    pub doc: String,
    /// A field's type.
    #[serde(rename = "type")]
    pub ty: Option<String>,
    /// A method's or function's `(params) -> returns`, without `self`.
    pub sig: Option<String>,
    /// A signal's payload types, comma-separated; `()` for none.
    pub payload: Option<String>,
    pub generics: Option<String>,
    #[serde(default)]
    pub yields: bool,
    #[serde(default)]
    pub halves: Vec<String>,
    pub since: Option<String>,
    pub deprecated: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub doc: String,
    pub members: Vec<Member>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionDef {
    pub kind: String,
    pub method: String,
    pub spec: String,
    pub returns: Option<String>,
    pub halves: Vec<String>,
    pub doc: String,
    pub since: Option<String>,
}

impl ContributionDef {
    pub fn returns(&self) -> &str {
        self.returns.as_deref().unwrap_or("Contribution")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    pub halves: Vec<String>,
    pub doc: String,
    pub members: Vec<Member>,
    pub since: Option<String>,
}

impl Service {
    pub fn type_name(&self) -> String {
        format!("{}Service", self.name)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub name: String,
    pub halves: Vec<String>,
    pub payload: String,
    pub doc: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub name: String,
    pub category: String,
    pub sig: String,
    pub props: String,
    pub doc: String,
    pub native: Option<Native>,
    pub since: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Native {
    pub engine: String,
    pub native: String,
    pub luau: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Global {
    pub name: String,
    pub kind: MemberKind,
    pub doc: String,
    pub sig: Option<String>,
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub doc: String,
    pub fields: Vec<ManifestField>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestField {
    pub name: String,
    /// `string`, `string[]`, `enum`, `enum[]` or `table[]`.
    #[serde(rename = "type")]
    pub ty: String,
    pub doc: String,
    #[serde(default)]
    pub required: bool,
    pub pattern: Option<String>,
    pub default: Option<String>,
    pub format: Option<String>,
    #[serde(rename = "enum")]
    pub enum_name: Option<String>,
    #[serde(default)]
    pub fields: Vec<ManifestField>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leak {
    pub old: String,
    pub replacement: String,
}

impl Contract {
    pub fn type_def(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }

    pub fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|e| e.name == name)
    }

    /// A record's fields with its `extends` records' fields first, flattened.
    pub fn fields_of(&self, ty: &TypeDef) -> Vec<Field> {
        ty.extends
            .iter()
            .filter_map(|base| self.type_def(base))
            .flat_map(|base| self.fields_of(base))
            .chain(ty.fields.iter().cloned())
            .fold(Vec::new(), |mut all: Vec<Field>, f| {
                match all.iter_mut().find(|g| g.name == f.name) {
                    Some(g) => *g = f,
                    None => all.push(f),
                }
                all
            })
    }

    pub fn services_for(&self, half: &str) -> impl Iterator<Item = &Service> {
        self.services.iter().filter(move |s| s.halves.iter().any(|h| h == half))
    }

    pub fn contributions_for(&self, half: &str) -> impl Iterator<Item = &ContributionDef> {
        self.contributions.iter().filter(move |c| c.halves.iter().any(|h| h == half))
    }
}

pub const HALVES: [&str; 2] = ["window", "app"];

pub fn context_type(half: &str) -> &'static str {
    if half == "app" { "AppContext" } else { "WindowContext" }
}
