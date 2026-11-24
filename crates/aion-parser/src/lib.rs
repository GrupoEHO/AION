pub mod lexer;
pub mod parser;

use aion_ast::{Diagnostic, DiagnosticReport, Phase, Program, Span};
use chumsky::error::{Simple, SimpleReason};
use chumsky::{Parser, Stream};

use crate::lexer::{lexer, Token};
use crate::parser::program_parser;

/// Parses an AION source string into a flat-graph [`Program`], or returns structured [`Diagnostic`]s.
pub fn parse_program(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let tokens = lexer()
        .parse(source)
        .map_err(|errs| errs.into_iter().map(|e| lex_err_to_diag(source, e)).collect::<Vec<_>>())?;

    let len = source.len();
    let stream = Stream::from_iter(len..len, tokens.into_iter());
    program_parser(source)
        .parse(stream)
        .map_err(|errs| errs.into_iter().map(|e| parse_err_to_diag(source, e)).collect())
}

/// Parses an AION source string and formats any syntax/lexical errors as a JSON [`DiagnosticReport`].
pub fn parse_to_json_report(source: &str) -> Result<Program, String> {
    parse_program(source).map_err(|diags| {
        let report = DiagnosticReport::from_diagnostics(diags);
        report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{e}\"}}"))
    })
}

fn lex_err_to_diag(source: &str, err: Simple<char>) -> Diagnostic {
    let span = Span::from_range(source, err.span().start, err.span().end);
    let expected = format_expected(err.expected().map(|c| match c {
        Some(ch) => format!("'{ch}'"),
        None => "EOF".to_string(),
    }));
    let found = err
        .found()
        .map(|c| format!("'{c}'"))
        .unwrap_or_else(|| "EOF".to_string());
    let message = match err.reason() {
        SimpleReason::Custom(msg) => msg.clone(),
        _ => format!("unexpected character {found} during lexical analysis"),
    };
    let mut diag = Diagnostic::error("E_LEX", Phase::Lexer, message, span)
        .with_found(found)
        .with_fix_hint("Check token syntax, string quotes, or node prefix (#CFG, #EXT, #SRV, #MW, #M, #API, #S, #N).");
    if let Some(exp) = expected {
        diag = diag.with_expected(exp);
    }
    diag
}

fn parse_err_to_diag(source: &str, err: Simple<Token>) -> Diagnostic {
    let span = Span::from_range(source, err.span().start, err.span().end);
    let expected = format_expected(err.expected().map(|t| match t {
        Some(tok) => format!("`{tok}`"),
        None => "EOF".to_string(),
    }));
    let found = err
        .found()
        .map(|t| format!("`{t}`"))
        .unwrap_or_else(|| "EOF".to_string());

    let message = match err.reason() {
        SimpleReason::Custom(msg) => msg.clone(),
        SimpleReason::Unclosed { delimiter, .. } => {
            format!("unclosed delimiter `{delimiter}` before {found}")
        }
        SimpleReason::Unexpected => match &expected {
            Some(exp) => format!("unexpected token {found}, expected {exp}"),
            None => format!("unexpected token {found}"),
        },
    };

    let fix_hint = match &expected {
        Some(exp) => format!("Replace or insert token near line {} col {}: expected {exp}.", span.line, span.col),
        None => format!("Check syntax near line {} col {}.", span.line, span.col),
    };

    let mut diag = Diagnostic::error("E_SYNTAX", Phase::Parser, message, span)
        .with_found(found)
        .with_fix_hint(fix_hint);
    if let Some(exp) = expected {
        diag = diag.with_expected(exp);
    }
    diag
}

fn format_expected(iter: impl Iterator<Item = String>) -> Option<String> {
    let mut items: Vec<String> = Vec::new();
    for item in iter {
        if !items.contains(&item) {
            items.push(item);
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(items.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_ast::{NodeId, NodeKind, UiBody};

    const EXAMPLE_SRC: &str = r#"
// Configuración y módulos externos (intercambiables sin tocar el lenguaje)
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"

// Servidor HTTP genérico con soporte nativo Protobuf
#SRV0:HTTP[port:8080, proto:on]

// Middleware genérico (agnóstico al proveedor)
#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}

// Modelo de contrato único (Backend + Protobuf + Frontend)
#M0:User{id:str, uid:str, pts:i32}

// Endpoint REST/Protobuf usando el servidor genérico y el driver externo
#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}

// Frontend Multiplataforma (Web/Android/iOS)
#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2,#N3}
#N2:TXT[24,700,#0F172A]{"Puntos: {#S0}"}
#N3:BTN[160,48,8,#2563EB]{#N4} @tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")
#N4:TXT[16,600,#FFFFFF]{"Sumar +10"}
"#;

    #[test]
    fn parses_architecture_example() {
        let prog = parse_program(EXAMPLE_SRC).expect("should parse architecture example");
        assert_eq!(prog.nodes.len(), 13);
        assert!(prog.get(&NodeId::new("#CFG")).is_some());
        assert!(prog.get(&NodeId::new("#EXT:db")).is_some());
        assert!(prog.get(&NodeId::new("#EXT:auth")).is_some());
        assert!(prog.get(&NodeId::new("#SRV0")).is_some());
        assert!(prog.get(&NodeId::new("#MW0")).is_some());
        assert!(prog.get(&NodeId::new("#M0")).is_some());
        assert!(prog.get(&NodeId::new("#API0")).is_some());
        assert!(prog.get(&NodeId::new("#S0")).is_some());

        let n2 = prog.get(&NodeId::new("#N2")).unwrap();
        match &n2.kind {
            NodeKind::Ui(ui) => match &ui.body {
                UiBody::Text { interpolations, .. } => {
                    assert_eq!(interpolations, &vec![NodeId::new("#S0")]);
                }
                other => panic!("unexpected body for #N2: {other:?}"),
            },
            other => panic!("unexpected kind for #N2: {other:?}"),
        }
    }

    #[test]
    fn emits_structured_json_errors_on_syntax_failure() {
        let bad = r#"#S0:i32 = "#;
        let json_str = parse_to_json_report(bad).unwrap_err();
        let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(val["ok"], false);
        assert!(val["error_count"].as_u64().unwrap() >= 1);
        assert_eq!(val["diagnostics"][0]["code"], "E_SYNTAX");
        assert_eq!(val["diagnostics"][0]["phase"], "parser");
        assert!(val["diagnostics"][0]["fix_hint"].as_str().is_some());
    }
}
