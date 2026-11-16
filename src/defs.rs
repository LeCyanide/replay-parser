//! Entity definitions: the game's `scripts/` XML, which says what every
//! entity method and property carries on the wire.
//!
//! Read from any [`ScriptSource`] — a loose `scripts/` directory today, the
//! game's packed resources once a package reader exists.
//!
//! The rules that decide wire indices, each of which shifts every later index
//! by one if got wrong:
//!
//! * Entity type ids are 1-based positions in `entities.xml`'s
//!   `ClientServerEntities`.
//! * An entity's members are its interfaces' (each interface's own
//!   interfaces first, in `Implements` order) followed by its own.
//! * Client methods are stably sorted by their argument stream size, with
//!   every variable-size argument counting as 0xffff ("infinite").
//! * Client-visible properties (`ALL_CLIENTS`, `OTHER_CLIENTS`, `OWN_CLIENT`,
//!   `CELL_PUBLIC_AND_OWN`, `BASE_AND_CLIENT`) are stably sorted the same way
//!   for property updates and entity creation. `CellPlayerCreate` and
//!   `BasePlayerCreate` instead carry the cell-side and `BASE_AND_CLIENT`
//!   subsets in declaration order.

use std::collections::HashMap;
use std::path::PathBuf;

/// Anything that can hand over a file under `scripts/` by relative path.
pub trait ScriptSource {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>>;
}

/// A directory laid out like the client's `res/scripts`.
pub struct ScriptDir(pub PathBuf);

impl ScriptSource for ScriptDir {
    fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.0.join(path))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DefError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("{path}: not UTF-8")]
    Utf8 { path: String },
    #[error("{path}: {source}")]
    Xml { path: String, source: roxmltree::Error },
    #[error("{path}: {msg}")]
    Shape { path: String, msg: String },
}

/// Size used for "variable length" when ordering members.
pub const INFINITE: usize = 0xffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Vector2,
    Vector3,
    Vector4,
    String,
    UnicodeString,
    /// Raw bytes, length-prefixed. `PYTHON` (a pickle), `MAILBOX`, and
    /// `USER_TYPE` without a declared wire type are carried this way.
    Blob,
    /// A `PYTHON`-typed value: a length-prefixed pickle.
    Python,
}

