use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use aion_ast::{
    ApiNode, EventAction, Expr, ExprKind, HttpMethod, NodeId, NodeKind, Program, PropValue,
};
use aion_ext::ExtValue;
use aion_runtime_net::{json_to_ext_value, ProtoSchema, JSON_MIME, PROTOBUF_MIME};
use async_trait::async_trait;

use crate::signals::{SignalStore, SignalValue};
use crate::UiError;

/// Outgoing HTTP/Protobuf request built by the UI bridge for `CALL(#API..., payload)`.
#[derive(Debug, Clone)]
pub struct UiHttpRequest {
    pub method: HttpMethod,
    pub base_url: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Incoming HTTP/Protobuf response delivered to the UI bridge.
#[derive(Debug, Clone)]
pub struct UiHttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// Pluggable async transport used by [`UiRuntime`] (real `reqwest` or in-memory `axum::Router`).
#[async_trait]
pub trait UiHttpTransport: Send + Sync {
    async fn send(&self, req: UiHttpRequest) -> Result<UiHttpResponse, String>;
}

/// Production HTTP/Protobuf transport backed by `reqwest`.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UiHttpTransport for ReqwestTransport {
    async fn send(&self, req: UiHttpRequest) -> Result<UiHttpResponse, String> {
        let url = format!("{}{}", req.base_url.trim_end_matches('/'), req.path);
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
        };

        let mut builder = self.client.request(method, &url).body(req.body);
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        let resp = builder.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(JSON_MIME)
            .to_string();
        let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();

        Ok(UiHttpResponse {
            status,
            content_type,
            body,
        })
    }
}

/// Observable lifecycle events emitted during UI event execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiLifecycleEvent {
    LoadingStarted(NodeId),
    LoadingFinished(NodeId),
    Toast(String),
    SignalUpdated { signal: NodeId, dirtied: Vec<NodeId> },
    Navigated(NodeId),
}

/// Result of dispatching a UI event (`@tap`, etc.).
#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOutcome {
    Ok {
        response: ExtValue,
        dirtied_nodes: Vec<NodeId>,
    },
    ErrorHandled {
        status: Option<u16>,
        message: String,
    },
    NoOp,
}

/// Interactive UI runtime connecting `#S` signals, `#N` events, `@loading`/`@err` callbacks,
/// and typed Protobuf `CALL(#API...)` execution.
pub struct UiRuntime {
    pub program: Program,
    pub signals: SignalStore,
    pub schema: ProtoSchema,
    pub transport: Arc<dyn UiHttpTransport>,
    pub default_headers: HashMap<String, String>,
    pub toasts: Vec<String>,
    pub loading_nodes: BTreeSet<NodeId>,
    pub active_route: Option<NodeId>,
    pub events_log: Vec<UiLifecycleEvent>,
}

impl UiRuntime {
    pub fn new(
        program: Program,
        transport: Arc<dyn UiHttpTransport>,
    ) -> Result<Self, UiError> {
        let signals = SignalStore::from_program(&program);
        let schema = ProtoSchema::from_program(&program)
            .map_err(|e| UiError::Bridge(format!("failed to build proto schema: {e}")))?;
        Ok(Self {
            program,
            signals,
            schema,
            transport,
            default_headers: HashMap::new(),
            toasts: Vec::new(),
            loading_nodes: BTreeSet::new(),
            active_route: None,
            events_log: Vec::new(),
        })
    }

