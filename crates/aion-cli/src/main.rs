use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use aion_ast::DiagnosticReport;
use aion_checker::check_program;
use aion_cli::android::{build_android_aab, AndroidBuildOptions};
use aion_cli::mcp::serve_mcp_stdio;
use aion_cli::web::serve_web;
use aion_cli::{build_registry, cmd_check, cmd_create, cmd_parse, cmd_patch};
use aion_parser::parse_program;
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
            "Usage:\n  aion create <name> [dir]\n  aion parse <file.aion>\n  aion check <file.aion> [--manifests-dir <dir>]\n  aion patch <file.aion> <patch.json> [--manifests-dir <dir>]\n  aion build android <file.aion> [--app-id <id>] [--name <name>] [--version-code <n>] [--version-name <ver>] [--api-base-url <url>] [--out <dir>] [--debug] [--generate-only]\n  aion run <file.aion> [--manifests-dir <dir>]\n  aion web <file.aion> [--manifests-dir <dir>]\n  aion mcp [--manifests-dir <dir>]"
        );
        process::exit(2);
    }

    let cmd = args[1].as_str();
    let manifests_dir = find_manifests_dir(&args);

    match cmd {
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
            let src = read_file_or_exit(Path::new(&args[2]));
            match cmd_parse(&src) {
                Ok(json) => println!("{json}"),
                Err(json_err) => {
                    println!("{json_err}");
                    process::exit(1);
                }
            }
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Usage: aion check <file.aion> [--manifests-dir <dir>]");
                process::exit(2);
            }
            let src = read_file_or_exit(Path::new(&args[2]));
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let (ok, json) = cmd_check(&src, &reg);
            println!("{json}");
            if !ok {
                process::exit(1);
            }
        }
        "patch" => {
            if args.len() < 4 {
                eprintln!("Usage: aion patch <file.aion> <patch.json> [--manifests-dir <dir>]");
                process::exit(2);
            }
            let src = read_file_or_exit(Path::new(&args[2]));
            let patch_json = read_file_or_exit(Path::new(&args[3]));
            let reg = build_registry(Some(&manifests_dir)).unwrap_or_else(|e| {
                eprintln!("{{\"ok\":false,\"error\":\"{e}\"}}");
                process::exit(1);
            });
            let (ok, out, _) = cmd_patch(&src, &patch_json, &reg);
            println!("{out}");
            if !ok {
                process::exit(1);
            }
        }
        "build" => {
            if args.len() < 4 || args[2] != "android" {
                eprintln!("Usage: aion build android <file.aion> [--app-id com.aion.app] [--name \"App\"] [--version-code 1] [--version-name 0.1.0] [--api-base-url http://10.0.2.2:8080] [--out build/android] [--debug] [--generate-only]");
                process::exit(2);
            }
            let file_path = Path::new(&args[3]);
            let src = read_file_or_exit(file_path);
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

            let mut opts = AndroidBuildOptions::default();
            if let Some(v) = find_flag(&args, "--app-id") {
                opts.app_id = v;
            }
            if let Some(v) = find_flag(&args, "--name") {
                opts.app_name = v;
            }
            if let Some(v) = find_flag(&args, "--version-code") {
                opts.version_code = v.parse().unwrap_or(1);
            }
            if let Some(v) = find_flag(&args, "--version-name") {
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
            opts.debug = args.iter().any(|a| a == "--debug");
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
            if let Err(e) = rt.block_on(serve_web(prog, Arc::new(reg), RuntimeConfig::new())) {
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