impl Primitive {
    fn fixed_size(self) -> Option<usize> {
        Some(match self {
            Primitive::U8 | Primitive::I8 => 1,
            Primitive::U16 | Primitive::I16 => 2,
            Primitive::U32 | Primitive::I32 | Primitive::F32 => 4,
            Primitive::U64 | Primitive::I64 | Primitive::F64 | Primitive::Vector2 => 8,
            Primitive::Vector3 => 12,
            Primitive::Vector4 => 16,
            Primitive::String | Primitive::UnicodeString | Primitive::Blob | Primitive::Python => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Primitive(Primitive),
    /// `size` is Some for fixed-length arrays, which carry no count.
    Array {
        of: Box<Type>,
        size: Option<usize>,
    },
    Tuple {
        of: Box<Type>,
        size: Option<usize>,
    },
    FixedDict {
        allow_none: bool,
        fields: Vec<(String, Type)>,
    },
    /// A `USER_TYPE` with a declared type. On the wire it is a length-prefixed
    /// blob regardless (see [`crate::value`]); the declared type says what the
    /// converter produces from it. Always ordered as variable size.
    User(Box<Type>),
    /// An `alias.xml` name, kept so values can say what they mean.
    Alias {
        name: String,
        inner: Box<Type>,
    },
}

impl Type {
    /// Stream size used for ordering, [`INFINITE`] when variable.
    pub fn sort_size(&self) -> usize {
        match self {
            Type::Primitive(p) => p.fixed_size().unwrap_or(INFINITE),
            Type::Array { of, size: Some(n) } | Type::Tuple { of, size: Some(n) } => match of.sort_size() {
                INFINITE => INFINITE,
                s => s * n,
            },
            Type::Array { size: None, .. } | Type::Tuple { size: None, .. } => INFINITE,
            Type::FixedDict { allow_none: true, .. } => INFINITE,
            Type::FixedDict { allow_none: false, fields } => {
                fields.iter().try_fold(0usize, |acc, (_, t)| match t.sort_size() {
                    INFINITE => None,
                    s => Some(acc + s),
                })
            }
            .unwrap_or(INFINITE),
            Type::User(_) => INFINITE,
            Type::Alias { inner, .. } => inner.sort_size(),
        }
    }

    pub fn alias_name(&self) -> Option<&str> {
        match self {
            Type::Alias { name, .. } => Some(name),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flags {
    AllClients,
    OtherClients,
    OwnClient,
    CellPublicAndOwn,
    BaseAndClient,
    Base,
    CellPrivate,
    CellPublic,
}

impl Flags {
    fn parse(s: &str) -> Option<Flags> {
        Some(match s {
            "ALL_CLIENTS" => Flags::AllClients,
            "OTHER_CLIENTS" => Flags::OtherClients,
            "OWN_CLIENT" => Flags::OwnClient,
            "CELL_PUBLIC_AND_OWN" => Flags::CellPublicAndOwn,
            "BASE_AND_CLIENT" => Flags::BaseAndClient,
            "BASE" => Flags::Base,
            "CELL_PRIVATE" => Flags::CellPrivate,
            "CELL_PUBLIC" => Flags::CellPublic,
            _ => return None,
        })
    }

    /// Whether the property ever reaches a client.
    pub fn client_visible(self) -> bool {
        matches!(
            self,
            Flags::AllClients | Flags::OtherClients | Flags::OwnClient | Flags::CellPublicAndOwn | Flags::BaseAndClient
        )
    }

    /// Sent in the cell half of the player's own entity creation.
    fn cell_to_own_client(self) -> bool {
        matches!(self, Flags::AllClients | Flags::OtherClients | Flags::OwnClient | Flags::CellPublicAndOwn)
    }
}

#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub ty: Type,
    pub flags: Flags,
}

#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub args: Vec<(String, Type)>,
    variable_header: usize,
}

impl Method {
    /// Sum of the argument sizes; [`INFINITE`] or more when any is variable.
    pub fn fixed_args_size(&self) -> usize {
        self.args.iter().map(|(_, t)| t.sort_size()).sum()
    }

    fn sort_size(&self) -> usize {
        let fixed = self.fixed_args_size();
        if fixed >= INFINITE { INFINITE + self.variable_header } else { fixed + self.variable_header }
    }
}

#[derive(Clone, Debug)]
pub struct EntityDef {
    pub name: String,
    /// Indexed by the method id in an `EntityMethod` packet.
    pub client_methods: Vec<Method>,
    /// Indexed by the property id in `EntityProperty` and `EntityCreate`.
    pub client_properties: Vec<Property>,
    /// What `CellPlayerCreate` carries, in order.
    pub cell_player_properties: Vec<Property>,
    /// What `BasePlayerCreate` carries, in order.
    pub base_player_properties: Vec<Property>,
}

#[derive(Clone, Debug)]
pub struct EntityDefs {
    /// Index `type_id - 1`.
    pub entities: Vec<EntityDef>,
}

impl EntityDefs {
    pub fn by_type_id(&self, type_id: u16) -> Option<&EntityDef> {
        (type_id as usize).checked_sub(1).and_then(|i| self.entities.get(i))
    }

    pub fn load(src: &dyn ScriptSource) -> Result<EntityDefs, DefError> {
        let aliases = load_aliases(src)?;
        let path = "entities.xml";
        let text = read_text(src, path)?;
        let doc = parse_xml(path, &text)?;
        let list = child(doc.root_element(), "ClientServerEntities")
            .ok_or_else(|| shape(path, "no ClientServerEntities".into()))?;
        let mut entities = Vec::new();
        for e in elements(list) {
            let name = e.tag_name().name();
            let mut parts = Members::default();
            collect(src, &aliases, &format!("entity_defs/{name}.def"), &mut parts, 0)?;
            entities.push(parts.finish(name));
        }
        Ok(EntityDefs { entities })
    }
}

#[derive(Default)]
struct Members {
    methods: Vec<Method>,
    properties: Vec<Property>,
}

impl Members {
    fn finish(mut self, name: &str) -> EntityDef {
        // A method defined again by a later interface is rejected by the
        // engine, which keeps the first definition.
        let mut seen = std::collections::HashSet::new();
        self.methods.retain(|m| seen.insert(m.name.clone()));
        self.methods.sort_by_key(Method::sort_size);

        let mut client_properties: Vec<Property> =
            self.properties.iter().filter(|p| p.flags.client_visible()).cloned().collect();
        client_properties.sort_by_key(|p| p.ty.sort_size());

        EntityDef {
            name: name.to_owned(),
            client_methods: self.methods,
            client_properties,
            cell_player_properties: self.properties.iter().filter(|p| p.flags.cell_to_own_client()).cloned().collect(),
            base_player_properties: self
                .properties
                .iter()
                .filter(|p| p.flags == Flags::BaseAndClient)
                .cloned()
                .collect(),
        }
    }
}

/// Appends a def file's members after those of the interfaces it implements.
fn collect(
    src: &dyn ScriptSource,
    aliases: &HashMap<String, Type>,
    path: &str,
    out: &mut Members,
    depth: usize,
) -> Result<(), DefError> {
    if depth > 16 {
        return Err(shape(path, "interfaces nest more than 16 deep (a cycle?)".into()));
    }
    let text = read_text(src, path)?;
    let doc = parse_xml(path, &text)?;
    let root = doc.root_element();

    if let Some(imp) = child(root, "Implements") {
        for i in elements(imp) {
            let iface = node_text(i).ok_or_else(|| shape(path, "empty Interface".into()))?;
            collect(src, aliases, &format!("entity_defs/interfaces/{iface}.def"), out, depth + 1)?;
        }
    }
    if let Some(props) = child(root, "Properties") {
        for p in elements(props) {
            let name = p.tag_name().name();
            let ty_node = child(p, "Type").ok_or_else(|| shape(path, format!("property {name} has no Type")))?;
            let flags_text = child(p, "Flags")
                .and_then(node_text)
                .ok_or_else(|| shape(path, format!("property {name} has no Flags")))?;
            let flags = Flags::parse(flags_text).ok_or_else(|| shape(path, format!("unknown flags {flags_text:?}")))?;
            out.properties.push(Property { name: name.to_owned(), ty: parse_type(path, ty_node, aliases)?, flags });
        }
    }
    if let Some(methods) = child(root, "ClientMethods") {
        for m in elements(methods) {
            out.methods.push(parse_method(path, m, aliases)?);
        }
    }
    Ok(())
}

fn parse_method(path: &str, m: roxmltree::Node, aliases: &HashMap<String, Type>) -> Result<Method, DefError> {
    let mut args = Vec::new();
    for c in elements(m) {
        match c.tag_name().name() {
            "Arg" => args.push((format!("arg{}", args.len()), parse_type(path, c, aliases)?)),
            "Args" => {
                for a in elements(c) {
                    args.push((a.tag_name().name().to_owned(), parse_type(path, a, aliases)?));
                }
            }
            _ => {}
        }
    }
    let variable_header = child(m, "VariableLengthHeaderSize")
        .and_then(|n| elements(n).next().and_then(node_text).or_else(|| node_text(n)))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    Ok(Method { name: m.tag_name().name().to_owned(), args, variable_header })
}

fn load_aliases(src: &dyn ScriptSource) -> Result<HashMap<String, Type>, DefError> {
    let path = "entity_defs/alias.xml";
    let text = read_text(src, path)?;
    let doc = parse_xml(path, &text)?;
    let mut aliases = HashMap::new();
    // Aliases may refer to aliases defined earlier in the file.
    for a in elements(doc.root_element()) {
        let ty = parse_type(path, a, &aliases)?;
        aliases.insert(a.tag_name().name().to_owned(), ty);
    }
    Ok(aliases)
}

/// Parses a type node: its leading text names the type, child elements
/// (`of`, `size`, `Properties`, `AllowNone`, `Type`) refine it.
pub fn parse_type(path: &str, node: roxmltree::Node, aliases: &HashMap<String, Type>) -> Result<Type, DefError> {
    let name = node_text(node).ok_or_else(|| shape(path, format!("<{}> names no type", node.tag_name().name())))?;
    let p = |p| Ok(Type::Primitive(p));
    match name {
        "UINT8" => p(Primitive::U8),
        "UINT16" => p(Primitive::U16),
        "UINT32" => p(Primitive::U32),
        "UINT64" => p(Primitive::U64),
        "INT8" => p(Primitive::I8),
        "INT16" => p(Primitive::I16),
        "INT32" => p(Primitive::I32),
        "INT64" => p(Primitive::I64),
        "FLOAT" | "FLOAT32" => p(Primitive::F32),
        "FLOAT64" => p(Primitive::F64),
        "VECTOR2" => p(Primitive::Vector2),
        "VECTOR3" => p(Primitive::Vector3),
        "VECTOR4" => p(Primitive::Vector4),
        "STRING" => p(Primitive::String),
        "UNICODE_STRING" => p(Primitive::UnicodeString),
        "BLOB" | "MAILBOX" => p(Primitive::Blob),
        "PYTHON" => p(Primitive::Python),
        "USER_TYPE" => match child(node, "Type") {
            Some(inner) => Ok(Type::User(Box::new(parse_type(path, inner, aliases)?))),
            None => p(Primitive::Blob),
        },
        "ARRAY" | "TUPLE" => {
            let of = child(node, "of").ok_or_else(|| shape(path, format!("{name} without <of>")))?;
            let of = Box::new(parse_type(path, of, aliases)?);
            let size = match child(node, "size").and_then(node_text) {
                Some(s) => Some(s.parse().map_err(|_| shape(path, format!("bad {name} size {s:?}")))?),
                None => None,
            };
            Ok(if name == "ARRAY" { Type::Array { of, size } } else { Type::Tuple { of, size } })
        }
        "FIXED_DICT" => {
            let allow_none =
                child(node, "AllowNone").and_then(node_text).is_some_and(|s| s.eq_ignore_ascii_case("true"));
            let mut fields = Vec::new();
            if let Some(props) = child(node, "Properties") {
                for f in elements(props) {
                    let t = child(f, "Type").ok_or_else(|| shape(path, "dict field without Type".into()))?;
                    fields.push((f.tag_name().name().to_owned(), parse_type(path, t, aliases)?));
                }
            }
            Ok(Type::FixedDict { allow_none, fields })
        }
        other => match aliases.get(other) {
            Some(t) => Ok(Type::Alias { name: other.to_owned(), inner: Box::new(t.clone()) }),
            None => Err(shape(path, format!("unknown type {other:?}"))),
        },
    }
}

// ---- XML helpers

fn read_text(src: &dyn ScriptSource, path: &str) -> Result<String, DefError> {
    let bytes = src.read(path).map_err(|source| DefError::Io { path: path.to_owned(), source })?;
    String::from_utf8(bytes).map_err(|_| DefError::Utf8 { path: path.to_owned() })
}

fn parse_xml<'t>(path: &str, text: &'t str) -> Result<roxmltree::Document<'t>, DefError> {
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    roxmltree::Document::parse_with_options(text, opts)
        .map_err(|source| DefError::Xml { path: path.to_owned(), source })
}

fn shape(path: &str, msg: String) -> DefError {
    DefError::Shape { path: path.to_owned(), msg }
}

fn elements<'a, 'i>(n: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    elements(n).find(|c| c.tag_name().name() == name)
}

/// The first non-blank text directly inside a node, trimmed.
fn node_text<'a>(n: roxmltree::Node<'a, '_>) -> Option<&'a str> {
    n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).map(str::trim).find(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(HashMap<&'static str, &'static str>);
    impl ScriptSource for Mem {
        fn read(&self, path: &str) -> std::io::Result<Vec<u8>> {
            self.0.get(path).map(|s| s.as_bytes().to_vec()).ok_or_else(|| std::io::ErrorKind::NotFound.into())
        }
    }

    fn sample() -> Mem {
        Mem(HashMap::from([
            ("entities.xml", "<root><ClientServerEntities><Ship/></ClientServerEntities></root>"),
            (
                "entity_defs/alias.xml",
                "<root><BOOL>UINT8</BOOL>
                 <POS> FIXED_DICT <Properties><x><Type>FLOAT</Type></x><y><Type>FLOAT</Type></y></Properties></POS>
                 <MAYBE> FIXED_DICT <AllowNone>true</AllowNone><Properties><a><Type>BOOL</Type></a></Properties></MAYBE>
                 <IDS> ARRAY <of>UINT32</of></IDS></root>",
            ),
            (
                "entity_defs/Ship.def",
                "<root><Implements><Interface>Owner</Interface></Implements>
                 <Properties>
                   <name><Type>STRING</Type><Flags>ALL_CLIENTS</Flags></name>
                   <alive><Type>BOOL</Type><Flags>ALL_CLIENTS</Flags></alive>
                   <secret><Type>UINT32</Type><Flags>CELL_PRIVATE</Flags></secret>
                 </Properties>
                 <ClientMethods>
                   <onList><Arg>IDS</Arg></onList>
                   <onPos><Arg>POS</Arg><Arg>BOOL</Arg></onPos>
                   <onNothing/>
                 </ClientMethods></root>",
            ),
            (
                "entity_defs/interfaces/Owner.def",
                "<root><Properties><owner><Type>INT32</Type><Flags>OWN_CLIENT</Flags></owner></Properties>
                 <ClientMethods><onOwner><Args><who>INT32</who></Args></onOwner></ClientMethods></root>",
            ),
        ]))
    }

    #[test]
    fn orders_members_like_the_engine() {
        let defs = EntityDefs::load(&sample()).unwrap();
        let ship = defs.by_type_id(1).unwrap();
        let methods: Vec<_> = ship.client_methods.iter().map(|m| m.name.as_str()).collect();
        // sizes: onOwner 4+1, onList inf+1, onPos 9+1, onNothing 0+1
        assert_eq!(methods, ["onNothing", "onOwner", "onPos", "onList"]);
        let props: Vec<_> = ship.client_properties.iter().map(|p| p.name.as_str()).collect();
        // owner(4) from the interface first, then name(inf), alive(1); stable sort by size.
        assert_eq!(props, ["alive", "owner", "name"]);
        let cell: Vec<_> = ship.cell_player_properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(cell, ["owner", "name", "alive"]);
    }

    #[test]
    fn nullable_dicts_and_user_types_count_as_variable() {
        let defs = EntityDefs::load(&sample()).unwrap();
        let ship = defs.by_type_id(1).unwrap();
        let on_pos = ship.client_methods.iter().find(|m| m.name == "onPos").unwrap();
        assert_eq!(on_pos.fixed_args_size(), 9);
        assert_eq!(Type::User(Box::new(Type::Primitive(Primitive::U8))).sort_size(), INFINITE);
        let aliases = load_aliases(&sample()).unwrap();
        assert_eq!(aliases["MAYBE"].sort_size(), INFINITE);
    }
}
