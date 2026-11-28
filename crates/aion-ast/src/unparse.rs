use std::fmt::Write;

use crate::ast::{
    ApiOutput, BinaryOp, ErrAction, EventAction, EventHandler, Expr, ExprKind, FlatNode,
    FlowControl, HttpMethod, NodeKind, Program, PropValue, Stmt, UiArg, UiBody,
};

/// Formats a flat-graph [`Program`] into canonical, human- and AI-readable `.aion` source code.
pub fn format_program(program: &Program) -> String {
    let mut out = String::new();
    for (idx, node) in program.nodes.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        format_flat_node(node, &mut out);
    }
    if !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Formats a single [`FlatNode`] into canonical `.aion` syntax.
pub fn format_flat_node(node: &FlatNode, out: &mut String) {
    match &node.kind {
        NodeKind::Cfg(cfg) => {
            let vars: Vec<String> = cfg
                .vars
                .iter()
                .map(|v| {
                    if v.ty == crate::types::AionType::Str {
                        v.name.clone()
                    } else {
                        format!("{}:{}", v.name, v.ty)
                    }
                })
                .collect();
            let _ = write!(out, "{}:{}{{{}}}", node.id, cfg.kind, vars.join(", "));
        }
        NodeKind::Ext(ext) => {
            let _ = write!(out, "#EXT:{} = \"{}\"", ext.alias, ext.raw_spec);
        }
        NodeKind::Srv(srv) => {
            let props: Vec<String> = srv
                .props
                .iter()
                .map(|p| format!("{}:{}", p.key, format_prop_value(&p.value)))
                .collect();
            let _ = write!(out, "{}:{}[{}]", node.id, srv.protocol, props.join(", "));
        }
        NodeKind::Mw(mw) => {
            let _ = writeln!(out, "{}:{}({}) {{", node.id, mw.kind, mw.ctx_param);
            for stmt in &mw.body {
                out.push_str("  ");
                format_stmt(stmt, out);
                out.push('\n');
            }
            out.push('}');
        }
        NodeKind::Model(m) => {
            let fields: Vec<String> = m
                .fields
                .iter()
                .map(|f| format!("{}:{}", f.name, f.ty))
                .collect();
            let _ = write!(out, "{}:{}{{{}}}", node.id, m.name, fields.join(", "));
        }
        NodeKind::Api(api) => {
            let method = match api.method {
                HttpMethod::Get => "GET",
                HttpMethod::Post => "POST",
                HttpMethod::Put => "PUT",
                HttpMethod::Patch => "PATCH",
                HttpMethod::Delete => "DELETE",
            };
            let mut bracket_items = vec![escape_str(&api.path)];
            for mw in &api.middlewares {
                bracket_items.push(mw.to_string());
            }
            let _ = write!(
                out,
                "{}:{}.{}[{}]",
                node.id,
                api.srv_ref,
                method,
                bracket_items.join(", ")
            );
            if !api.input_fields.is_empty() {
                let ins: Vec<String> = api
                    .input_fields
                    .iter()
                    .map(|f| format!("{}:{}", f.name, f.ty))
                    .collect();
                let _ = write!(out, " IN{{{}}}", ins.join(", "));
            }
            if let Some(output) = &api.output {
                match output {
                    ApiOutput::ModelRef(id) => {
                        let _ = write!(out, " OUT{{{id}}}");
                    }
                    ApiOutput::Fields(fields) => {
                        let fs: Vec<String> = fields
                            .iter()
                            .map(|f| format!("{}:{}", f.name, f.ty))
                            .collect();
                        let _ = write!(out, " OUT{{{}}}", fs.join(", "));
                    }
                    ApiOutput::Type(ty) => {
                        let _ = write!(out, " OUT{{{ty}}}");
                    }
                }
            }
            out.push_str(" {\n");
            for stmt in &api.body {
                out.push_str("  ");
                format_stmt(stmt, out);
                out.push('\n');
            }
            out.push('}');
        }
        NodeKind::Signal(sig) => {
            let _ = write!(out, "{}:{} = ", node.id, sig.ty);
            format_expr(&sig.init, out);
            if sig.persist {
                out.push_str(" @persist");
            }
        }
        NodeKind::Ui(ui) => {
            let args: Vec<String> = ui.args.iter().map(format_ui_arg).collect();
            let _ = write!(out, "{}:{}[{}]", node.id, ui.widget, args.join(","));
            match &ui.body {
                UiBody::Empty => out.push_str("{}"),
                UiBody::Children(ch) => {
                    let list: Vec<&str> = ch.iter().map(|c| c.as_str()).collect();
                    let _ = write!(out, "{{{}}}", list.join(","));
                }
                UiBody::Text { template, .. } => {
                    let _ = write!(out, "{{{}}}", escape_str(template));
                }
                UiBody::Options(opts) => {
                    let quoted: Vec<String> = opts.iter().map(|s| escape_str(s)).collect();
                    let _ = write!(out, "{{{}}}", quoted.join(", "));
                }
            }
            for ev in &ui.events {
                out.push(' ');
                format_event_handler(ev, out);
            }
        }
    }
}

