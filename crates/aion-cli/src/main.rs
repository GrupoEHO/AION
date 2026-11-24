use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use aion_ast::DiagnosticReport;
use aion_checker::check_program;
use aion_cli::mcp::serve_mcp_stdio;
use aion_cli::web::serve_web;
use aion_cli::{build_registry, cmd_check, cmd_parse, cmd_patch};
use aion_parser::parse_program;
use aion_runtime_net::{serve, RuntimeConfig};

fn find_manifests_dir(args: &[String]) -> PathBuf {
    args.windows(2)
        .find(|w| w[0] == "--manifests-dir")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("manifests"))
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
            "Usage:\n  aion parse <file.aion>\n  aion check <file.aion> [--manifests-dir <dir>]\n  aion patch <file.aion> <patch.json> [--manifests-dir <dir>]\n  aion run <file.aion> [--manifests-dir <dir>]\n  aion web <file.aion> [--manifests-dir <dir>]\n  aion mcp [--manifests-dir <dir>]"
        );
        process::exit(2);
    }

    let cmd = args[1].as_str();
    let manifests_dir = find_manifests_dir(&args);

    match cmd {
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
