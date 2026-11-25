pub mod mcp;
pub mod web;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use aion_ast::{DiagnosticReport, Program};
use aion_checker::check_program;
use aion_ext::ExtRegistry;
use aion_ext_firebase::FirebaseAuthPlugin;
use aion_ext_mongo::MongoPlugin;
use aion_parser::{parse_program, parse_to_json_report};
use serde_json::{json, Value};

use crate::mcp::parse_patch_ops_value;

/// Builds an [`ExtRegistry`] preloaded with built-in plugins (`aion-ext-firebase`, `aion-ext-mongo`)
/// plus any additional `.json` ABI manifests in `manifests_dir`.
pub fn build_registry(manifests_dir: Option<&Path>) -> Result<ExtRegistry, String> {
    let mut reg = ExtRegistry::new();
    reg.register_plugin(Arc::new(FirebaseAuthPlugin::new()))
        .map_err(|e| format!("failed to register firebase plugin: {e}"))?;
    reg.register_plugin(Arc::new(MongoPlugin::new()))
        .map_err(|e| format!("failed to register mongo plugin: {e}"))?;

    if let Some(dir) = manifests_dir {
        if dir.is_dir() {
            let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("json") {
                    let manifest = aion_ext::ExtManifest::from_file(&p)
                        .map_err(|e| format!("manifest {}: {e}", p.display()))?;
                    if reg.resolve(&manifest.id, &manifest.ver).is_none() {
                        reg.register_manifest(manifest)
                            .map_err(|e| format!("register {}: {e}", p.display()))?;
                    }
                }
            }
        }
    }

    Ok(reg)
}

/// Executes `aion create <name> [dir]`: scaffolds a canonical 3-file AION project
/// (`aion.toml`, `app.aion`, `AGENTS.md`) ready for AI-driven development.
pub fn cmd_create(project_name: &str, base_dir: &Path) -> Result<serde_json::Value, String> {
    let trimmed = project_name.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "invalid project name `{project_name}`: use ASCII letters, digits, `-`, or `_`"
        ));
    }

    let project_dir = base_dir.join(trimmed);
    if project_dir.exists() {
        return Err(format!(
            "directory `{}` already exists",
            project_dir.display()
        ));
    }

    fs::create_dir_all(&project_dir)
        .map_err(|e| format!("failed to create {}: {e}", project_dir.display()))?;

    let aion_toml = format!(
        r#"[project]
name = "{trimmed}"
version = "0.1.0"
entry = "app.aion"
targets = ["web", "android", "ios"]

[ext]
# Add external ABI modules as needed:
# db = "aion/mongo@1.2"
# auth = "aion/firebase-auth@2.0"
"#
    );

    let app_aion = format!(
        r#"// {trimmed} — Canonical AION Full-Stack Starter
#SRV0:HTTP[port:8080, proto:on]

#M0:StatusRes{{message:str, count:i32}}

#API0:#SRV0.POST["/v1/ping"] IN{{current:i32}} OUT{{#M0}} {{
  {{message: "ok", count: in.current + 1}} -> RET
}}

#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{{#N1}}
#N1:FLEX[v,c,c,16]{{#N2,#N3,#N4}}
#N2:TXT[22,700,#0F172A]{{"{trimmed} · AION"}}
#N3:TXT[16,500,#334155]{{"Contador: {{#S0}}"}}
#N4:BTN[180,46,8,#2563EB]{{#N5}} @tap->CALL(#API0,{{current:#S0}})=>#S0=res.count @err->TOAST("Error")
#N5:TXT[15,600,#FFFFFF]{{"Incrementar (+1)"}}
"#
    );

    let agents_md = format!(
        r#"# {trimmed} — AION Agent Guidelines

1. **Zero Nesting:** Keep `app.aion` strictly flat (`#CFG`, `#EXT`, `#SRV`, `#MW`, `#M`, `#API`, `#S`, `#N`).
2. **Verify External Modules First:** Always call MCP tool `aion_inspect_ext` before invoking any `#EXT` method.
3. **Mandatory Validation Loop:** After every edit or `aion_patch`, run:
   - `aion_check` (must return `ok: true` with `0` type, `#EXT`, `@err`, and `E_UI_OVERFLOW` errors).
   - `aion_layout_tree` for both `mobile` (`390x844`) and `desktop` (`1440x900`).
4. **Live Web Preview:** Run `aion web app.aion` to test the UI + `#API` endpoints in the browser.
"#
    );

    let toml_path = project_dir.join("aion.toml");
    let app_path = project_dir.join("app.aion");
    let agents_path = project_dir.join("AGENTS.md");

    fs::write(&toml_path, aion_toml).map_err(|e| format!("write aion.toml: {e}"))?;
    fs::write(&app_path, &app_aion).map_err(|e| format!("write app.aion: {e}"))?;
    fs::write(&agents_path, agents_md).map_err(|e| format!("write AGENTS.md: {e}"))?;

    Ok(json!({
        "ok": true,
        "project": trimmed,
        "path": project_dir.display().to_string(),
        "files": [
            toml_path.display().to_string(),
            app_path.display().to_string(),
            agents_path.display().to_string(),
        ],
        "next_steps": [
            format!("aion check {}/app.aion", project_dir.display()),
            format!("aion web {}/app.aion", project_dir.display()),
        ]
    }))
}

