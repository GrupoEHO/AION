use std::collections::{BTreeMap, HashMap};

use aion_ast::{
    BinaryOp, ErrAction, Expr, ExprKind, ExtNode, FlowControl, NodeId, Program, Stmt,
};
use aion_ext::{ExtRegistry, ExtValue};

use crate::{RuntimeConfig, RuntimeNetError};

/// Per-request execution context passed through `#MW` middlewares and `#API` handlers.
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    pub headers: HashMap<String, String>,
    pub vars: BTreeMap<String, ExtValue>,
}

impl RequestContext {
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Self {
        let mut map = HashMap::new();
        for (name, val) in headers {
            if let Ok(s) = val.to_str() {
                map.insert(name.as_str().to_ascii_lowercase(), s.to_string());
            }
        }
        Self {
            headers: map,
            vars: BTreeMap::new(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvalOutcome {
    Next,
    Ret(ExtValue),
    Err { status: u16, message: String },
}

/// Evaluates a `#MW` middleware body.
pub fn eval_middleware(
    program: &Program,
    registry: &ExtRegistry,
    config: &RuntimeConfig,
    ctx_param: &str,
    body: &[Stmt],
    ctx: &mut RequestContext,
) -> Result<EvalOutcome, RuntimeNetError> {
    let ext_map = collect_ext_map(program);
    let mut locals: HashMap<String, ExtValue> = HashMap::new();

    for stmt in body {
        if let Some(outcome) = eval_stmt(
            registry,
            config,
            &ext_map,
            ctx_param,
            ctx,
            None,
            &mut locals,
            stmt,
        )? {
            return Ok(outcome);
        }
    }
    Ok(EvalOutcome::Next)
}

/// Evaluates an `#API` handler body with decoded `IN` payload.
pub fn eval_api(
    program: &Program,
    registry: &ExtRegistry,
    config: &RuntimeConfig,
    body: &[Stmt],
    ctx: &mut RequestContext,
    input: &BTreeMap<String, ExtValue>,
) -> Result<EvalOutcome, RuntimeNetError> {
    let ext_map = collect_ext_map(program);
    let mut locals: HashMap<String, ExtValue> = HashMap::new();
    let mut last_val = ExtValue::Void;

    for stmt in body {
        match stmt {
            Stmt::Let { .. } => {
                if let Some(outcome) = eval_stmt(
                    registry,
                    config,
                    &ext_map,
                    "ctx",
                    ctx,
                    Some(input),
                    &mut locals,
                    stmt,
                )? {
                    return Ok(outcome);
                }
            }
            Stmt::Expr {
                expr,
                fallback_err,
                flow,
                ..
            } => {
                match eval_expr(
                    registry,
                    config,
                    &ext_map,
                    "ctx",
                    ctx,
                    Some(input),
                    &locals,
                    expr,
                ) {
                    Ok(val) => {
                        last_val = val.clone();
                        if let Some(f) = flow {
                            match f {
                                FlowControl::Ret => return Ok(EvalOutcome::Ret(val)),
                                FlowControl::Next => return Ok(EvalOutcome::Next),
                                FlowControl::Err(err_act) => {
                                    return Ok(resolve_err_action(
                                        registry,
                                        config,
                                        &ext_map,
                                        "ctx",
                                        ctx,
                                        Some(input),
                                        &locals,
                                        err_act,
                                    )?);
                                }
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(err_act) = fallback_err {
                            return Ok(resolve_err_action(
                                registry,
                                config,
                                &ext_map,
                                "ctx",
                                ctx,
                                Some(input),
                                &locals,
                                err_act,
                            )?);
                        }
                        return Err(err);
                    }
                }
            }
        }
    }

    Ok(EvalOutcome::Ret(last_val))
}

#[allow(clippy::too_many_arguments)]
fn eval_stmt(
    registry: &ExtRegistry,
    config: &RuntimeConfig,
    ext_map: &HashMap<&str, &ExtNode>,
    ctx_param: &str,
    ctx: &mut RequestContext,
    input: Option<&BTreeMap<String, ExtValue>>,
    locals: &mut HashMap<String, ExtValue>,
    stmt: &Stmt,
) -> Result<Option<EvalOutcome>, RuntimeNetError> {
    match stmt {
        Stmt::Let {
            name,
            value,
            fallback_err,
            ..
        } => {
            match eval_expr(
                registry, config, ext_map, ctx_param, ctx, input, locals, value,
            ) {
                Ok(val) => {
                    locals.insert(name.clone(), val);
                    Ok(None)
                }
                Err(err) => {
                    if let Some(err_act) = fallback_err {
                        let outcome = resolve_err_action(
                            registry, config, ext_map, ctx_param, ctx, input, locals, err_act,
                        )?;
                        Ok(Some(outcome))
                    } else {
                        Err(err)
                    }
                }
            }
        }
        Stmt::Expr {
            expr,
            fallback_err,
            flow,
            ..
        } => {
            match eval_expr(
                registry, config, ext_map, ctx_param, ctx, input, locals, expr,
            ) {
                Ok(val) => {
                    if let Some(f) = flow {
                        match f {
                            FlowControl::Next => Ok(Some(EvalOutcome::Next)),
                            FlowControl::Ret => Ok(Some(EvalOutcome::Ret(val))),
                            FlowControl::Err(err_act) => Ok(Some(resolve_err_action(
                                registry, config, ext_map, ctx_param, ctx, input, locals, err_act,
                            )?)),
                        }
                    } else {
                        Ok(None)
                    }
                }
                Err(err) => {
                    if let Some(err_act) = fallback_err {
                        let outcome = resolve_err_action(
                            registry, config, ext_map, ctx_param, ctx, input, locals, err_act,
                        )?;
                        Ok(Some(outcome))
                    } else {
                        Err(err)
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_err_action(
    registry: &ExtRegistry,
    config: &RuntimeConfig,
    ext_map: &HashMap<&str, &ExtNode>,
    ctx_param: &str,
    ctx: &mut RequestContext,
    input: Option<&BTreeMap<String, ExtValue>>,
    locals: &HashMap<String, ExtValue>,
    err_act: &ErrAction,
) -> Result<EvalOutcome, RuntimeNetError> {
    let msg_val = eval_expr(
        registry,
        config,
        ext_map,
        ctx_param,
        ctx,
        input,
        locals,
        &err_act.message,
    )?;
    let message = ext_value_to_string(&msg_val);
    let status = u16::try_from(err_act.status).unwrap_or(500);
    Ok(EvalOutcome::Err { status, message })
}

#[allow(clippy::too_many_arguments)]
fn eval_expr(
    registry: &ExtRegistry,
    config: &RuntimeConfig,
    ext_map: &HashMap<&str, &ExtNode>,
    ctx_param: &str,
    ctx: &mut RequestContext,
    input: Option<&BTreeMap<String, ExtValue>>,
    locals: &HashMap<String, ExtValue>,
    expr: &Expr,
) -> Result<ExtValue, RuntimeNetError> {
    match &expr.kind {
        ExprKind::Int(n) => Ok(ExtValue::I32(*n as i32)),
        ExprKind::Float(f) => Ok(ExtValue::F64(*f)),
        ExprKind::Str(s) => Ok(ExtValue::Str(s.clone())),
        ExprKind::Bool(b) => Ok(ExtValue::Bool(*b)),
        ExprKind::Null => Ok(ExtValue::Void),
        ExprKind::NodeRef(NodeId(id)) => Ok(ExtValue::Str(id.clone())),
        ExprKind::Ident(name) => {
            if let Some(v) = locals.get(name) {
                Ok(v.clone())
            } else {
                Err(RuntimeNetError::Eval(format!(
                    "undefined identifier `{name}` at line {}",
                    expr.span.line
                )))
            }
        }
        ExprKind::Member { object, field } => {
            if let ExprKind::Ident(ns) = &object.kind {
                if ns == "CFG" {
                    return config
                        .get_env(field)
                        .map(ExtValue::Str)
                        .ok_or_else(|| {
                            RuntimeNetError::Eval(format!(
                                "missing config/env variable `CFG.{field}`"
                            ))
                        });
                }
                if ns == "in" {
                    let map = input.ok_or_else(|| {
                        RuntimeNetError::Eval("`in` referenced outside `#API` handler".to_string())
                    })?;
                    return Ok(map.get(field).cloned().unwrap_or(ExtValue::Void));
                }
            }

            let obj_val = eval_expr(
                registry, config, ext_map, ctx_param, ctx, input, locals, object,
            )?;
            match obj_val {
                ExtValue::Map(map) => Ok(map.get(field).cloned().unwrap_or(ExtValue::Void)),
                other => Err(RuntimeNetError::Eval(format!(
                    "cannot access field `{field}` on non-map value {other:?}"
                ))),
            }
        }
        ExprKind::Call { callee, args } => {
            let mut eval_args = Vec::with_capacity(args.len());
            for arg in args {
                eval_args.push(eval_expr(
                    registry, config, ext_map, ctx_param, ctx, input, locals, arg,
                )?);
            }

            if let ExprKind::Member { object, field } = &callee.kind {
                if let ExprKind::Ident(target) = &object.kind {
                    if target == ctx_param || target == "ctx" {
                        return eval_ctx_method(ctx, field, eval_args);
                    }
                    if let Some(ext_node) = ext_map.get(target.as_str()) {
                        return registry
                            .invoke(&ext_node.module_id, &ext_node.version_req, field, eval_args)
                            .map_err(|e| RuntimeNetError::Ext(e.to_string()));
                    }
                }
            }

            Err(RuntimeNetError::Eval(format!(
                "unsupported call expression at line {}",
                expr.span.line
            )))
        }
        ExprKind::Object(entries) => {
            let mut map = BTreeMap::new();
            for (k, v) in entries {
                let val = eval_expr(
                    registry, config, ext_map, ctx_param, ctx, input, locals, v,
                )?;
                map.insert(k.clone(), val);
            }
            Ok(ExtValue::Map(map))
        }
        ExprKind::List(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                list.push(eval_expr(
                    registry, config, ext_map, ctx_param, ctx, input, locals, item,
                )?);
            }
            Ok(ExtValue::List(list))
        }
        ExprKind::Binary { op, left, right } => {
            let l = eval_expr(
                registry, config, ext_map, ctx_param, ctx, input, locals, left,
            )?;
            let r = eval_expr(
                registry, config, ext_map, ctx_param, ctx, input, locals, right,
            )?;
            eval_binary(*op, l, r)
        }
    }
}

fn eval_ctx_method(
    ctx: &mut RequestContext,
    method: &str,
    args: Vec<ExtValue>,
) -> Result<ExtValue, RuntimeNetError> {
    match method {
        "hdr" => {
            let key = args
                .first()
                .map(ext_value_to_string)
                .unwrap_or_default();
            Ok(ExtValue::Str(
                ctx.header(&key).unwrap_or_default().to_string(),
            ))
        }
        "get" => {
            let key = args
                .first()
                .map(ext_value_to_string)
                .unwrap_or_default();
            Ok(ctx.vars.get(&key).cloned().unwrap_or(ExtValue::Void))
        }
        "set" => {
            let key = args
                .first()
                .map(ext_value_to_string)
                .unwrap_or_default();
            let val = args.get(1).cloned().unwrap_or(ExtValue::Void);
            ctx.vars.insert(key, val);
            Ok(ExtValue::Void)
        }
        other => Err(RuntimeNetError::Eval(format!(
            "unknown `ctx.{other}` method"
        ))),
    }
}

fn eval_binary(op: BinaryOp, l: ExtValue, r: ExtValue) -> Result<ExtValue, RuntimeNetError> {
    match (op, l, r) {
        (BinaryOp::Add, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::I32(a + b)),
        (BinaryOp::Sub, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::I32(a - b)),
        (BinaryOp::Mul, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::I32(a * b)),
        (BinaryOp::Div, ExtValue::I32(a), ExtValue::I32(b)) if b != 0 => {
            Ok(ExtValue::I32(a / b))
        }
        (BinaryOp::Add, ExtValue::F64(a), ExtValue::F64(b)) => Ok(ExtValue::F64(a + b)),
        (BinaryOp::Sub, ExtValue::F64(a), ExtValue::F64(b)) => Ok(ExtValue::F64(a - b)),
        (BinaryOp::Mul, ExtValue::F64(a), ExtValue::F64(b)) => Ok(ExtValue::F64(a * b)),
        (BinaryOp::Div, ExtValue::F64(a), ExtValue::F64(b)) if b != 0.0 => {
            Ok(ExtValue::F64(a / b))
        }
        (BinaryOp::Add, ExtValue::Str(a), ExtValue::Str(b)) => {
            Ok(ExtValue::Str(format!("{a}{b}")))
        }
        (BinaryOp::Eq, a, b) => Ok(ExtValue::Bool(a == b)),
        (BinaryOp::Ne, a, b) => Ok(ExtValue::Bool(a != b)),
        (BinaryOp::Lt, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::Bool(a < b)),
        (BinaryOp::Le, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::Bool(a <= b)),
        (BinaryOp::Gt, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::Bool(a > b)),
        (BinaryOp::Ge, ExtValue::I32(a), ExtValue::I32(b)) => Ok(ExtValue::Bool(a >= b)),
        (BinaryOp::And, ExtValue::Bool(a), ExtValue::Bool(b)) => Ok(ExtValue::Bool(a && b)),
        (BinaryOp::Or, ExtValue::Bool(a), ExtValue::Bool(b)) => Ok(ExtValue::Bool(a || b)),
        _ => Err(RuntimeNetError::Eval(
            "incompatible operand types for binary operator".to_string(),
        )),
    }
}

fn ext_value_to_string(v: &ExtValue) -> String {
    match v {
        ExtValue::Str(s) => s.clone(),
        ExtValue::I32(n) => n.to_string(),
        ExtValue::F64(f) => f.to_string(),
        ExtValue::Bool(b) => b.to_string(),
        ExtValue::Ts(t) => t.to_string(),
        ExtValue::Void => String::new(),
        other => format!("{other:?}"),
    }
}

fn collect_ext_map(program: &Program) -> HashMap<&str, &ExtNode> {
    let mut map = HashMap::new();
    for node in &program.nodes {
        if let aion_ast::NodeKind::Ext(ext) = &node.kind {
            map.insert(ext.alias.as_str(), ext);
        }
    }
    map
}
