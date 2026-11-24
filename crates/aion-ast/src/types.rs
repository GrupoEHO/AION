use serde::{Deserialize, Serialize};
use std::fmt;

use crate::ast::NodeId;

/// Primitive and composite types in the AION type system.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AionType {
    I32,
    F64,
    Str,
    Bool,
    Bytes,
    Ts,
    List(Box<AionType>),
    Map(Box<AionType>, Box<AionType>),
    Void,
    Any,
    /// Reference to a `#M` model node (e.g. `#M0`).
    ModelRef(NodeId),
    /// Reference to an external ABI type `@Type` defined by an `#EXT` module.
    ExtType {
        ext_alias: Option<String>,
        name: String,
    },
    /// Unresolved named type before checking.
    Named(String),
    Unknown,
}

impl AionType {
    /// Parses a canonical AION type string (e.g. `i32`, `list<str>`, `map<str, any>`, `@AuthUser`, `#M0`).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        if s.is_empty() {
            return Err("empty type string".to_string());
        }
        match s {
            "i32" => return Ok(Self::I32),
            "f64" => return Ok(Self::F64),
            "str" => return Ok(Self::Str),
            "bool" => return Ok(Self::Bool),
            "bytes" => return Ok(Self::Bytes),
            "ts" => return Ok(Self::Ts),
            "void" => return Ok(Self::Void),
            "any" => return Ok(Self::Any),
            _ => {}
        }

        if let Some(inner) = s.strip_prefix("list<").and_then(|r| r.strip_suffix('>')) {
            let elem = Self::parse(inner)?;
            return Ok(Self::List(Box::new(elem)));
        }

        if let Some(inner) = s.strip_prefix("map<").and_then(|r| r.strip_suffix('>')) {
            let (k_str, v_str) = split_top_level_comma(inner)
                .ok_or_else(|| format!("invalid map type syntax: `{s}`"))?;
            let key = Self::parse(k_str)?;
            let val = Self::parse(v_str)?;
            return Ok(Self::Map(Box::new(key), Box::new(val)));
        }

        if let Some(type_name) = s.strip_prefix('@') {
            if type_name.is_empty() {
                return Err(format!("empty external type name in `{s}`"));
            }
            return Ok(Self::ExtType {
                ext_alias: None,
                name: type_name.to_string(),
            });
        }

        if s.starts_with('#') {
            let node_id = NodeId::parse(s)?;
            return Ok(Self::ModelRef(node_id));
        }

        if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Ok(Self::Named(s.to_string()));
        }

        Err(format!("unsupported type syntax: `{s}`"))
    }

    /// Returns true if a value of type `self` can be assigned/passed where `expected` is required.
    pub fn is_assignable_to(&self, expected: &AionType) -> bool {
        if matches!(self, AionType::Any) || matches!(expected, AionType::Any) {
            return true;
        }
        if matches!(self, AionType::Unknown) || matches!(expected, AionType::Unknown) {
            return false;
        }
        match (self, expected) {
            (AionType::List(a), AionType::List(b)) => a.is_assignable_to(b),
            (AionType::Map(k1, v1), AionType::Map(k2, v2)) => {
                k1.is_assignable_to(k2) && v1.is_assignable_to(v2)
            }
            (
                AionType::ExtType {
                    ext_alias: a1,
                    name: n1,
                },
                AionType::ExtType {
                    ext_alias: a2,
                    name: n2,
                },
            ) => {
                n1 == n2
                    && match (a1, a2) {
                        (Some(x), Some(y)) => x == y,
                        _ => true,
                    }
            }
            _ => self == expected,
        }
    }
}

fn split_top_level_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (idx, ch) in s.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                return Some((s[..idx].trim(), s[idx + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

impl fmt::Display for AionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AionType::I32 => write!(f, "i32"),
            AionType::F64 => write!(f, "f64"),
            AionType::Str => write!(f, "str"),
            AionType::Bool => write!(f, "bool"),
            AionType::Bytes => write!(f, "bytes"),
            AionType::Ts => write!(f, "ts"),
            AionType::List(inner) => write!(f, "list<{inner}>"),
            AionType::Map(k, v) => write!(f, "map<{k}, {v}>"),
            AionType::Void => write!(f, "void"),
            AionType::Any => write!(f, "any"),
            AionType::ModelRef(id) => write!(f, "{id}"),
            AionType::ExtType { ext_alias, name } => match ext_alias {
                Some(alias) => write!(f, "{alias}.@{name}"),
                None => write!(f, "@{name}"),
            },
            AionType::Named(name) => write!(f, "{name}"),
            AionType::Unknown => write!(f, "<unknown>"),
        }
    }
}
