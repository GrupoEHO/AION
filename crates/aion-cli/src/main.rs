use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use aion_ast::DiagnosticReport;
use aion_checker::check_program;
use aion_cli::android::{build_android_aab, AndroidBuildOptions};
use aion_cli::ios::{build_ios_app, IosBuildOptions};
use aion_cli::mcp::serve_mcp_stdio;
use aion_cli::web::serve_web;
use aion_cli::web_build::{build_web_bundle, ProjectMeta, WebBuildOptions};
use aion_cli::{build_registry, cmd_create, cmd_patch};
use aion_parser::{parse_file_with_includes, parse_program};
use aion_runtime_net::{serve, RuntimeConfig};

fn find_manifests_dir(args: &[String]) -> PathBuf {
    args.windows(2)
        .find(|w| w[0] == "--manifests-dir")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("manifests"))
}

fn find_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn read_file_or_exit(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{{\"ok\":false,\"error\":\"failed to read {}: {}\"}}",
                path.display(),
                e
            );
            process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!(
            "Usage:\n  aion create <name> [dir]\n  aion parse <file.aion>\n  aion check <file.aion> [--manifests-dir <dir>]\n  aion patch <file.aion> <patch.json> [--manifests-dir <dir>]\n  aion build appbundle [--release|--debug] [file.aion] [--app-id <id>] [--name <name>] [--out <dir>]\n  aion build ios|ipa [--release|--debug] [--simulator] [--no-codesign] [file.aion] [--bundle-id <id>] [--name <name>] [--team-id <id>] [--out <dir>]\n  aion build web [--release|--debug] [--wasm] [file.aion] [--name <name>] [--api-base-url <url>] [--out <dir>]\n  aion run <file.aion> [--manifests-dir <dir>]\n  aion web <file.aion> [--manifests-dir <dir>]\n  aion mcp [--manifests-dir <dir>]"
        );
        process::exit(2);
    }

    let cmd = args[1].as_str();
    let manifests_dir = find_manifests_dir(&args);

    match cmd {
        "--version" | "-V" | "version" => {
            println!("aion {}", env!("CARGO_PKG_VERSION"));
        }
        "create" => {
            if args.len() < 3 {
                eprintln!("Usage: aion create <name> [dir]");
                process::exit(2);
            }
            let base_dir = args
                .get(3)
                .map(PathBuf::from)
                .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            match cmd_create(&args[2], &base_dir) {
                Ok(payload) => println!("{}", serde_json::to_string_pretty(&payload).unwrap()),
                Err(err) => {
                    println!("{{\"ok\":false,\"error\":\"{err}\"}}");
                    process::exit(1);
                }
            }
        }
        "parse" => {
            if args.len() < 3 {
                eprintln!("Usage: aion parse <file.aion>");
                process::exit(2);
            }
            match parse_file_with_includes(Path::new(&args[2])) {
                Ok(prog) => println!("{}", serde_json::to_string_pretty(&prog).unwrap()),
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            }
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Usage: aion check <file.aion> [--manifests-dir <dir>]");
                process::exit(2);
            }
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let prog = match parse_file_with_includes(Path::new(&args[2])) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let report = check_program(&prog, &reg);
            println!("{}", report.to_json_pretty().unwrap());
            if !report.ok {
                process::exit(1);
            }
        }
        "fmt" => {
            if args.len() < 3 {
                eprintln!("Usage: aion fmt <file.aion> [--write]");
                process::exit(2);
            }
            let target_file = Path::new(&args[2]);
            let src = read_file_or_exit(target_file);
            let prog = match parse_program(&src) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let formatted = aion_ast::format_program(&prog);
            if args.iter().any(|a| a == "--write" || a == "-w") {
                if let Err(e) = fs::write(target_file, &formatted) {
                    eprintln!("{{\"ok\":false,\"error\":\"failed to write {}: {}\"}}", target_file.display(), e);
                    process::exit(1);
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": true,
                        "file": target_file.display().to_string(),
                        "bytes": formatted.len()
                    }))
                    .unwrap()
                );
            } else {
                print!("{formatted}");
            }
        }
        "patch" => {
            if args.len() < 4 {
                eprintln!("Usage: aion patch <file.aion> <patch.json> [--write] [--manifests-dir <dir>]");
                process::exit(2);
            }
            let target_file = Path::new(&args[2]);
            let src = read_file_or_exit(target_file);
            let patch_json = read_file_or_exit(Path::new(&args[3]));
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let (ok, out, patched_prog) = cmd_patch(&src, &patch_json, &reg);
            if ok && args.iter().any(|a| a == "--write" || a == "-w") {
                if let Some(prog) = patched_prog {
                    let formatted = aion_ast::format_program(&prog);
                    if let Err(e) = fs::write(target_file, formatted) {
                        eprintln!("{{\"ok\":false,\"error\":\"failed to write {}: {}\"}}", target_file.display(), e);
                        process::exit(1);
                    }
                }
            }
            println!("{out}");
            if !ok {
                process::exit(1);
            }
        }
        "build" => {
            if args.len() < 3 {
                eprintln!("Usage:\n  aion build appbundle [--release|--debug] [file.aion]\n  aion build web [--release|--debug] [file.aion]");
                process::exit(2);
            }
            let target = args[2].as_str();
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let meta = ProjectMeta::load_from_dir(&cwd);

            // Optional positional `.aion` path anywhere after `aion build <target>`
            // (if omitted, reads `entry` from `aion.toml` or `app.aion`).
            let file_path = args
                .iter()
                .skip(3)
                .find(|a| !a.starts_with('-') && a.ends_with(".aion"))
                .or_else(|| args.get(3).filter(|a| !a.starts_with('-')))
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(meta.entry.clone().unwrap_or_else(|| "app.aion".to_string()))
                });

            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let prog = match parse_file_with_includes(&file_path) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let report = check_program(&prog, &reg);
            if !report.ok {
                println!("{}", report.to_json_pretty().unwrap());
                process::exit(1);
            }

            let is_debug = args.iter().any(|a| a == "--debug");
            let is_release = args.iter().any(|a| a == "--release") || !is_debug;

            match target {
                "appbundle" | "android" => {
                    let mut opts = AndroidBuildOptions::default();
                    if let Some(ref name) = meta.name {
                        opts.app_name = name.clone();
                        let safe_pkg: String = name
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .map(|c| c.to_ascii_lowercase())
                            .collect();
                        if !safe_pkg.is_empty() {
                            opts.app_id = format!("com.aion.{safe_pkg}");
                        }
                    }
                    if let Some(ref app_id) = meta.app_id {
                        opts.app_id = app_id.clone();
                    }
                    if let Some(ref ver) = meta.version {
                        opts.version_name = ver.clone();
                    }
                    if let Some(bn) = meta.build_number {
                        opts.version_code = bn;
                    }
                    if let Some(v) = find_flag(&args, "--app-id") {
                        opts.app_id = v;
                    }
                    if let Some(v) = find_flag(&args, "--name") {
                        opts.app_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--version-code")
                        .or_else(|| find_flag(&args, "--build-number"))
                    {
                        opts.version_code = v.parse().unwrap_or(1);
                    }
                    if let Some(v) = find_flag(&args, "--version-name")
                        .or_else(|| find_flag(&args, "--version"))
                    {
                        opts.version_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--api-base-url") {
                        opts.api_base_url = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--out") {
                        opts.out_dir = PathBuf::from(v);
                    }
                    if let Some(v) = find_flag(&args, "--keystore") {
                        opts.keystore = Some(PathBuf::from(v));
                    }
                    if let Some(v) = find_flag(&args, "--keystore-pass") {
                        opts.keystore_pass = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--key-alias") {
                        opts.key_alias = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--key-pass") {
                        opts.key_pass = Some(v);
                    }
                    opts.debug = is_debug;
                    opts.generate_only = args.iter().any(|a| a == "--generate-only");

                    match build_android_aab(&prog, &opts) {
                        Ok(res) => println!("{}", serde_json::to_string_pretty(&res).unwrap()),
                        Err(err) => {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "ok": false,
                                    "error": err
                                }))
                                .unwrap()
                            );
                            process::exit(1);
                        }
                    }
                }
                "web" => {
                    let mut opts = WebBuildOptions::default();
                    if let Some(ref name) = meta.name {
                        opts.app_name = name.clone();
                    }
                    if let Some(ref ver) = meta.version {
                        opts.version_name = ver.clone();
                    }
                    if let Some(bn) = meta.build_number {
                        opts.build_number = bn;
                    }
                    if let Some(ref aion_v) = meta.aion_version {
                        opts.aion_version = aion_v.clone();
                    }
                    if let Some(v) = find_flag(&args, "--name") {
                        opts.app_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--version")
                        .or_else(|| find_flag(&args, "--version-name"))
                    {
                        opts.version_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--build-number")
                        .or_else(|| find_flag(&args, "--version-code"))
                    {
                        opts.build_number = v.parse().unwrap_or(1);
                    }
                    if let Some(v) = find_flag(&args, "--api-base-url") {
                        opts.api_base_url = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--out") {
                        opts.out_dir = PathBuf::from(v);
                    }
                    opts.release = is_release;
                    opts.wasm = args.iter().any(|a| a == "--wasm");

                    match build_web_bundle(&prog, &opts) {
                        Ok(res) => println!("{}", serde_json::to_string_pretty(&res).unwrap()),
                        Err(err) => {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "ok": false,
                                    "error": err
                                }))
                                .unwrap()
                            );
                            process::exit(1);
                        }
                    }
                }
                "ios" | "ipa" => {
                    let mut opts = IosBuildOptions::default();
                    if let Some(ref name) = meta.name {
                        opts.app_name = name.clone();
                        let safe_pkg: String = name
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .map(|c| c.to_ascii_lowercase())
                            .collect();
                        if !safe_pkg.is_empty() {
                            opts.bundle_id = format!("com.aion.{safe_pkg}");
                        }
                    }
                    if let Some(ref app_id) = meta.app_id {
                        opts.bundle_id = app_id.clone();
                    }
                    if let Some(ref ver) = meta.version {
                        opts.version_name = ver.clone();
                    }
                    if let Some(bn) = meta.build_number {
                        opts.build_number = bn;
                    }
                    if let Some(v) = find_flag(&args, "--bundle-id").or_else(|| find_flag(&args, "--app-id")) {
                        opts.bundle_id = v;
                    }
                    if let Some(v) = find_flag(&args, "--name") {
                        opts.app_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--version").or_else(|| find_flag(&args, "--version-name")) {
                        opts.version_name = v;
                    }
                    if let Some(v) = find_flag(&args, "--build-number") {
                        opts.build_number = v.parse().unwrap_or(1);
                    }
                    if let Some(v) = find_flag(&args, "--team-id") {
                        opts.team_id = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--export-method") {
                        opts.export_method = v;
                    }
                    if let Some(v) = find_flag(&args, "--api-base-url") {
                        opts.api_base_url = Some(v);
                    }
                    if let Some(v) = find_flag(&args, "--out") {
                        opts.out_dir = PathBuf::from(v);
                    }
                    opts.debug = is_debug;
                    opts.simulator = args.iter().any(|a| a == "--simulator");
                    opts.ipa = target == "ipa" || args.iter().any(|a| a == "--ipa");
                    opts.no_codesign = args.iter().any(|a| a == "--no-codesign");
                    opts.generate_only = args.iter().any(|a| a == "--generate-only");

                    match build_ios_app(&prog, &opts) {
                        Ok(res) => println!("{}", serde_json::to_string_pretty(&res).unwrap()),
                        Err(err) => {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&serde_json::json!({
                                    "ok": false,
                                    "error": err
                                }))
                                .unwrap()
                            );
                            process::exit(1);
                        }
                    }
                }
                other => {
                    eprintln!("Unknown build target `{other}`. Expected `appbundle` (or `android`), `ios`, `ipa`, or `web`.");
                    process::exit(2);
                }
            }
        }
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: aion run <file.aion> [--manifests-dir <dir>]");
                process::exit(2);
            }
            let src = read_file_or_exit(Path::new(&args[2]));
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let prog = match parse_program(&src) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let report = check_program(&prog, &reg);
            if !report.ok {
                println!("{}", report.to_json_pretty().unwrap());
                process::exit(1);
            }
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| {
                    eprintln!("{{\"ok\":false,\"error\":\"tokio runtime error: {e}\"}}");
                    process::exit(1);
                });
            if let Err(e) = rt.block_on(serve(prog, Arc::new(reg), RuntimeConfig::new())) {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            }
        }
        "web" => {
            if args.len() < 3 {
                eprintln!("Usage: aion web <file.aion> [--manifests-dir <dir>]");
                process::exit(2);
            }
            let web_file = PathBuf::from(&args[2]);
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let prog = match parse_file_with_includes(&web_file) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let report = check_program(&prog, &reg);
            if !report.ok {
                println!("{}", report.to_json_pretty().unwrap());
                process::exit(1);
            }
            let open_browser = !args.iter().any(|a| a == "--no-open");
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| {
                    eprintln!("{{\"ok\":false,\"error\":\"tokio runtime error: {e}\"}}");
                    process::exit(1);
                });
            if let Err(e) = rt.block_on(serve_web(
                Some(web_file),
                prog,
                Arc::new(reg),
                RuntimeConfig::new(),
                open_browser,
            )) {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            }
        }
        "mcp" => {
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let stdin = io::stdin();
            let stdout = io::stdout();
            if let Err(e) = serve_mcp_stdio(stdin.lock(), stdout.lock(), &reg) {
                eprintln!("MCP stdio error: {e}");
                process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown command `{other}`. Expected: parse | check | patch | run | mcp");
            process::exit(2);
        }
    }
}
