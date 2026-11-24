use std::collections::HashMap;
use std::sync::Arc;

use aion_ast::{NodeKind, Program, PropValue};
use aion_ext::ExtRegistry;
use aion_runtime_net::{build_router, generate_proto3, RuntimeConfig};
use aion_runtime_ui::{compute_layout, SignalStore, Viewport};
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;

const WEB_SHELL_HTML: &str = r#"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>AION Web Runtime Preview</title>
  <style>
    :root { color-scheme: dark; }
    * { box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
    body { margin: 0; background: #0B1120; color: #E2E8F0; display: grid; grid-template-columns: 1fr 380px; min-height: 100vh; }
    @media (max-width: 960px) { body { grid-template-columns: 1fr; } }
    main { display: flex; flex-direction: column; align-items: center; padding: 20px; gap: 16px; }
    .toolbar { display: flex; gap: 10px; align-items: center; background: #1E293B; padding: 10px 16px; border-radius: 10px; }
    .toolbar button { background: #334155; color: #F8FAFC; border: 0; padding: 6px 12px; border-radius: 6px; cursor: pointer; font-weight: 600; }
    .toolbar button.active { background: #2563EB; }
    .badge { font-size: 12px; color: #94A3B8; }
    #stage-wrap { border: 2px solid #334155; border-radius: 14px; overflow: auto; max-width: 100%; box-shadow: 0 20px 50px rgba(0,0,0,.45); }
    #stage { position: relative; overflow: hidden; }
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
      <strong style="font-size:14px;">AION Web Preview</strong>
      <button id="btn-mobile" class="active" onclick="loadViewport('mobile')">Mobile 390×844</button>
      <button id="btn-desktop" onclick="loadViewport('desktop')">Desktop 1440×900</button>
      <span id="status-badge" class="badge">Cargando...</span>
    </div>
    <div id="stage-wrap">
      <div id="stage"></div>
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
    let currentViewport = 'mobile';

    function showToast(msg) {
      const el = document.getElementById('toast');
      el.textContent = msg;
      el.style.display = 'block';
      clearTimeout(window._toastTimer);
      window._toastTimer = setTimeout(() => el.style.display = 'none', 3000);
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

    async function handleNodeClick(node) {
      const tap = (node.events || []).find(e => e.event === 'tap');
      if (!tap) return;
      const act = tap.action;
      if (act.action_kind === 'call') {
        const apiNode = bundle.program.nodes.find(n => n.id === act.api);
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
          const errHandler = (node.events || []).find(e => e.event === 'err');
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
      for (const n of bundle.program.nodes) nodeMap[n.id] = n;

      for (const id of bundle.layout.paint_order) {
        const box = bundle.layout.boxes[id];
        const node = nodeMap[id];
        if (!box || !node || node.node_type !== 'ui') continue;

        let el;
        if (node.widget === 'BTN') {
          el = document.createElement('button');
          el.className = 'aion-node aion-btn';
          const radius = node.args[2]?.value ?? 6;
          const bg = node.args[3]?.value ?? '#2563EB';
          el.style.borderRadius = radius + 'px';
          el.style.background = bg;
          el.onclick = () => handleNodeClick(node);
        } else if (node.widget === 'INP') {
          el = document.createElement('input');
          el.className = 'aion-node aion-inp';
          el.type = 'text';
          const radius = node.args[2]?.value ?? 6;
          const bg = node.args[3]?.value ?? '#FFFFFF';
          const boundSig = node.args[4]?.value;
          el.style.borderRadius = radius + 'px';
          el.style.background = bg;
          if (node.body?.body_kind === 'text') el.placeholder = node.body.value.template;
          if (boundSig) {
            el.value = signals[boundSig] ?? '';
            el.addEventListener('input', e => {
              signals[boundSig] = e.target.value;
              updateSignalsUI();
            });
          }
        } else if (node.widget === 'TXT') {
          el = document.createElement('span');
          el.className = 'aion-node aion-txt';
          el.style.fontSize = (node.args[0]?.value ?? 16) + 'px';
          el.style.fontWeight = String(node.args[1]?.value ?? 400);
          el.style.color = node.args[2]?.value ?? '#0F172A';
          if (node.body?.body_kind === 'text') {
            el.dataset.template = node.body.value.template;
            el.textContent = resolveText(node.body.value.template);
          }
        } else {
          el = document.createElement('div');
          el.className = 'aion-node';
          if (node.widget === 'ROOT' && node.args[4]?.value) el.style.background = node.args[4].value;
          if (node.widget === 'NAV' && node.args[2]?.value) el.style.background = node.args[2].value;
          if (node.widget === 'LIST') el.style.overflow = 'auto';
        }

        el.style.left = box.x + 'px';
        el.style.top = box.y + 'px';
        el.style.width = box.width + 'px';
        el.style.height = box.height + 'px';
        stage.appendChild(el);
      }
      updateSignalsUI();
    }

    async function loadViewport(vp) {
      currentViewport = vp;
      document.getElementById('btn-mobile').classList.toggle('active', vp === 'mobile');
      document.getElementById('btn-desktop').classList.toggle('active', vp === 'desktop');
      const resp = await fetch('/__aion/bundle.json?viewport=' + vp);
      bundle = await resp.json();
      if (Object.keys(signals).length === 0) signals = { ...bundle.signals };
      document.getElementById('proto-view').textContent = bundle.proto;
      document.getElementById('status-badge').textContent =
        bundle.overflows.length === 0 ? 'Layout OK (0 overflows)' : (bundle.overflows.length + ' overflow(s)');
      renderStage();
    }

    loadViewport('mobile');
  </script>
</body>
</html>
"#;

#[derive(Clone)]
struct WebState {
    program: Arc<Program>,
    proto: Arc<String>,
}

pub fn build_web_router(
    program: Program,
    registry: Arc<ExtRegistry>,
    config: RuntimeConfig,
) -> Result<Router, String> {
    let api_router =
        build_router(program.clone(), registry, config).map_err(|e| e.to_string())?;
    let proto = Arc::new(generate_proto3(&program));
    let web_state = WebState {
        program: Arc::new(program),
        proto,
    };

    let web_router = Router::new()
        .route("/", get(serve_index))
        .route("/__aion/bundle.json", get(serve_bundle))
        .with_state(web_state);

    Ok(api_router.merge(web_router))
}

pub async fn serve_web(
    program: Program,
    registry: Arc<ExtRegistry>,
    config: RuntimeConfig,
) -> Result<(), String> {
    let port = extract_port(&program).unwrap_or(8080);
    let router = build_web_router(program, registry, config)?;
    let addr = format!("127.0.0.1:{port}");
    eprintln!("AION Web Preview running at http://{addr}");
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

async fn serve_bundle(
    State(state): State<WebState>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let viewport = match query.get("viewport").map(String::as_str) {
        Some("desktop") => Viewport::DESKTOP,
        _ => Viewport::MOBILE,
    };
    let signal_store = SignalStore::from_program(&state.program);
    let mut initial_signals = serde_json::Map::new();
    for node in &state.program.nodes {
        if let NodeKind::Signal(_) = &node.kind {
            if let Some(val) = signal_store.get(&node.id) {
                initial_signals.insert(
                    node.id.as_str().to_string(),
                    serde_json::to_value(val).unwrap_or(serde_json::Value::Null),
                );
            }
        }
    }

    match compute_layout(&state.program, viewport, Some(&signal_store)) {
        Ok(layout) => {
            let overflows = layout.detect_overflows();
            let payload = json!({
                "program": &*state.program,
                "signals": initial_signals,
                "layout": layout,
                "overflows": overflows,
                "proto": &*state.proto,
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