/// Executes `aion parse`: returns pretty JSON of the AST or structured JSON errors.
pub fn cmd_parse(source: &str) -> Result<String, String> {
    let prog = parse_to_json_report(source)?;
    serde_json::to_string_pretty(&prog).map_err(|e| e.to_string())
}

/// Executes `aion check`: parses and runs `aion-checker` (types + `#EXT` + `@err` + layout).
pub fn cmd_check(source: &str, registry: &ExtRegistry) -> (bool, String) {
    let report = match parse_program(source) {
        Ok(prog) => check_program(&prog, registry),
        Err(diags) => DiagnosticReport::from_diagnostics(diags),
    };
    let ok = report.ok;
    let json = report
        .to_json_pretty()
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{e}\"}}"));
    (ok, json)
}

/// Executes `aion patch`: applies `PatchOp`/`PatchDelta` transactionally and validates with `aion-checker`.
pub fn cmd_patch(
    source: &str,
    patch_json: &str,
    registry: &ExtRegistry,
) -> (bool, String, Option<Program>) {
    let mut prog = match parse_program(source) {
        Ok(p) => p,
        Err(diags) => {
            let report = DiagnosticReport::from_diagnostics(diags);
            return (false, report.to_json_pretty().unwrap(), None);
        }
    };

    let patch_val: Value = match serde_json::from_str(patch_json) {
        Ok(v) => v,
        Err(e) => {
            return (
                false,
                json!({ "ok": false, "error": format!("invalid patch JSON: {e}") }).to_string(),
                None,
            );
        }
    };

    let ops = match parse_patch_ops_value(&patch_val) {
        Ok(ops) => ops,
        Err(e) => {
            return (
                false,
                json!({ "ok": false, "error": e }).to_string(),
                None,
            );
        }
    };

    match prog.apply_patches(&ops) {
        Ok(patch_report) => {
            let check_report = check_program(&prog, registry);
            let ok = check_report.ok;
            let out = serde_json::to_string_pretty(&json!({
                "ok": ok,
                "patch_report": patch_report,
                "check_report": check_report,
                "program": prog
            }))
            .unwrap();
            (ok, out, Some(prog))
        }
        Err(err) => (
            false,
            serde_json::to_string_pretty(&json!({
                "ok": false,
                "patch_error": err.to_string()
            }))
            .unwrap(),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::serve_mcp_stdio;
    use std::io::Cursor;

    const EXAMPLE_AION: &str = r#"
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"
#SRV0:HTTP[port:8080, proto:on]
#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}
#M0:User{id:str, uid:str, pts:i32}
#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}
#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2,#N3}
#N2:TXT[24,700,#0F172A]{"Puntos: {#S0}"}
#N3:BTN[160,48,8,#2563EB]{#N4} @tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")
#N4:TXT[16,600,#FFFFFF]{"Sumar +10"}
"#;

    #[test]
    fn cli_parse_check_and_patch_commands_work() {
        let reg = build_registry(None).unwrap();

        let ast_json = cmd_parse(EXAMPLE_AION).expect("parse should succeed");
        assert!(ast_json.contains("#API0"));

        let (ok, check_json) = cmd_check(EXAMPLE_AION, &reg);
        assert!(ok, "expected check ok, got: {check_json}");

        let patch_op = aion_ast::PatchOp::Patch {
            id: aion_ast::NodeId::new("#S0"),
            delta: aion_ast::PatchDelta::SetSignalInit {
                init: aion_ast::Expr {
                    kind: aion_ast::ExprKind::Int(42),
                    span: aion_ast::Span::default(),
                },
            },
        };
        let patch_json = serde_json::to_string(&patch_op).unwrap();
        let (patch_ok, patch_out, patched_prog) = cmd_patch(EXAMPLE_AION, &patch_json, &reg);
        assert!(patch_ok, "expected patch ok, got: {patch_out}");
        assert!(patched_prog.is_some());
    }

    #[test]
    fn mcp_stdio_exposes_and_executes_all_four_tools() {
        let reg = build_registry(None).unwrap();

        let patch_op = aion_ast::PatchOp::Patch {
            id: aion_ast::NodeId::new("#S0"),
            delta: aion_ast::PatchDelta::SetSignalInit {
                init: aion_ast::Expr {
                    kind: aion_ast::ExprKind::Int(99),
                    span: aion_ast::Span::default(),
                },
            },
        };

        let requests = [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "aion_check",
                    "arguments": { "source": EXAMPLE_AION }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "aion_patch",
                    "arguments": {
                        "source": EXAMPLE_AION,
                        "patches": [patch_op]
                    }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "aion_inspect_ext",
                    "arguments": { "module_id": "aion/mongo", "version": "1.2" }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": {
                    "name": "aion_layout_tree",
                    "arguments": {
                        "source": "#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}\n#N1:FLEX[v,c,c,16]{#N2}\n#N2:BTN[500,48,8,#2563EB]{}",
                        "viewport": "mobile"
                    }
                }
            }),
        ];

        let input = requests
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n");

        let mut output = Vec::new();
        serve_mcp_stdio(Cursor::new(input.as_bytes()), &mut output, &reg).unwrap();

        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // 6 responses (notification produces no response)
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "aion-mcp");

        // tools/list returns all 4 required MCP tools
        let tools = lines[1]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "aion_check",
                "aion_patch",
                "aion_inspect_ext",
                "aion_layout_tree",
                "aion_create",
                "aion_spec"
            ]
        );

        // aion_check succeeds on EXAMPLE_AION
        assert_eq!(lines[2]["result"]["isError"], false);

        // aion_patch succeeds transactionally and validates
        assert_eq!(lines[3]["result"]["isError"], false);
        let patch_text = lines[3]["result"]["content"][0]["text"].as_str().unwrap();
        let patch_body: Value = serde_json::from_str(patch_text).unwrap();
        assert_eq!(patch_body["ok"], true);
        assert_eq!(patch_body["patch_report"]["applied"], 1);

        // aion_inspect_ext returns manifest + WIT for aion/mongo
        assert_eq!(lines[4]["result"]["isError"], false);
        let ext_text = lines[4]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(ext_text.contains("aion/mongo"));
        assert!(ext_text.contains("interface mongo"));

        // aion_layout_tree detects 500px overflow on mobile 390px viewport
        assert_eq!(lines[5]["result"]["isError"], true);
        let layout_text = lines[5]["result"]["content"][0]["text"].as_str().unwrap();
        let layout_body: Value = serde_json::from_str(layout_text).unwrap();
        assert_eq!(layout_body["ok"], false);
        assert_eq!(layout_body["overflows"][0]["child_id"], "#N2");
    }

    #[test]
    fn cmd_create_scaffolds_valid_aion_project() {
        let reg = build_registry(None).unwrap();
        let tmp = std::env::temp_dir().join(format!("aion_create_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let res = cmd_create("xion", &tmp).expect("create should succeed");
        assert_eq!(res["ok"], true);
        let app_src = fs::read_to_string(tmp.join("xion/app.aion")).unwrap();
        let (ok, report_json) = cmd_check(&app_src, &reg);
        assert!(ok, "starter app.aion must pass aion check: {report_json}");
        assert!(tmp.join("xion/aion.toml").is_file());
        assert!(tmp.join("xion/AGENTS.md").is_file());

        let _ = fs::remove_dir_all(&tmp);
    }
}
