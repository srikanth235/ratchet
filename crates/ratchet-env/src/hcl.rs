//! A small, tolerant reader for the HCL subset a drafted environment is written in (`hcl.ts`). STUB: see PARITY.md.

#[derive(Debug, Clone, PartialEq)]
pub enum HclValue {
    String {
        value: String,
        dynamic: bool,
    },
    Number(f64),
    Bool(bool),
    Null,
    /// Entries in source order.
    Object(Vec<(String, HclValue)>),
    List(Vec<HclValue>),
    /// Raw text of whatever the reader could not read as a literal.
    Expr(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HclBlock {
    /// `resource`, `module`, `data`, `variable`, `output`, `provider`, `terraform`, `locals`, `lifecycle`, ...
    pub block_type: String,
    pub labels: Vec<String>,
    /// Attributes in source order.
    pub attrs: Vec<(String, HclValue)>,
    pub blocks: Vec<HclBlock>,
    /// The block's source text (header and body).
    pub raw: String,
    /// 1-based line of the block header.
    pub line: usize,
}

impl HclBlock {
    pub fn attr(&self, name: &str) -> Option<&HclValue> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HclFile {
    pub blocks: Vec<HclBlock>,
    pub errors: Vec<String>,
}

pub fn strip_comments(_src: &str) -> String {
    todo!("port hcl.ts")
}

pub fn parse_hcl(_source: &str) -> HclFile {
    todo!("port hcl.ts")
}

pub fn literal_string(v: Option<&HclValue>) -> Option<String> {
    match v {
        Some(HclValue::String {
            value,
            dynamic: false,
        }) => Some(value.clone()),
        _ => None,
    }
}

pub fn literal_number(v: Option<&HclValue>) -> Option<f64> {
    match v {
        Some(HclValue::Number(n)) => Some(*n),
        _ => None,
    }
}

/// Every block (each one included) in a tree, depth first.
pub fn walk_blocks(blocks: &[HclBlock]) -> Vec<&HclBlock> {
    let mut out = vec![];
    for b in blocks {
        out.push(b);
        out.extend(walk_blocks(&b.blocks));
    }
    out
}
