use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ast::{
    EventHandler, Expr, FieldDef, FlatNode, NodeId, NodeKind, Program, Stmt, UiArg, UiBody,
};

/// Differential patch operation on AION's flat-graph AST (`ADD`, `PATCH`, `DEL`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "UPPERCASE")]
pub enum PatchOp {
    Add {
        node: FlatNode,
        #[serde(default)]
        after: Option<NodeId>,
    },
    Patch {
        id: NodeId,
        delta: PatchDelta,
    },
    Del {
        id: NodeId,
    },
}

/// Granular mutation payload for `PATCH` operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PatchDelta {
    Replace(FlatNode),
    SetUiArgs { args: Vec<UiArg> },
    SetUiBody { body: UiBody },
    UpsertUiEvent { handler: EventHandler },
    RemoveUiEvent { event: String },
    SetSignalInit { init: Expr },
    UpsertModelField { field: FieldDef },
    RemoveModelField { name: String },
    SetBody { body: Vec<Stmt> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchReport {
    pub applied: usize,
    pub total_nodes: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchError {
    #[error("node `{0}` already exists")]
    DuplicateNode(NodeId),
    #[error("node `{0}` not found")]
    NodeNotFound(NodeId),
    #[error("anchor node `{0}` for ADD not found")]
    AnchorNotFound(NodeId),
    #[error("patch delta `{delta}` is incompatible with node `{id}`")]
    IncompatibleDelta { id: NodeId, delta: String },
    #[error("replacement node id `{found}` does not match target `{expected}`")]
    IdMismatch { expected: NodeId, found: NodeId },
}

impl Program {
    /// Applies a single differential patch (`ADD`, `PATCH`, `DEL`) to the flat graph.
    pub fn apply_patch(&mut self, op: &PatchOp) -> Result<(), PatchError> {
        match op {
            PatchOp::Add { node, after } => {
                if self.get(&node.id).is_some() {
                    return Err(PatchError::DuplicateNode(node.id.clone()));
                }
                match after {
                    Some(anchor) => {
                        let pos = self
                            .nodes
                            .iter()
                            .position(|n| &n.id == anchor)
                            .ok_or_else(|| PatchError::AnchorNotFound(anchor.clone()))?;
                        self.nodes.insert(pos + 1, node.clone());
                    }
                    None => self.nodes.push(node.clone()),
                }
                Ok(())
            }
            PatchOp::Del { id } => {
                let pos = self
                    .nodes
                    .iter()
                    .position(|n| &n.id == id)
                    .ok_or_else(|| PatchError::NodeNotFound(id.clone()))?;
                self.nodes.remove(pos);
                Ok(())
            }
            PatchOp::Patch { id, delta } => {
                let target = self
                    .get_mut(id)
                    .ok_or_else(|| PatchError::NodeNotFound(id.clone()))?;
                apply_delta(target, delta)
            }
        }
    }

    /// Transactionally applies a sequence of differential patches.
    /// If any patch fails, the program remains unchanged.
    pub fn apply_patches(&mut self, ops: &[PatchOp]) -> Result<PatchReport, PatchError> {
        let mut staged = self.clone();
        for op in ops {
            staged.apply_patch(op)?;
        }
        *self = staged;
        Ok(PatchReport {
            applied: ops.len(),
            total_nodes: self.nodes.len(),
        })
    }
}

fn apply_delta(target: &mut FlatNode, delta: &PatchDelta) -> Result<(), PatchError> {
    match delta {
        PatchDelta::Replace(new_node) => {
            if new_node.id != target.id {
                return Err(PatchError::IdMismatch {
                    expected: target.id.clone(),
                    found: new_node.id.clone(),
                });
            }
            *target = new_node.clone();
            Ok(())
        }
        PatchDelta::SetUiArgs { args } => match &mut target.kind {
            NodeKind::Ui(ui) => {
                ui.args = args.clone();
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "set_ui_args".to_string(),
            }),
        },
        PatchDelta::SetUiBody { body } => match &mut target.kind {
            NodeKind::Ui(ui) => {
                ui.body = body.clone();
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "set_ui_body".to_string(),
            }),
        },
        PatchDelta::UpsertUiEvent { handler } => match &mut target.kind {
            NodeKind::Ui(ui) => {
                if let Some(existing) = ui.events.iter_mut().find(|h| h.event == handler.event) {
                    *existing = handler.clone();
                } else {
                    ui.events.push(handler.clone());
                }
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "upsert_ui_event".to_string(),
            }),
        },
        PatchDelta::RemoveUiEvent { event } => match &mut target.kind {
            NodeKind::Ui(ui) => {
                ui.events.retain(|h| &h.event != event);
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "remove_ui_event".to_string(),
            }),
        },
        PatchDelta::SetSignalInit { init } => match &mut target.kind {
            NodeKind::Signal(sig) => {
                sig.init = init.clone();
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "set_signal_init".to_string(),
            }),
        },
        PatchDelta::UpsertModelField { field } => match &mut target.kind {
            NodeKind::Model(m) => {
                if let Some(existing) = m.fields.iter_mut().find(|f| f.name == field.name) {
                    *existing = field.clone();
                } else {
                    m.fields.push(field.clone());
                }
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "upsert_model_field".to_string(),
            }),
        },
        PatchDelta::RemoveModelField { name } => match &mut target.kind {
            NodeKind::Model(m) => {
                m.fields.retain(|f| &f.name != name);
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "remove_model_field".to_string(),
            }),
        },
        PatchDelta::SetBody { body } => match &mut target.kind {
            NodeKind::Mw(mw) => {
                mw.body = body.clone();
                Ok(())
            }
            NodeKind::Api(api) => {
                api.body = body.clone();
                Ok(())
            }
            _ => Err(PatchError::IncompatibleDelta {
                id: target.id.clone(),
                delta: "set_body".to_string(),
            }),
        },
    }
}