    /// Updates the `#S` signal bound to an `INP[..., #S_target]` widget.
    pub fn input_change(&mut self, inp_node_id: &NodeId, text: impl Into<String>) -> Result<Vec<NodeId>, UiError> {
        let bound_sig = match self.program.get(inp_node_id) {
            Some(node) => match &node.kind {
                NodeKind::Ui(ui) => ui.args.get(4).and_then(|a| match a {
                    aion_ast::UiArg::NodeRef(id) => Some(id.clone()),
                    _ => None,
                }),
                _ => None,
            },
            None => return Err(UiError::Bridge(format!("unknown UI node `{inp_node_id}`"))),
        };
        let Some(sig_id) = bound_sig else {
            return Ok(Vec::new());
        };
        let dirtied = self.signals.set(&sig_id, SignalValue::Str(text.into()))?;
        self.events_log.push(UiLifecycleEvent::SignalUpdated {
            signal: sig_id,
            dirtied: dirtied.clone(),
        });
        Ok(dirtied)
    }

    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.default_headers.insert(name.into(), value.into());
    }

    /// Dispatches a synthetic or interactive UI event (e.g. `"tap"`) on `node_id`.
    pub async fn dispatch_event(
        &mut self,
        node_id: &NodeId,
        event_name: &str,
    ) -> Result<DispatchOutcome, UiError> {
        let ui = match self.program.get(node_id) {
            Some(node) => match &node.kind {
                NodeKind::Ui(ui) => ui.clone(),
                _ => return Err(UiError::Bridge(format!("`{node_id}` is not a UI node"))),
            },
            None => return Err(UiError::Bridge(format!("unknown UI node `{node_id}`"))),
        };

        let Some(handler) = ui.events.iter().find(|h| h.event == event_name) else {
            return Ok(DispatchOutcome::NoOp);
        };

        match &handler.action {
            EventAction::Call { api, payload, bind } => {
                // 1. Enter @loading state.
                self.loading_nodes.insert(node_id.clone());
                self.events_log
                    .push(UiLifecycleEvent::LoadingStarted(node_id.clone()));
                if let Some(loading_handler) = ui.events.iter().find(|h| h.event == "loading") {
                    self.run_local_action(&loading_handler.action, None)?;
                }

                // 2. Locate target `#API` and `#SRV`.
                let api_node = self.find_api(api)?.clone();
                let base_url = self.resolve_server_url(&api_node.srv_ref);

                // 3. Serialize `IN` payload into binary Protobuf.
                let payload_val = eval_ui_expr(payload, &self.signals, None);
                let body = self
                    .schema
                    .encode_input(api, &payload_val)
                    .map_err(|e| UiError::Bridge(format!("protobuf encode error: {e}")))?;

                let mut headers = self.default_headers.clone();
                headers.insert("content-type".to_string(), PROTOBUF_MIME.to_string());
                headers.insert("accept".to_string(), PROTOBUF_MIME.to_string());

                let req = UiHttpRequest {
                    method: api_node.method,
                    base_url,
                    path: api_node.path.clone(),
                    headers,
                    body,
                };

                // 4. Execute request over transport.
                let transport_res = self.transport.send(req).await;

                // 5. Exit @loading state.
                self.loading_nodes.remove(node_id);
                self.events_log
                    .push(UiLifecycleEvent::LoadingFinished(node_id.clone()));

                let resp = match transport_res {
                    Ok(r) if (200..300).contains(&r.status) => r,
                    Ok(r) => {
                        let err_msg = format!("HTTP {}", r.status);
                        if let Some(err_handler) = ui.events.iter().find(|h| h.event == "err") {
                            self.run_local_action(&err_handler.action, None)?;
                        }
                        return Ok(DispatchOutcome::ErrorHandled {
                            status: Some(r.status),
                            message: err_msg,
                        });
                    }
                    Err(net_err) => {
                        if let Some(err_handler) = ui.events.iter().find(|h| h.event == "err") {
                            self.run_local_action(&err_handler.action, None)?;
                        }
                        return Ok(DispatchOutcome::ErrorHandled {
                            status: None,
                            message: net_err,
                        });
                    }
                };

                // 6. Decode `OUT` payload (Protobuf or JSON fallback).
                let decoded = if resp.content_type.contains("protobuf") {
                    self.schema
                        .decode_output(api, &resp.body)
                        .map_err(|e| UiError::Bridge(format!("protobuf decode error: {e}")))?
                } else if resp.body.is_empty() {
                    ExtValue::Void
                } else {
                    let jv: serde_json::Value = serde_json::from_slice(&resp.body)
                        .map_err(|e| UiError::Bridge(format!("json decode error: {e}")))?;
                    json_to_ext_value(&jv)
                };

                // 7. Apply signal binding `=> #S0 = res.pts`.
                let mut dirtied_nodes = Vec::new();
                if let Some(assign) = bind {
                    let bound_ext = eval_ui_expr(&assign.value, &self.signals, Some(&decoded));
                    let sig_val = ext_to_signal_value(&bound_ext);
                    let dirtied = self.signals.set(&assign.signal, sig_val)?;
                    self.events_log.push(UiLifecycleEvent::SignalUpdated {
                        signal: assign.signal.clone(),
                        dirtied: dirtied.clone(),
                    });
                    dirtied_nodes = dirtied;
                }

                Ok(DispatchOutcome::Ok {
                    response: decoded,
                    dirtied_nodes,
                })
            }
            other => {
                self.run_local_action(other, None)?;
                Ok(DispatchOutcome::Ok {
                    response: ExtValue::Void,
                    dirtied_nodes: self.signals.dirty_nodes().iter().cloned().collect(),
                })
            }
        }
    }

    fn run_local_action(
        &mut self,
        action: &EventAction,
        res: Option<&ExtValue>,
    ) -> Result<(), UiError> {
        match action {
            EventAction::Toast { message } => {
                let val = eval_ui_expr(message, &self.signals, res);
                let text = match val {
                    ExtValue::Str(s) => s,
                    other => format!("{other:?}"),
                };
                self.toasts.push(text.clone());
                self.events_log.push(UiLifecycleEvent::Toast(text));
                Ok(())
            }
            EventAction::Set { signal, value } => {
                let val = eval_ui_expr(value, &self.signals, res);
                let dirtied = self.signals.set(signal, ext_to_signal_value(&val))?;
                self.events_log.push(UiLifecycleEvent::SignalUpdated {
                    signal: signal.clone(),
                    dirtied,
                });
                Ok(())
            }
            EventAction::Generic { name, args } if name == "NAV" => {
                if let Some(Expr {
                    kind: ExprKind::NodeRef(target),
                    ..
                }) = args.first()
                {
                    self.active_route = Some(target.clone());
                    self.events_log
                        .push(UiLifecycleEvent::Navigated(target.clone()));
                }
                Ok(())
            }
            EventAction::Generic { .. } | EventAction::Call { .. } => Ok(()),
        }
    }

    fn find_api(&self, api_id: &NodeId) -> Result<&ApiNode, UiError> {
        match self.program.get(api_id) {
            Some(node) => match &node.kind {
                NodeKind::Api(api) => Ok(api),
                _ => Err(UiError::Bridge(format!("`{api_id}` is not an `#API` node"))),
            },
            None => Err(UiError::Bridge(format!("unknown `#API` node `{api_id}`"))),
        }
    }

    fn resolve_server_url(&self, srv_id: &NodeId) -> String {
        let mut port = 8080i64;
        if let Some(node) = self.program.get(srv_id) {
            if let NodeKind::Srv(srv) = &node.kind {
                for prop in &srv.props {
                    if prop.key == "port" {
                        if let PropValue::Int(p) = prop.value {
                            port = p;
                        }
                    }
                }
            }
        }
        format!("http://127.0.0.1:{port}")
    }
}

