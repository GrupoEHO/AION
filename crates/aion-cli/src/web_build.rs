use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aion_ast::{NodeKind, Program, PropValue};
use aion_runtime_net::generate_proto3;
use aion_runtime_ui::{compute_layout, SignalStore, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Metadata read automatically from `aion.toml` when running `aion build` inside a project folder.
#[derive(Debug, Clone, Default)]
pub struct ProjectMeta {
    pub name: Option<String>,
    pub version: Option<String>,
    pub entry: Option<String>,
    pub app_id: Option<String>,
}

impl ProjectMeta {
    pub fn load_from_dir(dir: &Path) -> Self {
        let toml_path = dir.join("aion.toml");
        let Ok(raw) = fs::read_to_string(&toml_path) else {
            return Self::default();
        };
        let mut meta = Self::default();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('[') {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
                match key {
                    "name" => meta.name = Some(val),
                    "version" => meta.version = Some(val),
                    "entry" => meta.entry = Some(val),
                    "app_id" => meta.app_id = Some(val),
                    _ => {}
                }
            }
        }
        meta
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebBuildOptions {
    pub app_name: String,
    pub api_base_url: Option<String>,
    pub out_dir: PathBuf,
    pub release: bool,
    pub wasm: bool,
}

impl Default for WebBuildOptions {
    fn default() -> Self {
        Self {
            app_name: "AION App".to_string(),
            api_base_url: None,
            out_dir: PathBuf::from("build/web"),
            release: true,
            wasm: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WebBuildResult {
    pub ok: bool,
    pub app_name: String,
    pub mode: String,
    pub wasm: bool,
    pub out_dir: String,
    pub index_html_path: String,
    pub bundle_json_path: String,
    pub proto_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_size_bytes: Option<u64>,
    pub total_size_bytes: u64,
}

/// Compiles a validated AION [`Program`] into a standalone static web bundle
/// (`index.html`, `aion_bundle.json`, `schema.proto`) ready for Firebase Hosting,
/// Cloudflare Pages, Netlify, or Nginx.
pub fn build_web_bundle(
    program: &Program,
    opts: &WebBuildOptions,
) -> Result<WebBuildResult, String> {
    fs::create_dir_all(&opts.out_dir)
        .map_err(|e| format!("failed to create {}: {e}", opts.out_dir.display()))?;

    let signal_store = SignalStore::from_program(program);
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

    let mobile_layout = compute_layout(program, Viewport::MOBILE, Some(&signal_store))
        .map_err(|e| format!("mobile layout error: {e}"))?;
    let desktop_layout = compute_layout(program, Viewport::DESKTOP, Some(&signal_store))
        .map_err(|e| format!("desktop layout error: {e}"))?;
    let proto = generate_proto3(program);

    let default_port = extract_port(program).unwrap_or(8080);
    let api_base_url = opts
        .api_base_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{default_port}"));

    let bundle_payload = json!({
        "appName": opts.app_name,
        "mode": if opts.release { "release" } else { "debug" },
        "apiBaseUrl": api_base_url,
        "program": program,
        "signals": initial_signals,
        "layouts": {
            "mobile": mobile_layout,
            "desktop": desktop_layout,
        },
        "proto": proto,
    });

    let bundle_json = if opts.release {
        serde_json::to_string(&bundle_payload).unwrap()
    } else {
        serde_json::to_string_pretty(&bundle_payload).unwrap()
    };

    let bundle_json_path = opts.out_dir.join("aion_bundle.json");
    let proto_path = opts.out_dir.join("schema.proto");
    let index_html_path = opts.out_dir.join("index.html");

    fs::write(&bundle_json_path, &bundle_json)
        .map_err(|e| format!("write aion_bundle.json: {e}"))?;
    fs::write(&proto_path, &proto).map_err(|e| format!("write schema.proto: {e}"))?;

    let (html, wasm_path, wasm_size_bytes) = if opts.wasm {
        let (wp, wsize) = compile_wasm_runtime(&opts.out_dir, opts.release)?;
        (
            generate_wasm_index_html(&opts.app_name),
            Some(wp.display().to_string()),
            Some(wsize),
        )
    } else {
        (generate_prod_index_html(&opts.app_name), None, None)
    };

    fs::write(&index_html_path, &html).map_err(|e| format!("write index.html: {e}"))?;

    let total_size_bytes = bundle_json.len() as u64
        + proto.len() as u64
        + html.len() as u64
        + wasm_size_bytes.unwrap_or(0);

    Ok(WebBuildResult {
        ok: true,
        app_name: opts.app_name.clone(),
        mode: if opts.release { "release" } else { "debug" }.to_string(),
        wasm: opts.wasm,
        out_dir: opts.out_dir.display().to_string(),
        index_html_path: index_html_path.display().to_string(),
        bundle_json_path: bundle_json_path.display().to_string(),
        proto_path: proto_path.display().to_string(),
        wasm_path,
        wasm_size_bytes,
        total_size_bytes,
    })
}

fn compile_wasm_runtime(out_dir: &Path, release: bool) -> Result<(PathBuf, u64), String> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .ok_or_else(|| "unable to locate AION workspace root".to_string())?;
    let manifest_path = workspace_root.join("Cargo.toml");

    let cargo_bin = detect_rustup_cargo();
    let mut cmd = Command::new(&cargo_bin);
    if let Some(bin_dir) = cargo_bin.parent() {
        let rustc_bin = bin_dir.join(if cfg!(windows) { "rustc.exe" } else { "rustc" });
        if rustc_bin.is_file() {
            cmd.env("RUSTC", &rustc_bin);
        }
    }
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("-p")
        .arg("aion-web-wasm")
        .arg("--target")
        .arg("wasm32-unknown-unknown");
    if release {
        cmd.arg("--release");
    }

    let build_out = cmd
        .output()
        .map_err(|e| format!("failed to run cargo ({}): {e}", cargo_bin.display()))?;
    if !build_out.status.success() {
        let stderr = String::from_utf8_lossy(&build_out.stderr);
        return Err(format!(
            "cargo wasm32 build failed (ensure `rustup target add wasm32-unknown-unknown` is installed):\n{stderr}"
        ));
    }

    let profile = if release { "release" } else { "debug" };
    let raw_wasm = workspace_root
        .join("target/wasm32-unknown-unknown")
        .join(profile)
        .join("aion_web_wasm.wasm");
    if !raw_wasm.is_file() {
        return Err(format!(
            "expected compiled WASM module not found at {}",
            raw_wasm.display()
        ));
    }

    let pkg_dir = out_dir.join("pkg");
    fs::create_dir_all(&pkg_dir)
        .map_err(|e| format!("failed to create {}: {e}", pkg_dir.display()))?;

    let bindgen_bin = detect_wasm_bindgen();
    let bg_out = Command::new(&bindgen_bin)
        .arg("--target")
        .arg("web")
        .arg("--no-typescript")
        .arg("--out-dir")
        .arg(&pkg_dir)
        .arg(&raw_wasm)
        .output()
        .map_err(|e| {
            format!(
                "failed to run wasm-bindgen (`{}`): {e}. Install with `brew install wasm-bindgen` or `cargo install wasm-bindgen-cli`.",
                bindgen_bin.display()
            )
        })?;
    if !bg_out.status.success() {
        let stderr = String::from_utf8_lossy(&bg_out.stderr);
        return Err(format!("wasm-bindgen failed:\n{stderr}"));
    }

    let final_wasm = pkg_dir.join("aion_web_wasm_bg.wasm");
    let size = fs::metadata(&final_wasm)
        .map(|m| m.len())
        .map_err(|e| format!("missing {}: {e}", final_wasm.display()))?;
    Ok((final_wasm, size))
}

fn detect_rustup_cargo() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(&home).join(".cargo/bin/cargo");
        if p.is_file() {
            return p;
        }
    }
    let brew_rustup = PathBuf::from("/opt/homebrew/opt/rustup/bin/cargo");
    if brew_rustup.is_file() {
        return brew_rustup;
    }
    PathBuf::from("cargo")
}

fn detect_wasm_bindgen() -> PathBuf {
    for candidate in ["/opt/homebrew/bin/wasm-bindgen", "/usr/local/bin/wasm-bindgen"] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return p;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin/wasm-bindgen");
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("wasm-bindgen")
}

fn generate_wasm_index_html(app_name: &str) -> String {
    let safe_title = app_name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r##"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{safe_title}</title>
  <style>
    * {{ box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
    html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; background: #F8FAFC; overflow: auto; }}
    #stage {{ position: relative; margin: 0 auto; overflow: hidden; }}
    .aion-node {{ position: absolute; display: flex; align-items: center; }}
    .aion-btn {{ border: 0; cursor: pointer; justify-content: center; transition: opacity .15s; }}
    .aion-btn:hover {{ opacity: .9; }}
    .aion-inp {{ border: 1px solid #CBD5E1; padding: 0 12px; color: #0F172A; font-size: 14px; outline: none; }}
    .aion-inp:focus {{ border-color: #2563EB; box-shadow: 0 0 0 2px rgba(37,99,235,.2); }}
    .aion-sel {{ border: 1px solid #CBD5E1; padding: 0 10px; color: #0F172A; font-size: 14px; outline: none; cursor: pointer; }}
    .aion-txt {{ white-space: nowrap; pointer-events: none; }}
    #toast {{ position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%); background: #0F172A; color: #F8FAFC; padding: 10px 18px; border-radius: 8px; display: none; font-size: 14px; z-index: 99; box-shadow: 0 10px 25px rgba(0,0,0,.25); }}
  </style>
</head>
<body>
  <div id="stage"></div>
  <div id="toast"></div>
  <script type="module">
    import init, {{ AionWasmApp }} from './pkg/aion_web_wasm.js';

    let bundle = null;
    let wasmApp = null;
    let signalTypes = {{}};
    let persistedSet = new Set();
    let activeScreens = {{}};
    const STORAGE_PREFIX = 'aion_state:' + (location.pathname || 'root') + ':';

    function currentSignals() {{
      return JSON.parse(wasmApp.signals_json());
    }}

    function initPersisted() {{
      for (const n of bundle.program.nodes) {{
        if (n.kind?.node_type === 'signal') {{
          signalTypes[n.id] = n.kind.ty;
          persistedSet.add(n.id);
          try {{
            const raw = localStorage.getItem(STORAGE_PREFIX + n.id);
            if (raw !== null) wasmApp.set_signal(n.id, raw);
          }} catch (_) {{}}
        }}
      }}
    }}

    function savePersisted() {{
      const sigs = currentSignals();
      for (const id of persistedSet) {{
        try {{ localStorage.setItem(STORAGE_PREFIX + id, JSON.stringify(sigs[id])); }} catch (_) {{}}
      }}
    }}

    function coerceVal(sigId, rawVal) {{
      const ty = signalTypes[sigId];
      if (ty === 'I32') {{ const n = parseInt(rawVal, 10); return Number.isNaN(n) ? 0 : n; }}
      if (ty === 'F64') {{ const n = parseFloat(rawVal); return Number.isNaN(n) ? 0 : n; }}
      if (ty === 'Bool') return Boolean(rawVal);
      return rawVal;
    }}

    function extractSelectOptions(ui, signals) {{
      let raw = [];
      if (ui.body?.body_kind === 'options') raw = ui.body.value || [];
      else if (ui.body?.body_kind === 'text' && ui.body.value?.template) {{
        raw = ui.body.value.template.split(',').map(s => s.trim()).filter(Boolean);
      }} else if (ui.args?.[5]?.arg_kind === 'node_ref') {{
        const arr = signals[ui.args[5].value];
        if (Array.isArray(arr)) raw = arr.map(x => String(x));
      }}
      return raw.map(entry => {{
        const s = String(entry);
        const bar = s.indexOf('|');
        return bar >= 0 ? {{ value: s.slice(0, bar), label: s.slice(bar + 1) }} : {{ value: s, label: s }};
      }});
    }}

    function getDynamicListSig(ui) {{
      if (!ui || ui.widget !== 'LIST') return null;
      const ref = (ui.args || []).find(a => a.arg_kind === 'node_ref');
      return ref ? ref.value : null;
    }}

    function isInsideDynamicListTemplate(id, nodeMap, parentMap) {{
      let cur = id;
      while (cur) {{
        const p = parentMap[cur];
        if (!p) break;
        if (getDynamicListSig(nodeMap[p])) return true;
        cur = p;
      }}
      return false;
    }}

    function initRouters() {{
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {{
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          const initial = ui.args?.[0]?.value || children[0];
          if (initial && !(n.id in activeScreens)) activeScreens[n.id] = initial;
        }}
      }}
    }}

    function isNodeVisible(id, nodeMap, parentMap) {{
      if (isInsideDynamicListTemplate(id, nodeMap, parentMap)) return false;
      let mc = id;
      while (mc) {{
        const mu = nodeMap[mc];
        if (mu?.widget === 'MODAL' && !truthy(sigVal(mu.args?.[0]?.value))) return false;
        mc = parentMap[mc];
      }}
      let cur = id;
      while (cur) {{
        const p = parentMap[cur];
        if (!p) break;
        const pUi = nodeMap[p];
        if (pUi?.widget === 'ROUTER' && activeScreens[p] && activeScreens[p] !== cur) return false;
        cur = p;
      }}
      return true;
    }}

    function navigateTo(targetId) {{
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {{
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          if (children.includes(targetId)) {{
            activeScreens[n.id] = targetId;
            renderStage();
            const screenUi = bundle.program.nodes.find(x => x.id === targetId)?.kind;
            if (screenUi) runNodeEvent(screenUi, 'init', null);
            return true;
          }}
        }}
      }}
      return false;
    }}

    function truthy(v) {{ return !(v === false || v === null || v === undefined || v === 0 || v === '' || v === 'false'); }}
    function clampPct(v) {{ const n = Number(v) || 0; return Math.max(0, Math.min(100, n)); }}
    const sigVal = (id) => (typeof wasmApp !== 'undefined' && wasmApp) ? currentSignals()[id] : signals[id];

    function aionBeep() {{
      try {{
        const C = window.AudioContext || window.webkitAudioContext;
        const ctx = new C();
        [0, 0.25, 0.5].forEach(t => {{
          const o = ctx.createOscillator();
          const g = ctx.createGain();
          o.type = 'square';
          o.frequency.value = 880;
          g.gain.value = 0.15;
          o.connect(g); g.connect(ctx.destination);
          o.start(ctx.currentTime + t);
          o.stop(ctx.currentTime + t + 0.16);
        }});
      }} catch (_) {{}}
      try {{ if (navigator.vibrate) navigator.vibrate([200, 100, 200]); }} catch (_) {{}}
    }}

    function aionNotify(title, body) {{
      const fallback = () => showToast(title + (body ? ': ' + body : ''));
      try {{
        if (!('Notification' in window)) return fallback();
        if (Notification.permission === 'granted') {{ new Notification(title, {{ body }}); }}
        else if (Notification.permission !== 'denied') {{
          Notification.requestPermission().then(p => {{ if (p === 'granted') new Notification(title, {{ body }}); else fallback(); }});
        }} else fallback();
      }} catch (_) {{ fallback(); }}
    }}

    function startTicks() {{
      (window.__aionTimers || []).forEach(clearInterval);
      window.__aionTimers = [];
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type !== 'ui') continue;
        const evs = new Set((ui.events || []).map(e => e.event).filter(e => /^tick\d*$/.test(e)));
        for (const evName of evs) {{
          const ms = evName === 'tick' ? 1000 : Math.max(50, parseInt(evName.slice(4), 10) || 1000);
          window.__aionTimers.push(setInterval(async () => {{
            window.__aionTicking = true;
            try {{ await runNodeEvent(ui, evName, null); }} finally {{ window.__aionTicking = false; }}
          }}, ms));
        }}
      }}
    }}

    function showToast(msg) {{
      const el = document.getElementById('toast');
      el.textContent = msg;
      el.style.display = 'block';
      clearTimeout(window._toastTimer);
      window._toastTimer = setTimeout(() => el.style.display = 'none', 3000);
    }}

    function resolveText(tpl, signals, itemCtx) {{
      let out = wasmApp.resolve_text(tpl);
      if (itemCtx && itemCtx.item !== undefined) {{
        if (itemCtx.item && typeof itemCtx.item === 'object') {{
          for (const [fk, fv] of Object.entries(itemCtx.item)) {{
            out = out.replaceAll('{{item.' + fk + '}}', String(fv ?? ''));
          }}
        }} else {{
          out = out.replaceAll('{{item}}', String(itemCtx.item ?? ''));
        }}
      }}
      return out;
    }}

    function evalExpr(expr, resObj, signals, itemCtx) {{
      if (!expr || !expr.kind) return null;
      const k = expr.kind.expr_kind;
      const v = expr.kind.value;
      if (k === 'int' || k === 'float' || k === 'str' || k === 'bool') return v;
      if (k === 'node_ref') return signals[v];
      if (k === 'ident') {{
        if (v === 'res') return resObj;
        if (v === 'item') return itemCtx?.item;
        if (v === 'index') return itemCtx?.index;
        return null;
      }}
      if (k === 'member') {{
        const obj = evalExpr(v.object, resObj, signals, itemCtx);
        return obj && typeof obj === 'object' ? obj[v.field] : null;
      }}
      if (k === 'object') {{
        const obj = {{}};
        for (const [key, valExpr] of v) obj[key] = evalExpr(valExpr, resObj, signals, itemCtx);
        return obj;
      }}
      if (k === 'list') return (v || []).map(x => evalExpr(x, resObj, signals, itemCtx));
      if (k === 'call') {{
        const cn = v.callee?.kind?.value;
        if (cn === 'UID') return 'id' + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
        if (cn === 'NOW') return Date.now();
        return null;
      }}
      if (k === 'binary') {{
        const l = evalExpr(v.left, resObj, signals, itemCtx);
        const r = evalExpr(v.right, resObj, signals, itemCtx);
        switch (v.op) {{
          case 'Add': return (typeof l === 'string' || typeof r === 'string') ? (String(l ?? '') + String(r ?? '')) : ((Number(l) || 0) + (Number(r) || 0));
          case 'Sub': return (Number(l) || 0) - (Number(r) || 0);
          case 'Mul': return (Number(l) || 0) * (Number(r) || 0);
          case 'Div': return Number(r) ? (Number(l) / Number(r)) : 0;
          case 'Eq': return l === r;
          case 'Ne': return l !== r;
          case 'Lt': return Number(l) < Number(r);
          case 'Le': return Number(l) <= Number(r);
          case 'Gt': return Number(l) > Number(r);
          case 'Ge': return Number(l) >= Number(r);
          case 'And': return Boolean(l) && Boolean(r);
          case 'Or': return Boolean(l) || Boolean(r);
        }}
      }}
      return null;
    }}

    function refreshTexts() {{
      const sigs = currentSignals();
      for (const el of document.querySelectorAll('[data-template]')) {{
        el.textContent = resolveText(el.dataset.template, sigs, null);
      }}
    }}

    async function runNodeEvent(ui, evName, itemCtx) {{
      const handlers = (ui.events || []).filter(e => e.event === evName);
      if (!handlers.length) return;
      let needsRender = false;

      for (const handler of handlers) {{
        const signals = currentSignals();
        if (handler.guard && !truthy(evalExpr(handler.guard, null, signals, itemCtx))) continue;
        const act = handler.action;
        if (act.action_kind === 'generic') {{
          if (act.name === 'NAV') {{
            const target = act.args?.[0]?.kind?.value;
            if (target) {{
              savePersisted();
              navigateTo(target);
              needsRender = false;
            }}
            continue;
          }}
          if (act.name === 'BEEP') {{ aionBeep(); continue; }}
          if (act.name === 'NOTIFY') {{
            aionNotify(String(evalExpr(act.args?.[0], null, signals, itemCtx) ?? ''), String(evalExpr(act.args?.[1], null, signals, itemCtx) ?? ''));
            continue;
          }}
          if (act.name === 'SETAT') {{
            const sigId = act.args?.[0]?.kind?.value;
            const idx = Number(evalExpr(act.args?.[1], null, signals, itemCtx));
            const val = evalExpr(act.args?.[2], null, signals, itemCtx);
            if (sigId && Array.isArray(signals[sigId]) && idx >= 0 && idx < signals[sigId].length) {{
              const cur = [...signals[sigId]];
              cur[idx] = val;
              wasmApp.set_signal(sigId, JSON.stringify(cur));
              needsRender = true;
            }}
            continue;
          }}
          if (act.name === 'PUSH') {{
            const sigId = act.args?.[0]?.kind?.value;
            const elem = evalExpr(act.args?.[1], null, signals, itemCtx);
            if (sigId) {{
              const cur = Array.isArray(signals[sigId]) ? [...signals[sigId]] : [];
              cur.push(elem);
              wasmApp.set_signal(sigId, JSON.stringify(cur));
              needsRender = true;
            }}
            continue;
          }}
          if (act.name === 'REMOVE') {{
            const sigId = act.args?.[0]?.kind?.value;
            const target = evalExpr(act.args?.[1], null, signals, itemCtx);
            if (sigId && Array.isArray(signals[sigId])) {{
              const cur = [...signals[sigId]];
              if (typeof target === 'number') cur.splice(target, 1);
              else {{ const idx = cur.indexOf(target); if (idx >= 0) cur.splice(idx, 1); }}
              wasmApp.set_signal(sigId, JSON.stringify(cur));
              needsRender = true;
            }}
            continue;
          }}
        }}
        if (act.action_kind === 'set') {{
          const nextVal = evalExpr(act.value, null, signals, itemCtx);
          wasmApp.set_signal(act.signal, JSON.stringify(nextVal));
          needsRender = true;
          continue;
        }}
        if (act.action_kind === 'toast') {{
          showToast(String(evalExpr(act.message, null, signals, itemCtx) ?? ''));
          continue;
        }}
        if (act.action_kind === 'call') {{
          const apiNode = bundle.program.nodes.find(n => n.id === act.api)?.kind;
          if (!apiNode) continue;
          const payload = evalExpr(act.payload, null, signals, itemCtx) || {{}};
          const base = (bundle.apiBaseUrl || '').replace(/\/+$/, '');
          try {{
            const resp = await fetch(base + apiNode.path, {{
              method: apiNode.method.toUpperCase(),
              headers: {{ 'Content-Type': 'application/json', 'Accept': 'application/json' }},
              body: apiNode.method.toUpperCase() === 'GET' ? undefined : JSON.stringify(payload)
            }});
            const data = await resp.json();
            if (!resp.ok) throw new Error(data.error || ('HTTP ' + resp.status));
            if (act.bind) {{
              const nextVal = evalExpr(act.bind.value, data, currentSignals(), itemCtx);
              wasmApp.set_signal(act.bind.signal, JSON.stringify(nextVal));
              needsRender = true;
            }}
          }} catch (err) {{
            const errHandler = (ui.events || []).find(e => e.event === 'err');
            if (errHandler && errHandler.action.action_kind === 'toast') {{
              showToast(evalExpr(errHandler.action.message, null, signals, itemCtx) || String(err));
            }} else {{
              showToast('Error: ' + err.message);
            }}
          }}
        }}
      }}

      if (needsRender) {{
        savePersisted();
        renderStage();
      }}
    }}

    async function handleNodeClick(ui) {{ return runNodeEvent(ui, 'tap', null); }}

    function renderDynamicNode(tplId, nodeMap, signals, itemCtx) {{
      const ui = nodeMap[tplId];
      if (!ui || ui.node_type !== 'ui') return null;
      if (ui.widget === 'TXT') {{
        const span = document.createElement('span');
        span.style.fontSize = (ui.args[0]?.value ?? 14) + 'px';
        span.style.fontWeight = String(ui.args[1]?.value ?? 400);
        span.style.color = ui.args[2]?.value ?? '#0F172A';
        if (ui.body?.body_kind === 'text') span.textContent = resolveText(ui.body.value.template, signals, itemCtx);
        return span;
      }}
      if (ui.widget === 'BTN') {{
        const btn = document.createElement('button');
        btn.className = 'aion-btn';
        btn.style.display = 'inline-flex';
        btn.style.alignItems = 'center';
        btn.style.width = (ui.args[0]?.value ?? 90) + 'px';
        btn.style.height = (ui.args[1]?.value ?? 32) + 'px';
        btn.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
        btn.style.background = ui.args[3]?.value ?? '#2563EB';
        btn.onclick = () => runNodeEvent(ui, 'tap', itemCtx);
        for (const ch of (ui.body?.body_kind === 'children' ? ui.body.value : [])) {{
          const c = renderDynamicNode(ch, nodeMap, signals, itemCtx);
          if (c) btn.appendChild(c);
        }}
        return btn;
      }}
      const div = document.createElement('div');
      div.style.display = 'flex';
      div.style.flexDirection = ui.args[0]?.value === 'h' ? 'row' : 'column';
      const j = ui.args[1]?.value;
      div.style.justifyContent = j === 'sb' ? 'space-between' : j === 'c' ? 'center' : 'flex-start';
      div.style.alignItems = 'center';
      div.style.gap = (ui.args[3]?.value ?? 8) + 'px';
      div.style.padding = '8px 10px';
      div.style.background = '#FFFFFF';
      div.style.borderRadius = '8px';
      div.style.border = '1px solid #E2E8F0';
      for (const ch of (ui.body?.body_kind === 'children' ? ui.body.value : [])) {{
        const c = renderDynamicNode(ch, nodeMap, signals, itemCtx);
        if (c) div.appendChild(c);
      }}
      return div;
    }}

    function renderStage() {{
      if (window.__aionTicking && ['INPUT', 'SELECT'].includes(document.activeElement?.tagName)) {{
        if (typeof updateSignalsUI === 'function') updateSignalsUI();
        else if (typeof refreshTexts === 'function') refreshTexts();
        return;
      }}
      const stage = document.getElementById('stage');
      const w = Math.max(320, window.innerWidth);
      const h = Math.max(480, window.innerHeight);
      const layout = JSON.parse(wasmApp.compute_layout_json(w, h));
      const signals = currentSignals();

      stage.style.width = layout.viewport.width + 'px';
      stage.style.height = layout.viewport.height + 'px';
      stage.innerHTML = '';

      const nodeMap = {{}};
      for (const n of bundle.program.nodes) nodeMap[n.id] = n.kind;
      const parentMap = {{}};
      for (const [id, b] of Object.entries(layout.boxes)) {{
        if (b.parent) parentMap[id] = b.parent;
      }}
      initRouters();

      for (const id of layout.paint_order) {{
        const box = layout.boxes[id];
        const ui = nodeMap[id];
        if (!box || !ui || ui.node_type !== 'ui') continue;
        if (!isNodeVisible(id, nodeMap, parentMap)) continue;

        let el;
        if (ui.widget === 'BTN') {{
          el = document.createElement('button');
          el.className = 'aion-node aion-btn';
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#2563EB';
          el.onclick = () => handleNodeClick(ui);
        }} else if (ui.widget === 'INP') {{
          el = document.createElement('input');
          el.className = 'aion-node aion-inp';
          el.type = 'text';
          const boundSig = ui.args[4]?.value;
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#FFFFFF';
          if (ui.body?.body_kind === 'text') el.placeholder = ui.body.value.template;
          if (boundSig) {{
            el.value = signals[boundSig] ?? '';
            el.addEventListener('input', e => {{
              wasmApp.set_signal(boundSig, JSON.stringify(coerceVal(boundSig, e.target.value)));
              savePersisted();
              refreshTexts();
            }});
          }}
        }} else if (ui.widget === 'SEL' || ui.widget === 'SELECT') {{
          el = document.createElement('select');
          el.className = 'aion-node aion-sel';
          const boundSig = ui.args[4]?.value;
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#FFFFFF';
          const opts = extractSelectOptions(ui, signals);
          const curVal = boundSig ? String(signals[boundSig] ?? '') : '';
          for (const o of opts) {{
            const optEl = document.createElement('option');
            optEl.value = o.value;
            optEl.textContent = resolveText(o.label, signals, null);
            if (o.value === curVal) optEl.selected = true;
            el.appendChild(optEl);
          }}
          el.addEventListener('change', async e => {{
            if (boundSig) {{
              wasmApp.set_signal(boundSig, JSON.stringify(coerceVal(boundSig, e.target.value)));
              savePersisted();
            }}
            await runNodeEvent(ui, 'change', null);
            await runNodeEvent(ui, 'tap', null);
            renderStage();
          }});
        }} else if (ui.widget === 'PROG') {{
          el = document.createElement('div');
          el.className = 'aion-node';
          el.style.pointerEvents = 'none';
          el.style.overflow = 'hidden';
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#E2E8F0';
          const fill = document.createElement('div');
          fill.style.height = '100%';
          fill.style.width = clampPct(sigVal(ui.args[5]?.value)) + '%';
          fill.style.background = ui.args[4]?.value ?? '#2563EB';
          fill.style.transition = 'width .4s linear';
          el.appendChild(fill);
        }} else if (ui.widget === 'RING') {{
          el = document.createElement('div');
          el.className = 'aion-node';
          el.style.pointerEvents = 'none';
          el.style.borderRadius = '50%';
          const thick = ui.args[1]?.value ?? 10;
          const track = ui.args[2]?.value ?? '#E2E8F0';
          const fillC = ui.args[3]?.value ?? '#2563EB';
          const pct = clampPct(sigVal(ui.args[4]?.value));
          el.style.background = 'conic-gradient(' + fillC + ' ' + (pct * 3.6) + 'deg, ' + track + ' 0)';
          const mask = 'radial-gradient(farthest-side, transparent calc(100% - ' + thick + 'px), #000 calc(100% - ' + thick + 'px))';
          el.style.webkitMask = mask;
          el.style.mask = mask;
        }} else if (ui.widget === 'TXT') {{
          el = document.createElement('span');
          el.className = 'aion-node aion-txt';
          el.style.fontSize = (ui.args[0]?.value ?? 16) + 'px';
          el.style.fontWeight = String(ui.args[1]?.value ?? 400);
          el.style.color = ui.args[2]?.value ?? '#0F172A';
          if (ui.body?.body_kind === 'text') {{
            el.dataset.template = ui.body.value.template;
            el.textContent = resolveText(ui.body.value.template, signals, null);
          }}
        }} else {{
          el = document.createElement('div');
          el.className = 'aion-node';
          const isScrollFlex = ui.widget === 'FLEX' && (ui.args || []).some(a => a.value === 'scroll');
          el.style.pointerEvents = (ui.widget === 'LIST' || ui.widget === 'MODAL' || isScrollFlex) ? 'auto' : 'none';
          if (ui.widget === 'MODAL') el.style.background = 'rgba(15,23,42,.55)';
          if (isScrollFlex) el.style.overflow = 'auto';
          if (ui.widget === 'ROOT' && ui.args[4]?.value) el.style.background = ui.args[4].value;
          if (ui.widget === 'NAV' && ui.args[2]?.value) el.style.background = ui.args[2].value;
          if (ui.widget === 'LIST') {{
            el.style.overflow = 'auto';
            el.style.display = 'flex';
            el.style.flexDirection = ui.args[0]?.value === 'h' ? 'row' : 'column';
            el.style.alignItems = 'stretch';
            el.style.gap = (ui.args[1]?.value ?? 8) + 'px';
            const listSig = getDynamicListSig(ui);
            if (listSig) {{
              const items = Array.isArray(signals[listSig]) ? signals[listSig] : [];
              const tplId = ui.body?.body_kind === 'children' ? ui.body.value[0] : null;
              if (tplId) {{
                items.forEach((item, index) => {{
                  const rowEl = renderDynamicNode(tplId, nodeMap, signals, {{ item, index }});
                  if (rowEl) el.appendChild(rowEl);
                }});
              }}
            }}
          }}
        }}

        el.style.left = box.x + 'px';
        el.style.top = box.y + 'px';
        el.style.width = box.width + 'px';
        el.style.height = box.height + 'px';
        stage.appendChild(el);
      }}
    }}

    (async () => {{
      await init();
      const resp = await fetch('./aion_bundle.json');
      const raw = await resp.text();
      bundle = JSON.parse(raw);
      wasmApp = new AionWasmApp(raw);
      initPersisted();
      renderStage();
      for (const n of bundle.program.nodes) {{
        if (n.kind?.node_type === 'ui') runNodeEvent(n.kind, 'init', null);
      }}
      startTicks();
      window.addEventListener('resize', renderStage);
    }})();
  </script>
</body>
</html>
"##
    )
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

fn generate_prod_index_html(app_name: &str) -> String {
    let safe_title = app_name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r##"<!doctype html>
<html lang="es">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{safe_title}</title>
  <style>
    * {{ box-sizing: border-box; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }}
    html, body {{ margin: 0; padding: 0; width: 100%; height: 100%; background: #F8FAFC; overflow: auto; }}
    #stage {{ position: relative; margin: 0 auto; overflow: hidden; }}
    .aion-node {{ position: absolute; display: flex; align-items: center; }}
    .aion-btn {{ border: 0; cursor: pointer; justify-content: center; transition: opacity .15s; }}
    .aion-btn:hover {{ opacity: .9; }}
    .aion-inp {{ border: 1px solid #CBD5E1; padding: 0 12px; color: #0F172A; font-size: 14px; outline: none; }}
    .aion-inp:focus {{ border-color: #2563EB; box-shadow: 0 0 0 2px rgba(37,99,235,.2); }}
    .aion-sel {{ border: 1px solid #CBD5E1; padding: 0 10px; color: #0F172A; font-size: 14px; outline: none; cursor: pointer; }}
    .aion-txt {{ white-space: nowrap; pointer-events: none; }}
    #toast {{ position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%); background: #0F172A; color: #F8FAFC; padding: 10px 18px; border-radius: 8px; display: none; font-size: 14px; z-index: 99; box-shadow: 0 10px 25px rgba(0,0,0,.25); }}
  </style>
</head>
<body>
  <div id="stage"></div>
  <div id="toast"></div>
  <script>
    let bundle = null;
    let signals = {{}};
    let signalTypes = {{}};
    let persistedSet = new Set();
    let activeScreens = {{}};
    const STORAGE_PREFIX = 'aion_state:' + (location.pathname || 'root') + ':';

    function initPersisted() {{
      for (const n of bundle.program.nodes) {{
        if (n.kind?.node_type === 'signal') {{
          signalTypes[n.id] = n.kind.ty;
          persistedSet.add(n.id);
          try {{
            const raw = localStorage.getItem(STORAGE_PREFIX + n.id);
            if (raw !== null) signals[n.id] = JSON.parse(raw);
          }} catch (_) {{}}
        }}
      }}
    }}

    function savePersisted() {{
      for (const id of persistedSet) {{
        try {{ localStorage.setItem(STORAGE_PREFIX + id, JSON.stringify(signals[id])); }} catch (_) {{}}
      }}
    }}

    function coerceVal(sigId, rawVal) {{
      const ty = signalTypes[sigId];
      if (ty === 'I32') {{ const n = parseInt(rawVal, 10); return Number.isNaN(n) ? 0 : n; }}
      if (ty === 'F64') {{ const n = parseFloat(rawVal); return Number.isNaN(n) ? 0 : n; }}
      if (ty === 'Bool') return Boolean(rawVal);
      return rawVal;
    }}

    function extractSelectOptions(ui) {{
      let raw = [];
      if (ui.body?.body_kind === 'options') raw = ui.body.value || [];
      else if (ui.body?.body_kind === 'text' && ui.body.value?.template) {{
        raw = ui.body.value.template.split(',').map(s => s.trim()).filter(Boolean);
      }} else if (ui.args?.[5]?.arg_kind === 'node_ref') {{
        const arr = signals[ui.args[5].value];
        if (Array.isArray(arr)) raw = arr.map(x => String(x));
      }}
      return raw.map(entry => {{
        const s = String(entry);
        const bar = s.indexOf('|');
        return bar >= 0 ? {{ value: s.slice(0, bar), label: s.slice(bar + 1) }} : {{ value: s, label: s }};
      }});
    }}

    function getDynamicListSig(ui) {{
      if (!ui || ui.widget !== 'LIST') return null;
      const ref = (ui.args || []).find(a => a.arg_kind === 'node_ref');
      return ref ? ref.value : null;
    }}

    function isInsideDynamicListTemplate(id, nodeMap, parentMap) {{
      let cur = id;
      while (cur) {{
        const p = parentMap[cur];
        if (!p) break;
        if (getDynamicListSig(nodeMap[p])) return true;
        cur = p;
      }}
      return false;
    }}

    function initRouters() {{
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {{
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          const initial = ui.args?.[0]?.value || children[0];
          if (initial && !(n.id in activeScreens)) activeScreens[n.id] = initial;
        }}
      }}
    }}

    function isNodeVisible(id, nodeMap, parentMap) {{
      if (isInsideDynamicListTemplate(id, nodeMap, parentMap)) return false;
      let mc = id;
      while (mc) {{
        const mu = nodeMap[mc];
        if (mu?.widget === 'MODAL' && !truthy(sigVal(mu.args?.[0]?.value))) return false;
        mc = parentMap[mc];
      }}
      let cur = id;
      while (cur) {{
        const p = parentMap[cur];
        if (!p) break;
        const pUi = nodeMap[p];
        if (pUi?.widget === 'ROUTER' && activeScreens[p] && activeScreens[p] !== cur) return false;
        cur = p;
      }}
      return true;
    }}

    function navigateTo(targetId) {{
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type === 'ui' && ui.widget === 'ROUTER') {{
          const children = ui.body?.body_kind === 'children' ? ui.body.value : [];
          if (children.includes(targetId)) {{
            activeScreens[n.id] = targetId;
            renderStage();
            const screenUi = bundle.program.nodes.find(x => x.id === targetId)?.kind;
            if (screenUi) runNodeEvent(screenUi, 'init', null);
            return true;
          }}
        }}
      }}
      return false;
    }}

    function truthy(v) {{ return !(v === false || v === null || v === undefined || v === 0 || v === '' || v === 'false'); }}
    function clampPct(v) {{ const n = Number(v) || 0; return Math.max(0, Math.min(100, n)); }}
    const sigVal = (id) => (typeof wasmApp !== 'undefined' && wasmApp) ? currentSignals()[id] : signals[id];

    function aionBeep() {{
      try {{
        const C = window.AudioContext || window.webkitAudioContext;
        const ctx = new C();
        [0, 0.25, 0.5].forEach(t => {{
          const o = ctx.createOscillator();
          const g = ctx.createGain();
          o.type = 'square';
          o.frequency.value = 880;
          g.gain.value = 0.15;
          o.connect(g); g.connect(ctx.destination);
          o.start(ctx.currentTime + t);
          o.stop(ctx.currentTime + t + 0.16);
        }});
      }} catch (_) {{}}
      try {{ if (navigator.vibrate) navigator.vibrate([200, 100, 200]); }} catch (_) {{}}
    }}

    function aionNotify(title, body) {{
      const fallback = () => showToast(title + (body ? ': ' + body : ''));
      try {{
        if (!('Notification' in window)) return fallback();
        if (Notification.permission === 'granted') {{ new Notification(title, {{ body }}); }}
        else if (Notification.permission !== 'denied') {{
          Notification.requestPermission().then(p => {{ if (p === 'granted') new Notification(title, {{ body }}); else fallback(); }});
        }} else fallback();
      }} catch (_) {{ fallback(); }}
    }}

    function startTicks() {{
      (window.__aionTimers || []).forEach(clearInterval);
      window.__aionTimers = [];
      for (const n of bundle.program.nodes) {{
        const ui = n.kind;
        if (ui?.node_type !== 'ui') continue;
        const evs = new Set((ui.events || []).map(e => e.event).filter(e => /^tick\d*$/.test(e)));
        for (const evName of evs) {{
          const ms = evName === 'tick' ? 1000 : Math.max(50, parseInt(evName.slice(4), 10) || 1000);
          window.__aionTimers.push(setInterval(async () => {{
            window.__aionTicking = true;
            try {{ await runNodeEvent(ui, evName, null); }} finally {{ window.__aionTicking = false; }}
          }}, ms));
        }}
      }}
    }}

    function showToast(msg) {{
      const el = document.getElementById('toast');
      el.textContent = msg;
      el.style.display = 'block';
      clearTimeout(window._toastTimer);
      window._toastTimer = setTimeout(() => el.style.display = 'none', 3000);
    }}

    function resolveText(tpl, itemCtx) {{
      let out = tpl;
      for (const [k, v] of Object.entries(signals)) {{
        out = out.replaceAll('{{' + k + '}}', typeof v === 'object' && v !== null ? JSON.stringify(v) : String(v ?? ''));
      }}
      if (itemCtx && itemCtx.item !== undefined) {{
        if (itemCtx.item && typeof itemCtx.item === 'object') {{
          for (const [fk, fv] of Object.entries(itemCtx.item)) {{
            out = out.replaceAll('{{item.' + fk + '}}', String(fv ?? ''));
          }}
        }} else {{
          out = out.replaceAll('{{item}}', String(itemCtx.item ?? ''));
        }}
      }}
      return out;
    }}

    function evalExpr(expr, resObj, itemCtx) {{
      if (!expr || !expr.kind) return null;
      const k = expr.kind.expr_kind;
      const v = expr.kind.value;
      if (k === 'int' || k === 'float' || k === 'str' || k === 'bool') return v;
      if (k === 'node_ref') return signals[v];
      if (k === 'ident') {{
        if (v === 'res') return resObj;
        if (v === 'item') return itemCtx?.item;
        if (v === 'index') return itemCtx?.index;
        return null;
      }}
      if (k === 'member') {{
        const obj = evalExpr(v.object, resObj, itemCtx);
        return obj && typeof obj === 'object' ? obj[v.field] : null;
      }}
      if (k === 'object') {{
        const obj = {{}};
        for (const [key, valExpr] of v) obj[key] = evalExpr(valExpr, resObj, itemCtx);
        return obj;
      }}
      if (k === 'list') return (v || []).map(x => evalExpr(x, resObj, itemCtx));
      if (k === 'call') {{
        const cn = v.callee?.kind?.value;
        if (cn === 'UID') return 'id' + Date.now().toString(36) + Math.random().toString(36).slice(2, 8);
        if (cn === 'NOW') return Date.now();
        return null;
      }}
      if (k === 'binary') {{
        const l = evalExpr(v.left, resObj, itemCtx);
        const r = evalExpr(v.right, resObj, itemCtx);
        switch (v.op) {{
          case 'Add': return (typeof l === 'string' || typeof r === 'string') ? (String(l ?? '') + String(r ?? '')) : ((Number(l) || 0) + (Number(r) || 0));
          case 'Sub': return (Number(l) || 0) - (Number(r) || 0);
          case 'Mul': return (Number(l) || 0) * (Number(r) || 0);
          case 'Div': return Number(r) ? (Number(l) / Number(r)) : 0;
          case 'Eq': return l === r;
          case 'Ne': return l !== r;
          case 'Lt': return Number(l) < Number(r);
          case 'Le': return Number(l) <= Number(r);
          case 'Gt': return Number(l) > Number(r);
          case 'Ge': return Number(l) >= Number(r);
          case 'And': return Boolean(l) && Boolean(r);
          case 'Or': return Boolean(l) || Boolean(r);
        }}
      }}
      return null;
    }}

    function updateSignalsUI() {{
      for (const el of document.querySelectorAll('[data-template]')) {{
        el.textContent = resolveText(el.dataset.template, null);
      }}
    }}

    async function runNodeEvent(ui, evName, itemCtx) {{
      const handlers = (ui.events || []).filter(e => e.event === evName);
      if (!handlers.length) return;
      let needsRender = false;

      for (const handler of handlers) {{
        if (handler.guard && !truthy(evalExpr(handler.guard, null, itemCtx))) continue;
        const act = handler.action;
        if (act.action_kind === 'generic') {{
          if (act.name === 'NAV') {{
            const target = act.args?.[0]?.kind?.value;
            if (target) {{
              savePersisted();
              navigateTo(target);
              needsRender = false;
            }}
            continue;
          }}
          if (act.name === 'BEEP') {{ aionBeep(); continue; }}
          if (act.name === 'NOTIFY') {{
            aionNotify(String(evalExpr(act.args?.[0], null, itemCtx) ?? ''), String(evalExpr(act.args?.[1], null, itemCtx) ?? ''));
            continue;
          }}
          if (act.name === 'SETAT') {{
            const sigId = act.args?.[0]?.kind?.value;
            const idx = Number(evalExpr(act.args?.[1], null, itemCtx));
            const val = evalExpr(act.args?.[2], null, itemCtx);
            if (sigId && Array.isArray(signals[sigId]) && idx >= 0 && idx < signals[sigId].length) {{
              const cur = [...signals[sigId]];
              cur[idx] = val;
              signals[sigId] = cur;
              needsRender = true;
            }}
            continue;
          }}
          if (act.name === 'PUSH') {{
            const sigId = act.args?.[0]?.kind?.value;
            const elem = evalExpr(act.args?.[1], null, itemCtx);
            if (sigId) {{
              const cur = Array.isArray(signals[sigId]) ? [...signals[sigId]] : [];
              cur.push(elem);
              signals[sigId] = cur;
              needsRender = true;
            }}
            continue;
          }}
          if (act.name === 'REMOVE') {{
            const sigId = act.args?.[0]?.kind?.value;
            const target = evalExpr(act.args?.[1], null, itemCtx);
            if (sigId && Array.isArray(signals[sigId])) {{
              const cur = [...signals[sigId]];
              if (typeof target === 'number') cur.splice(target, 1);
              else {{ const idx = cur.indexOf(target); if (idx >= 0) cur.splice(idx, 1); }}
              signals[sigId] = cur;
              needsRender = true;
            }}
            continue;
          }}
        }}
        if (act.action_kind === 'set') {{
          signals[act.signal] = evalExpr(act.value, null, itemCtx);
          needsRender = true;
          continue;
        }}
        if (act.action_kind === 'toast') {{
          showToast(String(evalExpr(act.message, null, itemCtx) ?? ''));
          continue;
        }}
        if (act.action_kind === 'call') {{
          const apiNode = bundle.program.nodes.find(n => n.id === act.api)?.kind;
          if (!apiNode) continue;
          const payload = evalExpr(act.payload, null, itemCtx) || {{}};
          const base = (bundle.apiBaseUrl || '').replace(/\/+$/, '');
          try {{
            const resp = await fetch(base + apiNode.path, {{
              method: apiNode.method.toUpperCase(),
              headers: {{ 'Content-Type': 'application/json', 'Accept': 'application/json' }},
              body: apiNode.method.toUpperCase() === 'GET' ? undefined : JSON.stringify(payload)
            }});
            const data = await resp.json();
            if (!resp.ok) throw new Error(data.error || ('HTTP ' + resp.status));
            if (act.bind) {{
              signals[act.bind.signal] = evalExpr(act.bind.value, data, itemCtx);
              needsRender = true;
            }}
          }} catch (err) {{
            const errHandler = (ui.events || []).find(e => e.event === 'err');
            if (errHandler && errHandler.action.action_kind === 'toast') {{
              showToast(evalExpr(errHandler.action.message, null, itemCtx) || String(err));
            }} else {{
              showToast('Error: ' + err.message);
            }}
          }}
        }}
      }}

      if (needsRender) {{
        savePersisted();
        renderStage();
      }}
    }}

    async function handleNodeClick(ui) {{ return runNodeEvent(ui, 'tap', null); }}

    function renderDynamicNode(tplId, nodeMap, itemCtx) {{
      const ui = nodeMap[tplId];
      if (!ui || ui.node_type !== 'ui') return null;
      if (ui.widget === 'TXT') {{
        const span = document.createElement('span');
        span.style.fontSize = (ui.args[0]?.value ?? 14) + 'px';
        span.style.fontWeight = String(ui.args[1]?.value ?? 400);
        span.style.color = ui.args[2]?.value ?? '#0F172A';
        if (ui.body?.body_kind === 'text') span.textContent = resolveText(ui.body.value.template, itemCtx);
        return span;
      }}
      if (ui.widget === 'BTN') {{
        const btn = document.createElement('button');
        btn.className = 'aion-btn';
        btn.style.display = 'inline-flex';
        btn.style.alignItems = 'center';
        btn.style.width = (ui.args[0]?.value ?? 90) + 'px';
        btn.style.height = (ui.args[1]?.value ?? 32) + 'px';
        btn.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
        btn.style.background = ui.args[3]?.value ?? '#2563EB';
        btn.onclick = () => runNodeEvent(ui, 'tap', itemCtx);
        for (const ch of (ui.body?.body_kind === 'children' ? ui.body.value : [])) {{
          const c = renderDynamicNode(ch, nodeMap, itemCtx);
          if (c) btn.appendChild(c);
        }}
        return btn;
      }}
      const div = document.createElement('div');
      div.style.display = 'flex';
      div.style.flexDirection = ui.args[0]?.value === 'h' ? 'row' : 'column';
      const j = ui.args[1]?.value;
      div.style.justifyContent = j === 'sb' ? 'space-between' : j === 'c' ? 'center' : 'flex-start';
      div.style.alignItems = 'center';
      div.style.gap = (ui.args[3]?.value ?? 8) + 'px';
      div.style.padding = '8px 10px';
      div.style.background = '#FFFFFF';
      div.style.borderRadius = '8px';
      div.style.border = '1px solid #E2E8F0';
      for (const ch of (ui.body?.body_kind === 'children' ? ui.body.value : [])) {{
        const c = renderDynamicNode(ch, nodeMap, itemCtx);
        if (c) div.appendChild(c);
      }}
      return div;
    }}

    function renderStage() {{
      if (window.__aionTicking && ['INPUT', 'SELECT'].includes(document.activeElement?.tagName)) {{
        if (typeof updateSignalsUI === 'function') updateSignalsUI();
        else if (typeof refreshTexts === 'function') refreshTexts();
        return;
      }}
      const stage = document.getElementById('stage');
      const isDesktop = window.innerWidth >= 900;
      const layout = isDesktop ? bundle.layouts.desktop : bundle.layouts.mobile;
      const vp = layout.viewport;
      stage.style.width = vp.width + 'px';
      stage.style.height = vp.height + 'px';
      stage.innerHTML = '';

      const nodeMap = {{}};
      for (const n of bundle.program.nodes) nodeMap[n.id] = n.kind;
      const parentMap = {{}};
      for (const [id, b] of Object.entries(layout.boxes)) {{
        if (b.parent) parentMap[id] = b.parent;
      }}
      initRouters();

      for (const id of layout.paint_order) {{
        const box = layout.boxes[id];
        const ui = nodeMap[id];
        if (!box || !ui || ui.node_type !== 'ui') continue;
        if (!isNodeVisible(id, nodeMap, parentMap)) continue;

        let el;
        if (ui.widget === 'BTN') {{
          el = document.createElement('button');
          el.className = 'aion-node aion-btn';
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#2563EB';
          el.onclick = () => handleNodeClick(ui);
        }} else if (ui.widget === 'INP') {{
          el = document.createElement('input');
          el.className = 'aion-node aion-inp';
          el.type = 'text';
          const boundSig = ui.args[4]?.value;
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#FFFFFF';
          if (ui.body?.body_kind === 'text') el.placeholder = ui.body.value.template;
          if (boundSig) {{
            el.value = signals[boundSig] ?? '';
            el.addEventListener('input', e => {{
              signals[boundSig] = coerceVal(boundSig, e.target.value);
              savePersisted();
              updateSignalsUI();
            }});
          }}
        }} else if (ui.widget === 'SEL' || ui.widget === 'SELECT') {{
          el = document.createElement('select');
          el.className = 'aion-node aion-sel';
          const boundSig = ui.args[4]?.value;
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#FFFFFF';
          const opts = extractSelectOptions(ui);
          const curVal = boundSig ? String(signals[boundSig] ?? '') : '';
          for (const o of opts) {{
            const optEl = document.createElement('option');
            optEl.value = o.value;
            optEl.textContent = resolveText(o.label, null);
            if (o.value === curVal) optEl.selected = true;
            el.appendChild(optEl);
          }}
          el.addEventListener('change', async e => {{
            if (boundSig) {{
              signals[boundSig] = coerceVal(boundSig, e.target.value);
              savePersisted();
            }}
            await runNodeEvent(ui, 'change', null);
            await runNodeEvent(ui, 'tap', null);
            renderStage();
          }});
        }} else if (ui.widget === 'PROG') {{
          el = document.createElement('div');
          el.className = 'aion-node';
          el.style.pointerEvents = 'none';
          el.style.overflow = 'hidden';
          el.style.borderRadius = (ui.args[2]?.value ?? 6) + 'px';
          el.style.background = ui.args[3]?.value ?? '#E2E8F0';
          const fill = document.createElement('div');
          fill.style.height = '100%';
          fill.style.width = clampPct(sigVal(ui.args[5]?.value)) + '%';
          fill.style.background = ui.args[4]?.value ?? '#2563EB';
          fill.style.transition = 'width .4s linear';
          el.appendChild(fill);
        }} else if (ui.widget === 'RING') {{
          el = document.createElement('div');
          el.className = 'aion-node';
          el.style.pointerEvents = 'none';
          el.style.borderRadius = '50%';
          const thick = ui.args[1]?.value ?? 10;
          const track = ui.args[2]?.value ?? '#E2E8F0';
          const fillC = ui.args[3]?.value ?? '#2563EB';
          const pct = clampPct(sigVal(ui.args[4]?.value));
          el.style.background = 'conic-gradient(' + fillC + ' ' + (pct * 3.6) + 'deg, ' + track + ' 0)';
          const mask = 'radial-gradient(farthest-side, transparent calc(100% - ' + thick + 'px), #000 calc(100% - ' + thick + 'px))';
          el.style.webkitMask = mask;
          el.style.mask = mask;
        }} else if (ui.widget === 'TXT') {{
          el = document.createElement('span');
          el.className = 'aion-node aion-txt';
          el.style.fontSize = (ui.args[0]?.value ?? 16) + 'px';
          el.style.fontWeight = String(ui.args[1]?.value ?? 400);
          el.style.color = ui.args[2]?.value ?? '#0F172A';
          if (ui.body?.body_kind === 'text') {{
            el.dataset.template = ui.body.value.template;
            el.textContent = resolveText(ui.body.value.template, null);
          }}
        }} else {{
          el = document.createElement('div');
          el.className = 'aion-node';
          const isScrollFlex = ui.widget === 'FLEX' && (ui.args || []).some(a => a.value === 'scroll');
          el.style.pointerEvents = (ui.widget === 'LIST' || ui.widget === 'MODAL' || isScrollFlex) ? 'auto' : 'none';
          if (ui.widget === 'MODAL') el.style.background = 'rgba(15,23,42,.55)';
          if (isScrollFlex) el.style.overflow = 'auto';
          if (ui.widget === 'ROOT' && ui.args[4]?.value) el.style.background = ui.args[4].value;
          if (ui.widget === 'NAV' && ui.args[2]?.value) el.style.background = ui.args[2].value;
          if (ui.widget === 'LIST') {{
            el.style.overflow = 'auto';
            el.style.display = 'flex';
            el.style.flexDirection = ui.args[0]?.value === 'h' ? 'row' : 'column';
            el.style.alignItems = 'stretch';
            el.style.gap = (ui.args[1]?.value ?? 8) + 'px';
            const listSig = getDynamicListSig(ui);
            if (listSig) {{
              const items = Array.isArray(signals[listSig]) ? signals[listSig] : [];
              const tplId = ui.body?.body_kind === 'children' ? ui.body.value[0] : null;
              if (tplId) {{
                items.forEach((item, index) => {{
                  const rowEl = renderDynamicNode(tplId, nodeMap, {{ item, index }});
                  if (rowEl) el.appendChild(rowEl);
                }});
              }}
            }}
          }}
        }}

        el.style.left = box.x + 'px';
        el.style.top = box.y + 'px';
        el.style.width = box.width + 'px';
        el.style.height = box.height + 'px';
        stage.appendChild(el);
      }}
      updateSignalsUI();
    }}

    (async () => {{
      const resp = await fetch('./aion_bundle.json');
      bundle = await resp.json();
      signals = {{ ...bundle.signals }};
      initPersisted();
      renderStage();
      for (const n of bundle.program.nodes) {{
        if (n.kind?.node_type === 'ui') runNodeEvent(n.kind, 'init', null);
      }}
      startTicks();
      window.addEventListener('resize', renderStage);
    }})();
  </script>
</body>
</html>
"##
    )
}
