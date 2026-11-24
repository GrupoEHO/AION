use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use crate::diagnostic::Span;
use crate::types::AionType;

/// Node prefix category in AION's flat graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodePrefix {
    Cfg,
    Ext,
    Srv,
    Mw,
    Api,
    Model,
    Signal,
    Ui,
}

impl fmt::Display for NodePrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodePrefix::Cfg => write!(f, "CFG"),
            NodePrefix::Ext => write!(f, "EXT"),
            NodePrefix::Srv => write!(f, "SRV"),
            NodePrefix::Mw => write!(f, "MW"),
            NodePrefix::Api => write!(f, "API"),
            NodePrefix::Model => write!(f, "M"),
            NodePrefix::Signal => write!(f, "S"),
            NodePrefix::Ui => write!(f, "N"),
        }
    }
}

/// Canonical identifier for any flat node (e.g. `#CFG`, `#EXT:db`, `#SRV0`, `#API0`, `#N3`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if !trimmed.starts_with('#') || trimmed.len() < 2 {
            return Err(format!("invalid node id `{trimmed}`: must start with `#`"));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Infers the node prefix category from its canonical string.
    pub fn prefix(&self) -> Option<NodePrefix> {
        let body = self.0.strip_prefix('#')?;
        if body == "CFG" || body.starts_with("CFG:") {
            Some(NodePrefix::Cfg)
        } else if body == "EXT" || body.starts_with("EXT:") {
            Some(NodePrefix::Ext)
        } else if body.starts_with("SRV") {
            Some(NodePrefix::Srv)
        } else if body.starts_with("MW") {
            Some(NodePrefix::Mw)
        } else if body.starts_with("API") {
            Some(NodePrefix::Api)
        } else if body.starts_with('M')
            && body[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            Some(NodePrefix::Model)
        } else if body.starts_with('S')
            && body[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            Some(NodePrefix::Signal)
        } else if body.starts_with('N')
            && body[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            Some(NodePrefix::Ui)
        } else {
            None
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Complete flat-graph AION program.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Program {
    pub nodes: Vec<FlatNode>,
}

impl Program {
    pub fn new(nodes: Vec<FlatNode>) -> Self {
        Self { nodes }
    }

    pub fn get(&self, id: &NodeId) -> Option<&FlatNode> {
        self.nodes.iter().find(|n| &n.id == id)
    }

    pub fn get_mut(&mut self, id: &NodeId) -> Option<&mut FlatNode> {
        self.nodes.iter_mut().find(|n| &n.id == id)
    }

    pub fn index_map(&self) -> HashMap<&NodeId, &FlatNode> {
        self.nodes.iter().map(|n| (&n.id, n)).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlatNode {
    pub id: NodeId,
    pub span: Span,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "node_type", rename_all = "snake_case")]
pub enum NodeKind {
    Cfg(CfgNode),
    Ext(ExtNode),
    Srv(SrvNode),
    Mw(MwNode),
    Model(ModelNode),
    Api(ApiNode),
    Signal(SignalNode),
    Ui(UiNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfgNode {
    pub kind: String,
    pub vars: Vec<CfgVar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfgVar {
    pub name: String,
    pub ty: AionType,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtNode {
    pub alias: String,
    pub module_id: String,
    pub version_req: String,
    pub raw_spec: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrvNode {
    pub protocol: String,
    pub props: Vec<SrvProp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrvProp {
    pub key: String,
    pub value: PropValue,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Ident(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MwNode {
    pub kind: String,
    pub ctx_param: String,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelNode {
    pub name: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldDef {
    pub name: String,
    pub ty: AionType,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn from_ident(s: &str) -> Option<Self> {
        match s {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "out_kind", content = "value", rename_all = "snake_case")]
pub enum ApiOutput {
    ModelRef(NodeId),
    Fields(Vec<FieldDef>),
    Type(AionType),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiNode {
    pub srv_ref: NodeId,
    pub method: HttpMethod,
    pub path: String,
    pub middlewares: Vec<NodeId>,
    pub input_fields: Vec<FieldDef>,
    pub output: Option<ApiOutput>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalNode {
    pub ty: AionType,
    pub init: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiNode {
    pub widget: String,
    pub args: Vec<UiArg>,
    pub body: UiBody,
    pub events: Vec<EventHandler>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "arg_kind", content = "value", rename_all = "snake_case")]
pub enum UiArg {
    Int(i64),
    Float(f64),
    Percent(f64),
    HexColor(String),
    Ident(String),
    Str(String),
    NodeRef(NodeId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "body_kind", content = "value", rename_all = "snake_case")]
pub enum UiBody {
    Empty,
    Children(Vec<NodeId>),
    Text {
        template: String,
        interpolations: Vec<NodeId>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventHandler {
    pub event: String,
    pub action: EventAction,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action_kind", rename_all = "snake_case")]
pub enum EventAction {
    Call {
        api: NodeId,
        payload: Expr,
        bind: Option<SignalAssign>,
    },
    Toast {
        message: Expr,
    },
    Set {
        signal: NodeId,
        value: Expr,
    },
    Generic {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalAssign {
    pub signal: NodeId,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stmt_kind", rename_all = "snake_case")]
pub enum Stmt {
    Let {
        name: String,
        ty: Option<AionType>,
        value: Expr,
        fallback_err: Option<ErrAction>,
        span: Span,
    },
    Expr {
        expr: Expr,
        fallback_err: Option<ErrAction>,
        flow: Option<FlowControl>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "flow_kind", content = "value", rename_all = "snake_case")]
pub enum FlowControl {
    Next,
    Ret,
    Err(ErrAction),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrAction {
    pub status: i32,
    pub message: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "expr_kind", content = "value", rename_all = "snake_case")]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Ident(String),
    NodeRef(NodeId),
    Member {
        object: Box<Expr>,
        field: String,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Object(Vec<(String, Expr)>),
    List(Vec<Expr>),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
}