fn eval_ui_expr(expr: &Expr, signals: &SignalStore, res: Option<&ExtValue>) -> ExtValue {
    match &expr.kind {
        ExprKind::Int(n) => ExtValue::I32(*n as i32),
        ExprKind::Float(f) => ExtValue::F64(*f),
        ExprKind::Str(s) => ExtValue::Str(signals.resolve_text(s)),
        ExprKind::Bool(b) => ExtValue::Bool(*b),
        ExprKind::Null => ExtValue::Void,
        ExprKind::NodeRef(id) => signals
            .get(id)
            .map(signal_to_ext_value)
            .unwrap_or(ExtValue::Void),
        ExprKind::Ident(name) if name == "res" => res.cloned().unwrap_or(ExtValue::Void),
        ExprKind::Member { object, field } => {
            let obj = eval_ui_expr(object, signals, res);
            match obj {
                ExtValue::Map(map) => map.get(field).cloned().unwrap_or(ExtValue::Void),
                _ => ExtValue::Void,
            }
        }
        ExprKind::Object(entries) => {
            let mut map = BTreeMap::new();
            for (k, v) in entries {
                map.insert(k.clone(), eval_ui_expr(v, signals, res));
            }
            ExtValue::Map(map)
        }
        ExprKind::List(items) => {
            ExtValue::List(items.iter().map(|i| eval_ui_expr(i, signals, res)).collect())
        }
        _ => ExtValue::Void,
    }
}

fn signal_to_ext_value(val: &SignalValue) -> ExtValue {
    match val {
        SignalValue::I32(n) => ExtValue::I32(*n),
        SignalValue::F64(f) => ExtValue::F64(*f),
        SignalValue::Str(s) => ExtValue::Str(s.clone()),
        SignalValue::Bool(b) => ExtValue::Bool(*b),
        SignalValue::Null => ExtValue::Void,
    }
}

fn ext_to_signal_value(val: &ExtValue) -> SignalValue {
    match val {
        ExtValue::I32(n) => SignalValue::I32(*n),
        ExtValue::F64(f) => SignalValue::F64(*f),
        ExtValue::Str(s) => SignalValue::Str(s.clone()),
        ExtValue::Bool(b) => SignalValue::Bool(*b),
        ExtValue::Ts(t) => SignalValue::I32(*t as i32),
        _ => SignalValue::Null,
    }
}
