use std::env;
use std::fs;
use std::path::Path;
use std::process;

use aion_ast::DiagnosticReport;
use aion_checker::check_program;
use aion_ext::ExtRegistry;
use aion_parser::{parse_program, parse_to_json_report};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: aion-cli <parse|check> <file.aion> [--manifests-dir <dir>]");
        process::exit(2);
    }

    let cmd = args[1].as_str();
    let file_path = Path::new(&args[2]);
    let manifests_dir = args
        .windows(2)
        .find(|w| w[0] == "--manifests-dir")
        .map(|w| Path::new(&w[1]))
        .unwrap_or_else(|| Path::new("manifests"));

    let source = match fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{{\"ok\":false,\"error\":\"failed to read {}: {}\"}}",
                file_path.display(),
                e
            );
            process::exit(1);
        }
    };

    match cmd {
        "parse" => match parse_to_json_report(&source) {
            Ok(prog) => {
                println!("{}", serde_json::to_string_pretty(&prog).unwrap());
            }
            Err(json_err) => {
                println!("{json_err}");
                process::exit(1);
            }
        },
        "check" => {
            let prog = match parse_program(&source) {
                Ok(p) => p,
                Err(diags) => {
                    let report = DiagnosticReport::from_diagnostics(diags);
                    println!("{}", report.to_json_pretty().unwrap());
                    process::exit(1);
                }
            };
            let mut reg = ExtRegistry::new();
            if manifests_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(manifests_dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some("json") {
                            if let Err(e) = reg.register_manifest_file(&p) {
                                eprintln!(
                                    "{{\"ok\":false,\"error\":\"manifest {}: {}\"}}",
                                    p.display(),
                                    e
                                );
                                process::exit(1);
                            }
                        }
                    }
                }
            }
            let report = check_program(&prog, &reg);
            println!("{}", report.to_json_pretty().unwrap());
            if !report.ok {
                process::exit(1);
            }
        }
        other => {
            eprintln!("Unknown command `{other}`. Expected `parse` or `check`.");
            process::exit(2);
        }
    }
}
