use std::io::{BufRead, Write};

use aion_ast::{DiagnosticReport, PatchOp};
use aion_checker::check_program;
use aion_ext::{manifest_to_wit, ExtRegistry};
use aion_parser::parse_program;
use aion_runtime_ui::{compute_layout, SignalStore, Viewport};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Runs the AION Model Context Protocol (MCP) JSON-RPC 2.0 server over `reader` / `writer` (typically `stdio`).
pub fn serve_mcp_stdio<R: BufRead, W: Write>(
    reader: R,
    mut writer: W,
    registry: &ExtRegistry,
) -> std::io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(err) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("parse error: {err}"),
                    }),
                };
                writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
                writer.flush()?;
                continue;
            }
        };

        if let Some(resp) = handle_mcp_request(req, registry) {
            writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
            writer.flush()?;
        }
    }
    Ok(())
}

fn handle_mcp_request(req: JsonRpcRequest, registry: &ExtRegistry) -> Option<JsonRpcResponse> {
    // Notifications have no `id` and expect no reply.
    let id = req.id?;

    let resp = match req.method.as_str() {
        "initialize" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "aion-mcp",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            error: None,
        },
        "ping" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({})),
            error: None,
        },
        "tools/list" => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({ "tools": tool_definitions() })),
            error: None,
        },
        "tools/call" => {
            let name = req.params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(name, &args, registry) {
                Ok((payload, is_error)) => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&payload).unwrap_or_default()
                        }],
                        "isError": is_error
                    })),
                    error: None,
                },
                Err(msg) => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: msg,
                    }),
                },
            }
        }
        other => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method not found: `{other}`"),
            }),
        },
    };

    Some(resp)
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "aion_check",
            "description": "Parse and validate an AION program (types, circular refs, #EXT ABI contracts, @err handlers, and UI layout overflow).",
            "inputSchema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." }
                }
            }
        },
        {
            "name": "aion_patch",
            "description": "Transactionally apply ADD/PATCH/DEL differential AST patches to an AION program, validate the result, and return canonical formatted_source (optionally writing directly to write_path).",
            "inputSchema": {
                "type": "object",
                "required": ["source", "patches"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." },
                    "patches": {
                        "description": "A single PatchOp object or an array of PatchOp objects."
                    },
                    "write_path": {
                        "type": "string",
                        "description": "Optional path to overwrite with canonical formatted_source if validation succeeds."
                    }
                }
            }
        },
        {
            "name": "aion_inspect_ext",
            "description": "Inspect registered #EXT ABI manifests (functions, types, errors, and WIT interface).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "module_id": { "type": "string", "description": "Optional module id (e.g. aion/mongo)." },
                    "version": { "type": "string", "description": "Optional version constraint (e.g. 1.2)." }
                }
            }
        },
        {
            "name": "aion_layout_tree",
            "description": "Compute the Taffy Flexbox layout boxes and detect UI overflows for mobile (390x844) or desktop (1440x900).",
            "inputSchema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." },
                    "viewport": {
                        "type": "string",
                        "enum": ["mobile", "desktop"],
                        "default": "mobile"
                    }
                }
            }
        },
        {
            "name": "aion_create",
            "description": "Scaffold a canonical minimal AION project (aion.toml, app.aion, AGENTS.md) ready for AI development.",
            "inputSchema": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string", "description": "Project directory name (e.g. xion)." },
                    "dir": { "type": "string", "description": "Optional parent directory path (defaults to current working directory)." }
                }
            }
        },
        {
            "name": "aion_spec",
            "description": "Return the concise AION language grammar, node syntax (#CFG/#EXT/#SRV/#MW/#M/#API/#S/#N), widgets (ROOT/FLEX/NAV/LIST/TXT/INP/BTN), and a full valid example.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "aion_build_android",
            "description": "Validate an AION program and compile it into an Android App Bundle (.aab) via Gradle.",
            "inputSchema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." },
                    "app_id": { "type": "string", "default": "com.aion.app" },
                    "app_name": { "type": "string", "default": "AION App" },
                    "version_code": { "type": "integer", "default": 1 },
                    "version_name": { "type": "string", "default": "0.1.0" },
                    "api_base_url": { "type": "string" },
                    "out_dir": { "type": "string", "default": "build/android" },
                    "debug": { "type": "boolean", "default": false },
                    "generate_only": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "aion_build_web",
            "description": "Validate an AION program and compile a standalone static Web bundle (index.html, aion_bundle.json, schema.proto) ready for hosting.",
            "inputSchema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." },
                    "app_name": { "type": "string", "default": "AION App" },
                    "api_base_url": { "type": "string" },
                    "out_dir": { "type": "string", "default": "build/web" },
                    "release": { "type": "boolean", "default": true },
                    "wasm": { "type": "boolean", "default": false, "description": "Compile Rust taffy + Signals runtime to WebAssembly (.wasm)." }
                }
            }
        },
        {
            "name": "aion_build_ios",
            "description": "Validate an AION program, generate a native SwiftUI Xcode project, and compile an iOS .app (Simulator/Device) or .ipa archive via xcodebuild.",
            "inputSchema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string", "description": "AION source code." },
                    "bundle_id": { "type": "string", "default": "com.aion.app" },
                    "app_name": { "type": "string", "default": "AION App" },
                    "version_name": { "type": "string", "default": "0.1.0" },
                    "build_number": { "type": "integer", "default": 1 },
                    "api_base_url": { "type": "string" },
                    "out_dir": { "type": "string", "default": "build/ios" },
                    "debug": { "type": "boolean", "default": false },
                    "simulator": { "type": "boolean", "default": false },
                    "ipa": { "type": "boolean", "default": false },
                    "no_codesign": { "type": "boolean", "default": true },
                    "team_id": { "type": "string" },
                    "generate_only": { "type": "boolean", "default": false }
                }
            }
        }
    ])
}

