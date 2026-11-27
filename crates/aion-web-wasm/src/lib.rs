use aion_ast::{NodeId, NodeKind, Program};
use aion_runtime_ui::{compute_layout, SignalStore, SignalValue, Viewport};
use serde_json::json;
use wasm_bindgen::prelude::*;

/// WebAssembly runtime executing AION's Rust `taffy` Flexbox layout engine and `Signals`
/// reactive state graph directly inside the browser.
#[wasm_bindgen]
pub struct AionWasmApp {
    program: Program,
    signals: SignalStore,
}

#[wasm_bindgen]
impl AionWasmApp {
    /// Initializes the WASM engine from a compiled `Program` JSON string (or `{ program: ... }` bundle).
    #[wasm_bindgen(constructor)]
    pub fn new(bundle_or_program_json: &str) -> Result<AionWasmApp, JsValue> {
        let val: serde_json::Value = serde_json::from_str(bundle_or_program_json)
            .map_err(|e| JsValue::from_str(&format!("invalid AION bundle JSON: {e}")))?;

        let prog_val = val.get("program").cloned().unwrap_or(val);
        let program: Program = serde_json::from_value(prog_val)
            .map_err(|e| JsValue::from_str(&format!("invalid AION Program AST: {e}")))?;
        let signals = SignalStore::from_program(&program);

        Ok(Self { program, signals })
    }

    /// Computes live Flexbox layout boxes `(x, y, width, height)` for any browser viewport size
    /// using Rust's `taffy` engine inside WebAssembly.
    pub fn compute_layout_json(&self, width: f32, height: f32) -> Result<String, JsValue> {
        let viewport = Viewport::custom("wasm", width.max(320.0), height.max(320.0));
        let layout = compute_layout(&self.program, viewport, Some(&self.signals))
            .map_err(|e| JsValue::from_str(&format!("taffy layout error: {e}")))?;
        serde_json::to_string(&layout)
            .map_err(|e| JsValue::from_str(&format!("serialize layout error: {e}")))
    }

    /// Mutates a `#S` signal inside WASM memory and returns JSON `{ dirtied: [...], signals: {...} }`.
    pub fn set_signal(&mut self, signal_id: &str, value_json: &str) -> Result<String, JsValue> {
        let jv: serde_json::Value = serde_json::from_str(value_json)
            .map_err(|e| JsValue::from_str(&format!("invalid signal value JSON: {e}")))?;
        let sig_val = json_to_signal_value(&jv);
        let dirtied = self
            .signals
            .set(&NodeId::new(signal_id), sig_val)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let dirtied_strs: Vec<&str> = dirtied.iter().map(|id| id.as_str()).collect();
        let out = json!({
            "dirtied": dirtied_strs,
            "signals": self.signals_map(),
        });
        Ok(out.to_string())
    }

    /// Resolves `{#S0}` template interpolations using current WASM signal values.
    pub fn resolve_text(&self, template: &str) -> String {
        self.signals.resolve_text(template)
    }

    /// Returns all current `#S` signal values as a JSON object string.
    pub fn signals_json(&self) -> String {
        serde_json::Value::Object(self.signals_map()).to_string()
    }
}

impl AionWasmApp {
    fn signals_map(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        for node in &self.program.nodes {
            if let NodeKind::Signal(_) = &node.kind {
                if let Some(v) = self.signals.get(&node.id) {
                    map.insert(
                        node.id.as_str().to_string(),
                        serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
                    );
                }
            }
        }
        map
    }
}

fn json_to_signal_value(jv: &serde_json::Value) -> SignalValue {
    match jv {
        serde_json::Value::Null => SignalValue::Null,
        serde_json::Value::Bool(b) => SignalValue::Bool(*b),
        serde_json::Value::String(s) => SignalValue::Str(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SignalValue::I32(i as i32)
            } else if let Some(f) = n.as_f64() {
                SignalValue::F64(f)
            } else {
                SignalValue::Null
            }
        }
        other => SignalValue::Str(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_app_computes_layout_and_mutates_signals() {
        let prog = Program::new(vec![
            aion_ast::FlatNode {
                id: NodeId::new("#S0"),
                span: aion_ast::Span::default(),
                kind: NodeKind::Signal(aion_ast::SignalNode {
                    ty: aion_ast::AionType::I32,
                    init: aion_ast::Expr {
                        kind: aion_ast::ExprKind::Int(0),
                        span: aion_ast::Span::default(),
                    },
                }),
            },
            aion_ast::FlatNode {
                id: NodeId::new("#N0"),
                span: aion_ast::Span::default(),
                kind: NodeKind::Ui(aion_ast::UiNode {
                    widget: "ROOT".to_string(),
                    args: vec![
                        aion_ast::UiArg::Int(0),
                        aion_ast::UiArg::Int(0),
                        aion_ast::UiArg::Percent(100.0),
                        aion_ast::UiArg::Percent(100.0),
                        aion_ast::UiArg::HexColor("#F8FAFC".to_string()),
                    ],
                    body: aion_ast::UiBody::Children(vec![NodeId::new("#N1")]),
                    events: vec![],
                }),
            },
            aion_ast::FlatNode {
                id: NodeId::new("#N1"),
                span: aion_ast::Span::default(),
                kind: NodeKind::Ui(aion_ast::UiNode {
                    widget: "TXT".to_string(),
                    args: vec![
                        aion_ast::UiArg::Int(18),
                        aion_ast::UiArg::Int(600),
                        aion_ast::UiArg::HexColor("#0F172A".to_string()),
                    ],
                    body: aion_ast::UiBody::Text {
                        template: "Total: {#S0}".to_string(),
                        interpolations: vec![NodeId::new("#S0")],
                    },
                    events: vec![],
                }),
            },
        ]);

        let json = serde_json::to_string(&prog).unwrap();
        let mut app = AionWasmApp::new(&json).unwrap();
        let layout_str = app.compute_layout_json(390.0, 844.0).unwrap();
        assert!(layout_str.contains("\"#N0\""));
        assert_eq!(app.resolve_text("Total: {#S0}"), "Total: 0");

        let update = app.set_signal("#S0", "42").unwrap();
        assert!(update.contains("\"#N1\""));
        assert_eq!(app.resolve_text("Total: {#S0}"), "Total: 42");
    }
}
