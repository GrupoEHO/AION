use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use aion_ast::{DiagnosticReport, NodeKind, Program, PropValue};
use aion_checker::check_program;
use aion_ext::ExtRegistry;
use aion_parser::parse_program;
use aion_runtime_net::{build_router, generate_proto3, RuntimeConfig};
use aion_runtime_ui::{compute_layout, SignalStore, Viewport};
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use tokio::sync::broadcast;
use tower::ServiceExt;

const WEB_SHELL_HTML: &str = r#"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>AION Web Runtime · Hot Reload</title>
  <style>
    :root { color-scheme: dark; }
    * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
    body { margin: 0; background: #0B1120; color: #E2E8F0; display: grid; grid-template-columns: 1fr 380px; min-height: 100vh; }
    @media (max-width: 960px) { body { grid-template-columns: 1fr; } }
    main { display: flex; flex-direction: column; align-items: center; padding: 20px; gap: 16px; }
    .toolbar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; background: #1E293B; padding: 10px 16px; border-radius: 10px; }
    .toolbar button { background: #334155; color: #F8FAFC; border: 0; padding: 6px 12px; border-radius: 6px; cursor: pointer; font-weight: 600; font-size: 13px; }
    .toolbar button.active { background: #2563EB; }
    .toolbar button.reload { background: #0D9488; }
    .toolbar button.restart { background: #D97706; }
    .badge { font-size: 12px; color: #94A3B8; }
    #stage-wrap { position: relative; border: 2px solid #334155; border-radius: 14px; overflow: auto; max-width: 100%; box-shadow: 0 20px 50px rgba(0,0,0,.45); }
    #stage { position: relative; overflow: hidden; }
    #error-overlay { display: none; position: absolute; inset: 0; background: rgba(15, 23, 42, .94); color: #FCA5A5; padding: 18px; overflow: auto; z-index: 20; font-family: ui-monospace, monospace; font-size: 12px; }
    .aion-node { position: absolute; display: flex; align-items: center; }
    .aion-btn { border: 0; cursor: pointer; justify-content: center; transition: opacity .15s; }
    .aion-btn:hover { opacity: .9; }
    .aion-inp { border: 1px solid #CBD5E1; padding: 0 12px; color: #0F172A; font-size: 14px; outline: none; }
    .aion-inp:focus { border-color: #2563EB; box-shadow: 0 0 0 2px rgba(37,99,235,.2); }
    .aion-txt { white-space: nowrap; pointer-events: none; }
    aside { background: #111827; border-left: 1px solid #1F2937; padding: 16px; overflow: auto; display: flex; flex-direction: column; gap: 14px; }
    h3 { margin: 0 0 6px; font-size: 13px; text-transform: uppercase; letter-spacing: .06em; color: #94A3B8; }
    pre { margin: 0; background: #0B1120; border: 1px solid #1F2937; border-radius: 8px; padding: 10px; font-size: 12px; overflow: auto; max-height: 220px; color: #CBD5E1; }
    #toast { position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%); background: #0F172A; color: #F8FAFC; border: 1px solid #334155; padding: 10px 18px; border-radius: 8px; display: none; font-size: 14px; z-index: 99; }
  </style>
</head>
<body>
  <main>
    <div class="toolbar">
      <strong style="font-size:14px;">AION Web</strong>
      <button id="btn-mobile" class="active" onclick="loadViewport('mobile', false)">Mobile 390×844</button>
      <button id="btn-desktop" onclick="loadViewport('desktop', false)">Desktop 1440×900</button>
      <button class="reload" onclick="triggerHotReload()" title="Preserva estado #S (tecla r)">⚡ Hot Reload</button>
      <button class="restart" onclick="triggerHotRestart()" title="Reinicia estado #S (tecla R)">🔄 Hot Restart</button>
      <span id="status-badge" class="badge">Cargando...</span>
    </div>
    <div id="stage-wrap">
      <div id="stage"></div>
      <pre id="error-overlay"></pre>
    </div>
  </main>
  <aside>
    <div>
      <h3>Signals (#S) en vivo</h3>
      <pre id="signals-view">{}</pre>
    </div>
    <div>
      <h3>Última respuesta #API</h3>
      <pre id="api-view">Sin llamadas aún</pre>
    </div>
    <div>
      <h3>Esquema .proto generado (#M)</h3>
      <pre id="proto-view"></pre>
    </div>
  </aside>
  <div id="toast"></div>

  <script>
    let bundle = null;
    let signals = {};
    let activeScreens = {};
    let currentViewport = 'mobile';

    function initRouters() {
      for (const n of bundle.program.nodes) {
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          const initial = ui.args?.[0]?.value || children[0];
          if (initial && !(n.id in activeScreens)) activeScreens[n.id] = initial;
        }
      }
    }

    function isNodeVisible(id, nodeMap, parentMap) {
      let cur = id;
      while (cur) {
        const p = parentMap[cur];
        if (!p) break;
        const pUi = nodeMap[p];
        if (pUi?.widget === 'ROUTER' && activeScreens[p] && activeScreens[p] !== cur) {
          return false;
        }
        cur = p;
      }
      return true;
    }

    function navigateTo(targetId) {
      for (const n of bundle.program.nodes) {
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          if (children.includes(targetId)) {
            activeScreens[n.id] = targetId;
            renderStage();
            return true;
          }
        }
      }
      return false;
    }

    function showToast(msg) {
      const el = document.getElementById('toast');
      el.textContent = msg;
      el.style.display = 'block';
      clearTimeout(window._toastTimer);
      window._toastTimer = setTimeout(() => el.style.display = 'none', 2600);
    }

    function showErrorOverlay(report) {
      const ov = document.getElementById('error-overlay');
      if (!report) {
        ov.style.display = 'none';
        ov.textContent = '';
      } else {
        ov.style.display = 'block';
        ov.textContent = 'AION Check Error (guarda un .aion válido para Hot Reload):\n\n' + JSON.stringify(report, null, 2);
      }
    }

    function resolveText(tpl) {
      let out = tpl;
      for (const [k, v] of Object.entries(signals)) {
        out = out.replaceAll('{' + k + '}', String(v ?? ''));
      }
      return out;
    }

    function evalExpr(expr, resObj) {
      if (!expr || !expr.kind) return null;
      const k = expr.kind.expr_kind;
      const v = expr.kind.value;
      if (k === 'int' || k === 'float' || k === 'str' || k === 'bool') return v;
      if (k === 'node_ref') return signals[v];
      if (k === 'ident') return v === 'res' ? resObj : null;
      if (k === 'member') {
        const obj = evalExpr(v.object, resObj);
        return obj && typeof obj === 'object' ? obj[v.field] : null;
      }
      if (k === 'object') {
        const obj = {};
        for (const [key, valExpr] of v) obj[key] = evalExpr(valExpr, resObj);
        return obj;
      }
      return null;
    }

    async function handleNodeClick(ui) {
      const tap = (ui.events || []).find(e => e.event === 'tap');
      if (!tap) return;
      const act = tap.action;
      if (act.action_kind === 'generic' && act.name === 'NAV') {
        const target = act.args?.[0]?.kind?.value;
        if (target) navigateTo(target);
        return;
      }
      if (act.action_kind === 'set') {
        signals[act.signal] = evalExpr(act.value, null);
        updateSignalsUI();
        return;
      }
      if (act.action_kind === 'call') {
        const apiNode = bundle.program.nodes.find(n => n.id === act.api)?.kind;
        if (!apiNode) return;
        const payload = evalExpr(act.payload, null) || {};
        try {
          const resp = await fetch(apiNode.path, {
            method: apiNode.method.toUpperCase(),
            headers: { 'Content-Type': 'application/json', 'Accept': 'application/json' },
            body: apiNode.method.toUpperCase() === 'GET' ? undefined : JSON.stringify(payload)
          });
          const data = await resp.json();
          document.getElementById('api-view').textContent = JSON.stringify(data, null, 2);
          if (!resp.ok) throw new Error(data.error || ('HTTP ' + resp.status));
          if (act.bind) {
            signals[act.bind.signal] = evalExpr(act.bind.value, data);
            updateSignalsUI();
          }
          showToast('Respuesta OK de ' + act.api);
        } catch (err) {
          const errHandler = (ui.events || []).find(e => e.event === 'err');
          if (errHandler && errHandler.action.action_kind === 'toast') {
            showToast(evalExpr(errHandler.action.message, null) || String(err));
          } else {
            showToast('Error: ' + err.message);
          }
        }
      }
    }

    function updateSignalsUI() {
      document.getElementById('signals-view').textContent = JSON.stringify(signals, null, 2);
      for (const el of document.querySelectorAll('[data-template]')) {
        el.textContent = resolveText(el.dataset.template);
      }
    }

    function renderStage() {
      const stage = document.getElementById('stage');
      const vp = bundle.layout.viewport;
      stage.style.width = vp.width + 'px';
      stage.style.height = vp.height + 'px';
      stage.innerHTML = '';

      const nodeMap = {};
      for (const n of bundle.program.nodes) nodeMap[n.id] = n.kind;
      const parentMap = {};
      for (const [id, b] of Object.entries(bundle.layout.boxes)) {
        if (b.parent) parentMap[id] = b.parent;
      }
      initRouters();

      for (const id of bundle.layout.paint_order) {
        const box = bundle.layout.boxes[id];
        const ui = nodeMap[id];
        if (!box || !ui || ui.node_type !== 'ui') continue;
        if (!isNodeVisible(id, nodeMap, parentMap)) continue;

        let el;
        if (ui.widget === 'BTN') {
          el = document.createElement('button');
          el.className = 'aion-node aion-btn';
          const radius = ui.args[2]?.value ?? 6;
          const bg = ui.args[3]?.value ?? '#2563EB';
          el.style.borderRadius = radius + 'px';
          el.style.background = bg;
          el.onclick = () => handleNodeClick(ui);
        } else if (ui.widget === 'INP') {
          el = document.createElement('input');
          el.className = 'aion-node aion-inp';
          el.type = 'text';
          const radius = ui.args[2]?.value ?? 6;
          const bg = ui.args[3]?.value ?? '#FFFFFF';
          const boundSig = ui.args[4]?.value;
          el.style.borderRadius = radius + 'px';
          el.style.background = bg;
          if (ui.body?.body_kind === 'text') el.placeholder = ui.body.value.template;
          if (boundSig) {
            el.value = signals[boundSig] ?? '';
            el.addEventListener('input', e => {
              signals[boundSig] = e.target.value;
              updateSignalsUI();
            });
          }
        } else if (ui.widget === 'TXT') {
          el = document.createElement('span');
          el.className = 'aion-node aion-txt';
          el.style.fontSize = (ui.args[0]?.value ?? 16) + 'px';
          el.style.fontWeight = String(ui.args[1]?.value ?? 400);
          el.style.color = ui.args[2]?.value ?? '#0F172A';
          if (ui.body?.body_kind === 'text') {
            el.dataset.template = ui.body.value.template;
            el.textContent = resolveText(ui.body.value.template);
          }
        } else {
          el = document.createElement('div');
          el.className = 'aion-node';
          if (ui.widget === 'ROOT' && ui.args[4]?.value) el.style.background = ui.args[4].value;
          if (ui.widget === 'NAV' && ui.args[2]?.value) el.style.background = ui.args[2].value;
          if (ui.widget === 'LIST') el.style.overflow = 'auto';
        }

        el.style.left = box.x + 'px';
        el.style.top = box.y + 'px';
        el.style.width = box.width + 'px';
        el.style.height = box.height + 'px';
        stage.appendChild(el);
      }
      updateSignalsUI();
    }

    async function loadViewport(vp, resetSignals) {
      currentViewport = vp;
      document.getElementById('btn-mobile').classList.toggle('active', vp === 'mobile');
      document.getElementById('btn-desktop').classList.toggle('active', vp === 'desktop');
      const resp = await fetch('/__aion/bundle.json?viewport=' + vp);
      bundle = await resp.json();
      if (resetSignals || Object.keys(signals).length === 0) {
        signals = { ...bundle.signals };
      } else {
        // Hot Reload: preserve existing #S values and only add newly declared signals.
        for (const [k, v] of Object.entries(bundle.signals)) {
          if (!(k in signals)) signals[k] = v;
        }
      }
      document.getElementById('proto-view').textContent = bundle.proto;
      document.getElementById('status-badge').textContent =
        'rev #' + bundle.revision + ' · ' +
        (bundle.overflows.length === 0 ? 'Layout OK' : (bundle.overflows.length + ' overflow(s)'));
      showErrorOverlay(bundle.last_error);
      renderStage();
    }

    async function triggerHotReload() {
      await fetch('/__aion/reload', { method: 'POST' });
    }

    async function triggerHotRestart() {
      await fetch('/__aion/restart', { method: 'POST' });
    }

    function connectHotReloadStream() {
      const es = new EventSource('/__aion/events');
      es.onmessage = async (ev) => {
        try {
          const msg = JSON.parse(ev.data);
          if (msg.type === 'hot_reload') {
            await loadViewport(currentViewport, false);
            showToast('⚡ Hot Reload (rev #' + msg.revision + ') — estado #S preservado');
          } else if (msg.type === 'hot_restart') {
            await loadViewport(currentViewport, true);
            showToast('🔄 Hot Restart (rev #' + msg.revision + ') — estado #S reiniciado');
          } else if (msg.type === 'check_error') {
            showErrorOverlay(msg.report);
            showToast('Error de compilación detectado');
          }
        } catch (_) {}
      };
    }

    window.addEventListener('keydown', (e) => {
      if (document.activeElement?.tagName === 'INPUT') return;
      if (e.key === 'r') triggerHotReload();
      if (e.key === 'R') triggerHotRestart();
    });

    loadViewport('mobile', true);
    connectHotReloadStream();
  </script>
</body>
</html>
"#;

#[derive(Clone)]
pub struct LiveApp {
    pub revision: u64,
    pub program: Program,
    pub proto: String,
    pub api_router: Router,
    pub last_error: Option<DiagnosticReport>,
}

#[derive(Clone)]
pub struct WebState {
    pub file_path: Option<PathBuf>,
    pub registry: Arc<ExtRegistry>,
    pub config: RuntimeConfig,
    pub live: Arc<RwLock<LiveApp>>,
    pub events_tx: broadcast::Sender<String>,
}

impl WebState {
    pub fn new(
        file_path: Option<PathBuf>,
        program: Program,
        registry: Arc<ExtRegistry>,
        config: RuntimeConfig,
    ) -> Result<Self, String> {
        let api_router = build_router(program.clone(), registry.clone(), config.clone())
            .map_err(|e| e.to_string())?;
        let proto = generate_proto3(&program);
        let (events_tx, _) = broadcast::channel(64);
        Ok(Self {
            file_path,
            registry,
            config,
            live: Arc::new(RwLock::new(LiveApp {
                revision: 1,
                program,
                proto,
                api_router,
                last_error: None,
            })),
            events_tx,
        })
    }

    /// Re-reads and validates `file_path` from disk.
    /// When `restart` is `false`, emits `hot_reload` (preserving `#S` signals).
    /// When `restart` is `true`, emits `hot_restart` (resetting `#S` signals).
    pub fn reload_from_disk(&self, restart: bool) -> Result<u64, DiagnosticReport> {
        let Some(ref path) = self.file_path else {
            let rev = self.live.read().unwrap().revision;
            let ev = if restart { "hot_restart" } else { "hot_reload" };
            let _ = self
                .events_tx
                .send(json!({ "type": ev, "revision": rev }).to_string());
            return Ok(rev);
        };

        let src = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                let diag = aion_ast::Diagnostic::error(
                    "E_IO",
                    aion_ast::Phase::Parser,
                    format!("failed to read {}: {e}", path.display()),
                    aion_ast::Span::default(),
                );
                let report = DiagnosticReport::from_diagnostics(vec![diag]);
                self.live.write().unwrap().last_error = Some(report.clone());
                let _ = self
                    .events_tx
                    .send(json!({ "type": "check_error", "report": report }).to_string());
                return Err(report);
            }
        };

        let prog = match parse_program(&src) {
            Ok(p) => p,
            Err(diags) => {
                let report = DiagnosticReport::from_diagnostics(diags);
                self.live.write().unwrap().last_error = Some(report.clone());
                let _ = self
                    .events_tx
                    .send(json!({ "type": "check_error", "report": report }).to_string());
                return Err(report);
            }
        };

        let check = check_program(&prog, &self.registry);
        if !check.ok {
            self.live.write().unwrap().last_error = Some(check.clone());
            let _ = self
                .events_tx
                .send(json!({ "type": "check_error", "report": check }).to_string());
            return Err(check);
        }

        let api_router = match build_router(
            prog.clone(),
            self.registry.clone(),
            self.config.clone(),
        ) {
            Ok(r) => r,
            Err(e) => {
                let diag = aion_ast::Diagnostic::error(
                    "E_RUNTIME",
                    aion_ast::Phase::Checker,
                    e.to_string(),
                    aion_ast::Span::default(),
                );
                let report = DiagnosticReport::from_diagnostics(vec![diag]);
                self.live.write().unwrap().last_error = Some(report.clone());
                let _ = self
                    .events_tx
                    .send(json!({ "type": "check_error", "report": report }).to_string());
                return Err(report);
            }
        };

        let proto = generate_proto3(&prog);
        let new_rev = {
            let mut guard = self.live.write().unwrap();
            guard.revision += 1;
            guard.program = prog;
            guard.proto = proto;
            guard.api_router = api_router;
            guard.last_error = None;
            guard.revision
        };

        let ev_type = if restart { "hot_restart" } else { "hot_reload" };
        let _ = self
            .events_tx
            .send(json!({ "type": ev_type, "revision": new_rev }).to_string());
        Ok(new_rev)
    }
}

pub fn build_web_router(state: WebState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/__aion/bundle.json", get(serve_bundle))
        .route("/__aion/events", get(serve_sse_events))
        .route("/__aion/reload", post(handle_hot_reload))
        .route("/__aion/restart", post(handle_hot_restart))
        .fallback(forward_to_live_api)
        .with_state(state)
}

pub async fn serve_web(
    file_path: Option<PathBuf>,
    program: Program,
    registry: Arc<ExtRegistry>,
    config: RuntimeConfig,
) -> Result<(), String> {
    let port = extract_port(&program).unwrap_or(8080);
    let state = WebState::new(file_path.clone(), program, registry, config)?;

    // Spawn live file watcher for automatic Hot Reload on save.
    if let Some(watch_path) = file_path {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            let mut last_mtime: Option<SystemTime> = fs::metadata(&watch_path)
                .and_then(|m| m.modified())
                .ok();
            let mut ticker = tokio::time::interval(Duration::from_millis(300));
            loop {
                ticker.tick().await;
                let cur_mtime = fs::metadata(&watch_path)
                    .and_then(|m| m.modified())
                    .ok();
                if cur_mtime != last_mtime && cur_mtime.is_some() {
                    last_mtime = cur_mtime;
                    match watcher_state.reload_from_disk(false) {
                        Ok(rev) => eprintln!("⚡ AION Hot Reload -> rev #{rev}"),
                        Err(report) => eprintln!(
                            "AION Hot Reload error ({} issue(s))",
                            report.error_count
                        ),
                    }
                }
            }
        });
    }

    let router = build_web_router(state);
    let addr = format!("127.0.0.1:{port}");
    eprintln!("AION Web Preview (Hot Reload enabled) running at http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("failed to bind {addr}: {e}"))?;
    axum::serve(listener, router)
        .await
        .map_err(|e| format!("server error: {e}"))
}

fn extract_port(program: &Program) -> Option<u16> {
    for node in &program.nodes {
        if let NodeKind::Srv(srv) = &node.kind {
            for p in &srv.props {
                if p.key == "port" {
                    if let PropValue::Int(port) = p.value {
                        return u16::try_from(port).ok();
                    }
                }
            }
        }
    }
    None
}

async fn serve_index() -> Html<&'static str> {
    Html(WEB_SHELL_HTML)
}

async fn handle_hot_reload(State(state): State<WebState>) -> Response {
    match state.reload_from_disk(false) {
        Ok(rev) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": true, "type": "hot_reload", "revision": rev }).to_string(),
        )
            .into_response(),
        Err(report) => (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            report.to_json_pretty().unwrap_or_default(),
        )
            .into_response(),
    }
}

async fn handle_hot_restart(State(state): State<WebState>) -> Response {
    match state.reload_from_disk(true) {
        Ok(rev) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": true, "type": "hot_restart", "revision": rev }).to_string(),
        )
            .into_response(),
        Err(report) => (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "application/json")],
            report.to_json_pretty().unwrap_or_default(),
        )
            .into_response(),
    }
}

async fn serve_sse_events(State(state): State<WebState>) -> Response {
    let mut rx = state.events_tx.subscribe();
    let (tx, body_rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(16);
    tokio::spawn(async move {
        let _ = tx.send(Ok(": connected\n\n".to_string())).await;
        while let Ok(msg) = rx.recv().await {
            if tx.send(Ok(format!("data: {msg}\n\n"))).await.is_err() {
                break;
            }
        }
    });

    let stream = ReceiverStream(body_rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

struct ReceiverStream(tokio::sync::mpsc::Receiver<Result<String, std::io::Error>>);

impl futures_core::Stream for ReceiverStream {
    type Item = Result<String, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}

async fn forward_to_live_api(State(state): State<WebState>, req: Request) -> Response {
    let router = state.live.read().unwrap().api_router.clone();
    match router.oneshot(req).await {
        Ok(resp) => resp,
        Err(infallible) => match infallible {},
    }
}

async fn serve_bundle(
    State(state): State<WebState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let viewport = match query.get("viewport").map(String::as_str) {
        Some("desktop") => Viewport::DESKTOP,
        _ => Viewport::MOBILE,
    };

    let (revision, program, proto, last_error) = {
        let guard = state.live.read().unwrap();
        (
            guard.revision,
            guard.program.clone(),
            guard.proto.clone(),
            guard.last_error.clone(),
        )
    };

    let signal_store = SignalStore::from_program(&program);
    let mut initial_signals = serde_json::Map::new();
    for node in &program.nodes {
        if let NodeKind::Signal(_) = &node.kind {
            if let Some(val) = signal_store.get(&node.id) {
                initial_signals.insert(
                    node.id.as_str().to_string(),
                    serde_json::to_value(val).unwrap_or(serde_json::Value::Null),
                );
            }
        }
    }

    match compute_layout(&program, viewport, Some(&signal_store)) {
        Ok(layout) => {
            let overflows = layout.detect_overflows();
            let payload = json!({
                "revision": revision,
                "program": program,
                "signals": initial_signals,
                "layout": layout,
                "overflows": overflows,
                "proto": proto,
                "last_error": last_error,
            });
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::to_string(&payload).unwrap(),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "application/json")],
            json!({ "ok": false, "error": err.to_string() }).to_string(),
        )
            .into_response(),
    }
}