fn format_prop_value(v: &PropValue) -> String {
    match v {
        PropValue::Int(n) => n.to_string(),
        PropValue::Float(f) => format_float(*f),
        PropValue::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        PropValue::Str(s) => escape_str(s),
        PropValue::Ident(s) => s.clone(),
    }
}

fn format_ui_arg(a: &UiArg) -> String {
    match a {
        UiArg::Int(n) => n.to_string(),
        UiArg::Float(f) => format_float(*f),
        UiArg::Percent(p) => {
            if (*p - p.round()).abs() < f64::EPSILON {
                format!("{}%", *p as i64)
            } else {
                format!("{p}%")
            }
        }
        UiArg::HexColor(c) => c.clone(),
        UiArg::Ident(s) => s.clone(),
        UiArg::Str(s) => escape_str(s),
        UiArg::NodeRef(id) => id.to_string(),
    }
}

fn format_event_handler(ev: &EventHandler, out: &mut String) {
    let _ = write!(out, "@{}", ev.event);
    if let Some(g) = &ev.guard {
        out.push_str(" if ");
        format_expr(g, out);
        out.push(' ');
    }
    out.push_str("->");
    match &ev.action {
        EventAction::Call { api, payload, bind } => {
            let _ = write!(out, "CALL({api},");
            format_expr(payload, out);
            out.push(')');
            if let Some(assign) = bind {
                let _ = write!(out, "=>{signal}=", signal = assign.signal);
                format_expr(&assign.value, out);
            }
        }
        EventAction::Toast { message } => {
            out.push_str("TOAST(");
            format_expr(message, out);
            out.push(')');
        }
        EventAction::Set { signal, value } => {
            let _ = write!(out, "{signal}=");
            format_expr(value, out);
        }
        EventAction::Generic { name, args } => {
            let _ = write!(out, "{name}(");
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                format_expr(a, out);
            }
            out.push(')');
        }
    }
}

fn format_stmt(stmt: &Stmt, out: &mut String) {
    match stmt {
        Stmt::Let {
            name,
            ty,
            value,
            fallback_err,
            ..
        } => {
            let _ = write!(out, "let {name}");
            if let Some(t) = ty {
                let _ = write!(out, ":{t}");
            }
            out.push_str(" = ");
            format_expr(value, out);
            if let Some(err) = fallback_err {
                out.push_str(" ? ");
                format_err_action(err, out);
            }
        }
        Stmt::Expr {
            expr,
            fallback_err,
            flow,
            ..
        } => {
            format_expr(expr, out);
            if let Some(err) = fallback_err {
                out.push_str(" ? ");
                format_err_action(err, out);
            }
            if let Some(fl) = flow {
                out.push_str(" -> ");
                match fl {
                    FlowControl::Next => out.push_str("NEXT"),
                    FlowControl::Ret => out.push_str("RET"),
                    FlowControl::Err(err) => format_err_action(err, out),
                }
            }
        }
    }
}

fn format_err_action(err: &ErrAction, out: &mut String) {
    let _ = write!(out, "ERR({}, ", err.status);
    format_expr(&err.message, out);
    out.push(')');
}

fn format_expr(expr: &Expr, out: &mut String) {
    match &expr.kind {
        ExprKind::Int(n) => {
            let _ = write!(out, "{n}");
        }
        ExprKind::Float(f) => {
            out.push_str(&format_float(*f));
        }
        ExprKind::Str(s) => {
            out.push_str(&escape_str(s));
        }
        ExprKind::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        ExprKind::Null => out.push_str("null"),
        ExprKind::Ident(s) => out.push_str(s),
        ExprKind::NodeRef(id) => out.push_str(id.as_str()),
        ExprKind::Member { object, field } => {
            format_expr(object, out);
            out.push('.');
            out.push_str(field);
        }
        ExprKind::Call { callee, args } => {
            format_expr(callee, out);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(a, out);
            }
            out.push(')');
        }
        ExprKind::Object(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if is_ident_or_dollar(k) {
                    out.push_str(k);
                } else {
                    out.push_str(&escape_str(k));
                }
                out.push(':');
                format_expr(v, out);
            }
            out.push('}');
        }
        ExprKind::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(item, out);
            }
            out.push(']');
        }
        ExprKind::Binary { op, left, right } => {
            let op_str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "!=",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
            };
            format_expr(left, out);
            let _ = write!(out, " {op_str} ");
            format_expr(right, out);
        }
    }
}

fn format_float(f: f64) -> String {
    let s = f.to_string();
    if s.contains('.') {
        s
    } else {
        format!("{s}.0")
    }
}

fn is_ident_or_dollar(s: &str) -> bool {
    if let Some(rest) = s.strip_prefix('$') {
        !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    } else {
        let mut chars = s.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() || first == '_' => {
                chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        }
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
