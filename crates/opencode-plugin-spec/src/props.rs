//! How the host reads a `ui.*` node's props table: by key, straight from the runtime's table,
//! with no intermediate value. The generated node tables use these types.

/// What a prop holds, and so how the host reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropType {
    Number,
    /// A whole number.
    Integer,
    String,
    Bool,
    /// A semantic token name or a `ui.hex` color ([`crate::ColorSpec`]).
    Color,
    /// A number of points, or `"full"`.
    Size,
    /// A string, number or boolean the host hashes for a memo.
    MemoVersion,
    /// One of a contract enum's strings, by the enum's name.
    Enum(&'static str),
    /// A `view:Handler` value.
    Handler,
    /// A Luau function the host calls itself, such as `renderRow`.
    Function,
    /// One `ui.*` node.
    Node,
    /// An array of `ui.*` nodes.
    Nodes,
    /// A host object, by its contract type name, such as `PtySession`.
    Handle(&'static str),
    /// A record, by its contract type name.
    Record(&'static str),
    /// An array of the named type.
    List(&'static str),
    /// A string-keyed table.
    Map,
    Any,
}

/// One prop.
#[derive(Clone, Copy, Debug)]
pub struct PropSpec {
    name: &'static str,
    ty: PropType,
    required: bool,
}

impl PropSpec {
    pub const fn new(name: &'static str, ty: PropType, required: bool) -> Self {
        PropSpec { name, ty, required }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn ty(&self) -> PropType {
        self.ty
    }

    /// A node without it is an error; every other prop may be nil.
    pub fn required(&self) -> bool {
        self.required
    }
}

/// A props type: its own props and the props type it extends.
#[derive(Debug)]
pub struct PropsSpec {
    name: &'static str,
    base: Option<&'static PropsSpec>,
    own: &'static [PropSpec],
}

impl PropsSpec {
    pub const fn new(name: &'static str, base: Option<&'static PropsSpec>, own: &'static [PropSpec]) -> Self {
        PropsSpec { name, base, own }
    }

    /// The contract's name for it, such as `UiBoxProps`.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// A prop by key. The host warns once per unknown key and ignores it.
    pub fn get(&self, key: &str) -> Option<&'static PropSpec> {
        self.own.iter().find(|p| p.name == key).or_else(|| self.base.and_then(|b| b.get(key)))
    }

    /// Every prop, the base type's first.
    pub fn all(&self) -> impl Iterator<Item = &'static PropSpec> {
        self.base.into_iter().flat_map(|b| b.all().collect::<Vec<_>>()).chain(self.own.iter())
    }
}
