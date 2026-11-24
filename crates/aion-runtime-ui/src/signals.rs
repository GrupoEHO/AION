use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use aion_ast::{Expr, ExprKind, NodeId, NodeKind, Program, UiArg, UiBody};
use serde::{Deserialize, Serialize};

use crate::UiError;

/// Runtime value stored in a reactive `#S` signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SignalValue {
    I32(i32),
    F64(f64),
    Str(String),
    Bool(bool),
    Null,
}

impl fmt::Display for SignalValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalValue::I32(n) => write!(f, "{n}"),
            SignalValue::F64(v) => write!(f, "{v}"),
            SignalValue::Str(s) => f.write_str(s),
            SignalValue::Bool(b) => write!(f, "{b}"),
            SignalValue::Null => f.write_str("null"),
        }
    }
}

/// Reactive state graph mapping `#S` signals to subscribed `#N` UI nodes.
///
/// Mutating a signal marks *only* its subscribed `#N` nodes as dirty.
#[derive(Debug, Clone, Default)]
pub struct SignalStore {
    values: BTreeMap<NodeId, SignalValue>,
    subscribers: BTreeMap<NodeId, BTreeSet<NodeId>>,
    dirty_nodes: BTreeSet<NodeId>,
}

impl SignalStore {
    pub fn from_program(program: &Program) -> Self {
        let mut store = Self::default();

        for node in &program.nodes {
            if let NodeKind::Signal(sig) = &node.kind {
                let initial = eval_signal_init(&sig.init);
                store.values.insert(node.id.clone(), initial);
                store.subscribers.entry(node.id.clone()).or_default();
            }
        }

        for node in &program.nodes {
            if let NodeKind::Ui(ui) = &node.kind {
                if let UiBody::Text { interpolations, .. } = &ui.body {
                    for sig_id in interpolations {
                        store
                            .subscribers
                            .entry(sig_id.clone())
                            .or_default()
                            .insert(node.id.clone());
                    }
                }
                for arg in &ui.args {
                    if let UiArg::NodeRef(sig_id) = arg {
                        if store.values.contains_key(sig_id) {
                            store
                                .subscribers
                                .entry(sig_id.clone())
                                .or_default()
                                .insert(node.id.clone());
                        }
                    }
                }
            }
        }

        store
    }

    pub fn get(&self, signal_id: &NodeId) -> Option<&SignalValue> {
        self.values.get(signal_id)
    }

    /// Updates `signal_id` and marks only subscribed `#N` nodes as dirty.
    /// Returns the list of `#N` node IDs marked dirty by this mutation.
    pub fn set(
        &mut self,
        signal_id: &NodeId,
        value: SignalValue,
    ) -> Result<Vec<NodeId>, UiError> {
        let Some(slot) = self.values.get_mut(signal_id) else {
            return Err(UiError::UnknownSignal(signal_id.clone()));
        };
        if *slot == value {
            return Ok(Vec::new());
        }
        *slot = value;

        let mut dirtied = Vec::new();
        if let Some(subs) = self.subscribers.get(signal_id) {
            for ui_id in subs {
                self.dirty_nodes.insert(ui_id.clone());
                dirtied.push(ui_id.clone());
            }
        }
        Ok(dirtied)
    }

    pub fn subscribers(&self, signal_id: &NodeId) -> Vec<NodeId> {
        self.subscribers
            .get(signal_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn is_dirty(&self, ui_id: &NodeId) -> bool {
        self.dirty_nodes.contains(ui_id)
    }

    pub fn dirty_nodes(&self) -> &BTreeSet<NodeId> {
        &self.dirty_nodes
    }

    pub fn take_dirty(&mut self) -> BTreeSet<NodeId> {
        std::mem::take(&mut self.dirty_nodes)
    }

    /// Resolves `{#S0}` interpolations inside a text template using current signal values.
    pub fn resolve_text(&self, template: &str) -> String {
        let mut out = template.to_string();
        for (sig_id, val) in &self.values {
            let needle = format!("{{{sig_id}}}");
            if out.contains(&needle) {
                out = out.replace(&needle, &val.to_string());
            }
        }
        out
    }
}

fn eval_signal_init(expr: &Expr) -> SignalValue {
    match &expr.kind {
        ExprKind::Int(n) => SignalValue::I32(*n as i32),
        ExprKind::Float(f) => SignalValue::F64(*f),
        ExprKind::Str(s) => SignalValue::Str(s.clone()),
        ExprKind::Bool(b) => SignalValue::Bool(*b),
        _ => SignalValue::Null,
    }
}