fn call_tool(
    name: &str,
    args: &Value,
    registry: &ExtRegistry,
) -> Result<(Value, bool), String> {
    match name {
        "aion_check" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            match parse_program(source) {
                Ok(prog) => {
                    let report = check_program(&prog, registry);
                    let is_err = !report.ok;
                    Ok((serde_json::to_value(report).unwrap(), is_err))
                }
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    Ok((serde_json::to_value(report).unwrap(), true))
                }
            }
        }
        "aion_patch" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            let patches_val = args
                .get("patches")
                .ok_or_else(|| "missing required argument `patches`".to_string())?;
            let ops = parse_patch_ops_value(patches_val)?;

            let mut prog = match parse_program(source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    return Ok((serde_json::to_value(report).unwrap(), true));
                }
            };

            match prog.apply_patches(&ops) {
                Ok(patch_report) => {
                    let check_report = check_program(&prog, registry);
                    let is_err = !check_report.ok;
                    let formatted_source = aion_ast::format_program(&prog);
                    let mut written_to: Option<String> = None;
                    if check_report.ok {
                        if let Some(wp) = args.get("write_path").and_then(Value::as_str) {
                            std::fs::write(wp, &formatted_source)
                                .map_err(|e| format!("failed writing to `{wp}`: {e}"))?;
                            written_to = Some(wp.to_string());
                        }
                    }
                    Ok((
                        json!({
                            "ok": check_report.ok,
                            "patch_report": patch_report,
                            "check_report": check_report,
                            "formatted_source": formatted_source,
                            "written_to": written_to,
                            "program": prog
                        }),
                        is_err,
                    ))
                }
                Err(patch_err) => Ok((
                    json!({
                        "ok": false,
                        "patch_error": patch_err.to_string()
                    }),
                    true,
                )),
            }
        }
        "aion_inspect_ext" => {
            if let Some(module_id) = args.get("module_id").and_then(Value::as_str) {
                let version = args
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        registry
                            .list_modules()
                            .into_iter()
                            .find(|(id, _)| *id == module_id)
                            .map(|(_, v)| v)
                            .unwrap_or("")
                    });
                if let Some(manifest) = registry.resolve(module_id, version) {
                    let wit = manifest_to_wit(manifest);
                    Ok((
                        json!({
                            "ok": true,
                            "manifest": manifest,
                            "wit": wit
                        }),
                        false,
                    ))
                } else {
                    Ok((
                        json!({
                            "ok": false,
                            "error": format!("module `{module_id}` not found"),
                            "available": registry.list_modules()
                        }),
                        true,
                    ))
                }
            } else {
                let modules: Vec<Value> = registry
                    .list_modules()
                    .into_iter()
                    .map(|(id, ver)| json!({ "id": id, "ver": ver }))
                    .collect();
                Ok((json!({ "ok": true, "modules": modules }), false))
            }
        }
        "aion_layout_tree" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            let viewport = match args.get("viewport").and_then(Value::as_str) {
                Some("desktop") => Viewport::DESKTOP,
                _ => Viewport::MOBILE,
            };
            let prog = match parse_program(source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    return Ok((serde_json::to_value(report).unwrap(), true));
                }
            };
            let signals = SignalStore::from_program(&prog);
            match compute_layout(&prog, viewport, Some(&signals)) {
                Ok(tree) => {
                    let overflows = tree.detect_overflows();
                    let has_overflow = !overflows.is_empty();
                    Ok((
                        json!({
                            "ok": !has_overflow,
                            "layout": tree,
                            "overflows": overflows
                        }),
                        has_overflow,
                    ))
                }
                Err(e) => Ok((json!({ "ok": false, "error": e.to_string() }), true)),
            }
        }
        "aion_create" => {
            let project_name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `name`".to_string())?;
            let base_dir = args
                .get("dir")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
            match crate::cmd_create(project_name, &base_dir) {
                Ok(payload) => Ok((payload, false)),
                Err(err) => Ok((json!({ "ok": false, "error": err }), true)),
            }
        }
        "aion_spec" => Ok((
            json!({
                "ok": true,
                "language": "AION",
                "version": env!("CARGO_PKG_VERSION"),
                "types": ["i32", "f64", "str", "bool", "bytes", "ts", "list<T>", "map<K,V>", "void", "any", "#M<id>", "@ExtType"],
                "nodes": {
                    "#CFG": "#CFG:ENV{VAR1, VAR2}",
                    "#EXT": "#EXT:alias = \"org/pkg@ver\"",
                    "#SRV": "#SRV0:HTTP[port:8080, proto:on]",
                    "#MW": "#MW0:PIPE(ctx) { let tok = ctx.hdr(\"Authorization\") ... -> NEXT }",
                    "#M": "#M_Item:Item{id:str, title:str, amount:f64}",
                    "#API": "#API_list:#SRV0.GET[\"/v1/items\", #MW0] OUT{list<#M_Item>} { ... -> RET }",
                    "#S": "#S_items:list<#M_Item> = [] @persist",
                    "#N": "#N_root:ROOT[0,0,100%,100%,#F8FAFC]{#N_main} @init->CALL(#API_list,{})=>#S_items=res @err->TOAST(\"Error\")"
                },
                "ctx_methods": ["ctx.hdr(name)", "ctx.get(key)", "ctx.set(key, val)", "ctx.param(name)", "ctx.query(name)"],
                "widgets": {
                    "ROOT": "ROOT[pad_x, pad_y, 100%, 100%, #BG_HEX]{#N1} (@init->... supported)",
                    "FLEX": "FLEX[v|h, c|s|e|sb|sa, c|s|e|st, gap, scroll?]{#N2,#N3}",
                    "NAV": "NAV[100%, 56, #BG_HEX]{#N_tab1,#N_tab2}",
                    "ROUTER": "ROUTER[#N_initial_screen]{#N_screen1,#N_screen2} (switched via @tap->NAV(#N_screen2))",
                    "LIST": "LIST[v|h, gap, #S_list_opt, noscroll?]{#N_row_tpl} (repeats #N_row_tpl with {item.field} & index)",
                    "TXT": "TXT[font_size, weight, #COLOR_HEX]{\"Text {#S0} or {item.field}\"}",
                    "INP": "INP[width, height, radius, #BG_HEX, #S_bound]{\"Placeholder\"}",
                    "SEL": "SEL[width, height, radius, #BG_HEX, #S_bound]{\"MXN|Peso\", \"USD|Dolar\"} @change->#S_total=#S_monto*#S_tasa (native dropdown; options are \"value\" or \"value|label\")",
                    "BTN": "BTN[width, height, radius, #BG_HEX]{#N_lbl} @tap->PUSH(#S_items,{...}) | REMOVE(#S_items,index) | CALL(#API0,{...})=>#S0=res.f @err->TOAST(\"Error\")"
                },
                "rules": [
                    "Flat graph only (zero nesting). Semantic IDs (#N_home, #S_budget, #M_Expense, #API_save) are recommended.",
                    "Never spin up multiple #SRV servers for multiple screens; use one #SRV0 + ROUTER + @tap->NAV(#N_screen).",
                    "Every #N node with CALL(#API...) MUST declare an @err handler.",
                    "UI must fit mobile (390x844) and desktop (1440x900) without E_UI_OVERFLOW.",
                    "Call aion_inspect_ext before using any #EXT method."
                ]
            }),
            false,
        )),
        "aion_build_android" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            let prog = match parse_program(source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    return Ok((serde_json::to_value(report).unwrap(), true));
                }
            };
            let report = check_program(&prog, registry);
            if !report.ok {
                return Ok((serde_json::to_value(report).unwrap(), true));
            }
            let mut opts = crate::android::AndroidBuildOptions::default();
            if let Some(v) = args.get("app_id").and_then(Value::as_str) {
                opts.app_id = v.to_string();
            }
            if let Some(v) = args.get("app_name").and_then(Value::as_str) {
                opts.app_name = v.to_string();
            }
            if let Some(v) = args.get("version_code").and_then(Value::as_u64) {
                opts.version_code = v as u32;
            }
            if let Some(v) = args.get("version_name").and_then(Value::as_str) {
                opts.version_name = v.to_string();
            }
            if let Some(v) = args.get("api_base_url").and_then(Value::as_str) {
                opts.api_base_url = Some(v.to_string());
            }
            if let Some(v) = args.get("out_dir").and_then(Value::as_str) {
                opts.out_dir = std::path::PathBuf::from(v);
            }
            opts.debug = args.get("debug").and_then(Value::as_bool).unwrap_or(false);
            opts.generate_only = args
                .get("generate_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match crate::android::build_android_aab(&prog, &opts) {
                Ok(res) => Ok((serde_json::to_value(res).unwrap(), false)),
                Err(err) => Ok((json!({ "ok": false, "error": err }), true)),
            }
        }
        "aion_build_web" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            let prog = match parse_program(source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    return Ok((serde_json::to_value(report).unwrap(), true));
                }
            };
            let report = check_program(&prog, registry);
            if !report.ok {
                return Ok((serde_json::to_value(report).unwrap(), true));
            }
            let mut opts = crate::web_build::WebBuildOptions::default();
            if let Some(v) = args.get("app_name").and_then(Value::as_str) {
                opts.app_name = v.to_string();
            }
            if let Some(v) = args.get("api_base_url").and_then(Value::as_str) {
                opts.api_base_url = Some(v.to_string());
            }
            if let Some(v) = args.get("out_dir").and_then(Value::as_str) {
                opts.out_dir = std::path::PathBuf::from(v);
            }
            opts.release = args.get("release").and_then(Value::as_bool).unwrap_or(true);
            opts.wasm = args.get("wasm").and_then(Value::as_bool).unwrap_or(false);
            match crate::web_build::build_web_bundle(&prog, &opts) {
                Ok(res) => Ok((serde_json::to_value(res).unwrap(), false)),
                Err(err) => Ok((json!({ "ok": false, "error": err }), true)),
            }
        }
        "aion_build_ios" => {
            let source = args
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required string argument `source`".to_string())?;
            let prog = match parse_program(source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    return Ok((serde_json::to_value(report).unwrap(), true));
                }
            };
            let report = check_program(&prog, registry);
            if !report.ok {
                return Ok((serde_json::to_value(report).unwrap(), true));
            }
            let mut opts = crate::ios::IosBuildOptions::default();
            if let Some(v) = args.get("bundle_id").and_then(Value::as_str) {
                opts.bundle_id = v.to_string();
            }
            if let Some(v) = args.get("app_name").and_then(Value::as_str) {
                opts.app_name = v.to_string();
            }
            if let Some(v) = args.get("version_name").and_then(Value::as_str) {
                opts.version_name = v.to_string();
            }
            if let Some(v) = args.get("build_number").and_then(Value::as_u64) {
                opts.build_number = v as u32;
            }
            if let Some(v) = args.get("api_base_url").and_then(Value::as_str) {
                opts.api_base_url = Some(v.to_string());
            }
            if let Some(v) = args.get("out_dir").and_then(Value::as_str) {
                opts.out_dir = std::path::PathBuf::from(v);
            }
            if let Some(v) = args.get("team_id").and_then(Value::as_str) {
                opts.team_id = Some(v.to_string());
            }
            opts.debug = args.get("debug").and_then(Value::as_bool).unwrap_or(false);
            opts.simulator = args.get("simulator").and_then(Value::as_bool).unwrap_or(false);
            opts.ipa = args.get("ipa").and_then(Value::as_bool).unwrap_or(false);
            opts.no_codesign = args
                .get("no_codesign")
                .and_then(Value::as_bool)
                .unwrap_or(opts.team_id.is_none());
            opts.generate_only = args
                .get("generate_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match crate::ios::build_ios_app(&prog, &opts) {
                Ok(res) => Ok((serde_json::to_value(res).unwrap(), false)),
                Err(err) => Ok((json!({ "ok": false, "error": err }), true)),
            }
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

pub fn parse_patch_ops_value(val: &Value) -> Result<Vec<PatchOp>, String> {
    if val.is_array() {
        serde_json::from_value::<Vec<PatchOp>>(val.clone())
            .map_err(|e| format!("invalid PatchOp array: {e}"))
    } else {
        serde_json::from_value::<PatchOp>(val.clone())
            .map(|op| vec![op])
            .map_err(|e| format!("invalid PatchOp object: {e}"))
    }
}
