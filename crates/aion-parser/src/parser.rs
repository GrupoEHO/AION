use std::ops::Range;

use aion_ast::{
    AionType, ApiNode, ApiOutput, BinaryOp, CfgNode, CfgVar, ErrAction, EventAction, EventHandler,
    Expr, ExprKind, ExtNode, FieldDef, FlatNode, FlowControl, HttpMethod, ModelNode, MwNode,
    NodeId, NodeKind, Program, PropValue, SignalAssign, SignalNode, Span, SrvNode, SrvProp, Stmt,
    UiArg, UiBody, UiNode, UseNode,
};
use chumsky::prelude::*;

use crate::lexer::Token;

fn to_span(src: &str, r: Range<usize>) -> Span {
    Span::from_range(src, r.start, r.end)
}

fn ident_parser() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    select! { Token::Ident(s) => s }.labelled("identifier")
}

fn node_tag_parser() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    select! { Token::NodeTag(s) => s }.labelled("node tag (#...)")
}

fn string_parser() -> impl Parser<Token, String, Error = Simple<Token>> + Clone {
    select! { Token::Str(s) => s }.labelled("string literal")
}

fn type_parser() -> impl Parser<Token, AionType, Error = Simple<Token>> + Clone {
    recursive(|ty| {
        let list_ty = select! { Token::Ident(s) if s == "list" => () }
            .ignore_then(just(Token::Lt))
            .ignore_then(ty.clone())
            .then_ignore(just(Token::Gt))
            .map(|inner| AionType::List(Box::new(inner)));

        let map_ty = select! { Token::Ident(s) if s == "map" => () }
            .ignore_then(just(Token::Lt))
            .ignore_then(ty.clone())
            .then_ignore(just(Token::Comma))
            .then(ty)
            .then_ignore(just(Token::Gt))
            .map(|(k, v)| AionType::Map(Box::new(k), Box::new(v)));

        let ext_ty = select! { Token::AtIdent(name) => AionType::ExtType { ext_alias: None, name } };

        let model_ref = node_tag_parser().map(|id| AionType::ModelRef(NodeId::new(id)));

        let prim_or_named = select! { Token::Ident(s) if s != "list" && s != "map" => s }.map(|s| match s.as_str() {
            "i32" => AionType::I32,
            "f64" => AionType::F64,
            "str" => AionType::Str,
            "bool" => AionType::Bool,
            "bytes" => AionType::Bytes,
            "ts" => AionType::Ts,
            "void" => AionType::Void,
            "any" => AionType::Any,
            _ => AionType::Named(s),
        });

        choice((list_ty, map_ty, ext_ty, model_ref, prim_or_named)).labelled("type")
    })
}

/// Parses `<type> =`, including `list<T>=` and `map<K,V>=` where `>=` was lexed as `Token::Ge`.
fn type_then_eq_parser() -> impl Parser<Token, AionType, Error = Simple<Token>> + Clone {
    let fused_list = select! { Token::Ident(s) if s == "list" => () }
        .ignore_then(just(Token::Lt))
        .ignore_then(type_parser())
        .then_ignore(just(Token::Ge))
        .map(|inner| AionType::List(Box::new(inner)));

    let fused_map = select! { Token::Ident(s) if s == "map" => () }
        .ignore_then(just(Token::Lt))
        .ignore_then(type_parser())
        .then_ignore(just(Token::Comma))
        .then(type_parser())
        .then_ignore(just(Token::Ge))
        .map(|(k, v)| AionType::Map(Box::new(k), Box::new(v)));

    choice((
        fused_list,
        fused_map,
        type_parser().then_ignore(just(Token::Eq)),
    ))
}

fn field_def_parser<'a>(
    src: &'a str,
) -> impl Parser<Token, FieldDef, Error = Simple<Token>> + Clone + 'a {
    ident_parser()
        .then_ignore(just(Token::Colon))
        .then(type_parser())
        .map_with_span(move |(name, ty), range| FieldDef {
            name,
            ty,
            span: to_span(src, range),
        })
        .labelled("field definition")
}

fn expr_parser<'a>(src: &'a str) -> impl Parser<Token, Expr, Error = Simple<Token>> + Clone + 'a {
    recursive(move |expr| {
        let obj_key = choice((
            ident_parser(),
            select! { Token::DollarIdent(s) => s },
            string_parser(),
        ));

        let object_lit = obj_key
            .then_ignore(just(Token::Colon))
            .then(expr.clone())
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LBrace), just(Token::RBrace))
            .map(ExprKind::Object);

        let list_lit = expr
            .clone()
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LBracket), just(Token::RBracket))
            .map(ExprKind::List);

        let neg_num = just(Token::Minus).ignore_then(choice((
            select! { Token::Int(n) => ExprKind::Int(-n) },
            select! { Token::Float(s) => s }.try_map(|s, span| {
                s.parse::<f64>()
                    .map(|v| ExprKind::Float(-v))
                    .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
            }),
        )));

        let atom = choice((
            neg_num,
            select! { Token::Int(n) => ExprKind::Int(n) },
            select! { Token::Float(s) => s }.try_map(|s, span| {
                s.parse::<f64>()
                    .map(ExprKind::Float)
                    .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
            }),
            string_parser().map(ExprKind::Str),
            select! { Token::Bool(b) => ExprKind::Bool(b) },
            just(Token::Null).to(ExprKind::Null),
            node_tag_parser().map(|tag| ExprKind::NodeRef(NodeId::new(tag))),
            ident_parser().map(ExprKind::Ident),
            object_lit,
            list_lit,
            expr.clone()
                .delimited_by(just(Token::LParen), just(Token::RParen))
                .map(|e| e.kind),
        ))
        .map_with_span(move |kind, range| Expr {
            kind,
            span: to_span(src, range),
        });

        enum Postfix {
            Member(String),
            Call(Vec<Expr>),
        }

        let member_op = just(Token::Dot)
            .ignore_then(ident_parser())
            .map(Postfix::Member);

        let call_op = expr
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .delimited_by(just(Token::LParen), just(Token::RParen))
            .map(Postfix::Call);

        let postfix = atom
            .then(
                member_op
                    .or(call_op)
                    .map_with_span(move |op, range| (op, range))
                    .repeated(),
            )
            .foldl(move |lhs, (op, op_range)| {
                let span = Span::from_range(src, lhs.span.start, op_range.end);
                let kind = match op {
                    Postfix::Member(field) => ExprKind::Member {
                        object: Box::new(lhs),
                        field,
                    },
                    Postfix::Call(args) => ExprKind::Call {
                        callee: Box::new(lhs),
                        args,
                    },
                };
                Expr { kind, span }
            });

        let product = postfix
            .clone()
            .then(
                choice((
                    just(Token::Star).to(BinaryOp::Mul),
                    just(Token::Slash).to(BinaryOp::Div),
                ))
                .then(postfix)
                .repeated(),
            )
            .foldl(move |left, (op, right)| {
                let span = Span::from_range(src, left.span.start, right.span.end);
                Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                }
            });

        let sum = product
            .clone()
            .then(
                choice((
                    just(Token::Plus).to(BinaryOp::Add),
                    just(Token::Minus).to(BinaryOp::Sub),
                ))
                .then(product)
                .repeated(),
            )
            .foldl(move |left, (op, right)| {
                let span = Span::from_range(src, left.span.start, right.span.end);
                Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                }
            });

        let cmp = sum
            .clone()
            .then(
                choice((
                    just(Token::EqEq).to(BinaryOp::Eq),
                    just(Token::NotEq).to(BinaryOp::Ne),
                    just(Token::Lt).to(BinaryOp::Lt),
                    just(Token::Le).to(BinaryOp::Le),
                    just(Token::Gt).to(BinaryOp::Gt),
                    just(Token::Ge).to(BinaryOp::Ge),
                ))
                .then(sum)
                .repeated(),
            )
            .foldl(move |left, (op, right)| {
                let span = Span::from_range(src, left.span.start, right.span.end);
                Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                }
            });

        cmp.clone()
            .then(
                choice((
                    just(Token::AndAnd).to(BinaryOp::And),
                    just(Token::OrOr).to(BinaryOp::Or),
                ))
                .then(cmp)
                .repeated(),
            )
            .foldl(move |left, (op, right)| {
                let span = Span::from_range(src, left.span.start, right.span.end);
                Expr {
                    kind: ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    span,
                }
            })
            .labelled("expression")
    })
}

fn err_action_parser<'a>(
    src: &'a str,
) -> impl Parser<Token, ErrAction, Error = Simple<Token>> + Clone + 'a {
    select! { Token::Ident(s) if s == "ERR" => () }
        .ignore_then(just(Token::LParen))
        .ignore_then(select! { Token::Int(n) => n as i32 })
        .then_ignore(just(Token::Comma))
        .then(expr_parser(src))
        .then_ignore(just(Token::RParen))
        .map_with_span(move |(status, message), range| ErrAction {
            status,
            message,
            span: to_span(src, range),
        })
        .labelled("ERR(status, message)")
}

fn stmt_parser<'a>(src: &'a str) -> impl Parser<Token, Stmt, Error = Simple<Token>> + Clone + 'a {
    let fallback = just(Token::Question)
        .ignore_then(err_action_parser(src))
        .or_not();

    let flow = just(Token::Arrow)
        .ignore_then(choice((
            select! { Token::Ident(s) if s == "NEXT" => FlowControl::Next },
            select! { Token::Ident(s) if s == "RET" => FlowControl::Ret },
            err_action_parser(src).map(FlowControl::Err),
        )))
        .or_not();

    let let_stmt = just(Token::Let)
        .ignore_then(ident_parser())
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .then_ignore(just(Token::Eq))
        .then(expr_parser(src))
        .then(fallback.clone())
        .then_ignore(just(Token::Semi).or_not())
        .map_with_span(move |(((name, ty), value), fallback_err), range| Stmt::Let {
            name,
            ty,
            value,
            fallback_err,
            span: to_span(src, range),
        });

    let expr_stmt = expr_parser(src)
        .then(fallback)
        .then(flow)
        .then_ignore(just(Token::Semi).or_not())
        .map_with_span(move |((expr, fallback_err), flow), range| Stmt::Expr {
            expr,
            fallback_err,
            flow,
            span: to_span(src, range),
        });

    let_stmt.or(expr_stmt).labelled("statement")
}

fn extract_interpolations(template: &str) -> Vec<NodeId> {
    let mut ids = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{#") {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let tag = &after[..close];
            if tag.len() >= 2 && tag[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                ids.push(NodeId::new(tag));
            }
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    ids
}

pub fn program_parser<'a>(src: &'a str) -> impl Parser<Token, Program, Error = Simple<Token>> + 'a {
    let cfg_var = ident_parser()
        .then(just(Token::Colon).ignore_then(type_parser()).or_not())
        .map_with_span(move |(name, ty), range| CfgVar {
            name,
            ty: ty.unwrap_or(AionType::Str),
            span: to_span(src, range),
        });

    let cfg_node = select! { Token::NodeTag(t) if t == "#CFG" => t }
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(
            cfg_var
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(move |((id, kind), vars), range| FlatNode {
            id: NodeId::new(id),
            span: to_span(src, range),
            kind: NodeKind::Cfg(CfgNode { kind, vars }),
        });

    let ext_node = select! { Token::NodeTag(t) if t == "#EXT" => () }
        .ignore_then(just(Token::Colon))
        .ignore_then(ident_parser())
        .then_ignore(just(Token::Eq))
        .then(string_parser())
        .try_map(move |(alias, raw_spec), range| {
            let (module_id, version_req) = raw_spec.split_once('@').ok_or_else(|| {
                Simple::custom(
                    range.clone(),
                    format!("external module spec `{raw_spec}` must follow `org/pkg@version`"),
                )
            })?;
            Ok(FlatNode {
                id: NodeId::new(format!("#EXT:{alias}")),
                span: to_span(src, range),
                kind: NodeKind::Ext(ExtNode {
                    alias,
                    module_id: module_id.to_string(),
                    version_req: version_req.to_string(),
                    raw_spec,
                }),
            })
        });

    let prop_val = choice((
        select! { Token::Int(n) => PropValue::Int(n) },
        select! { Token::Float(s) => s }.try_map(|s, span| {
            s.parse::<f64>()
                .map(PropValue::Float)
                .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
        }),
        select! { Token::Bool(b) => PropValue::Bool(b) },
        string_parser().map(PropValue::Str),
        ident_parser().map(|s| match s.as_str() {
            "on" => PropValue::Bool(true),
            "off" => PropValue::Bool(false),
            _ => PropValue::Ident(s),
        }),
    ));

    let srv_prop = ident_parser()
        .then_ignore(just(Token::Colon))
        .then(prop_val)
        .map_with_span(move |(key, value), range| SrvProp {
            key,
            value,
            span: to_span(src, range),
        });

    let srv_node = select! { Token::NodeTag(t) if t.starts_with("#SRV") => t }
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(
            srv_prop
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBracket), just(Token::RBracket)),
        )
        .map_with_span(move |((id, protocol), props), range| FlatNode {
            id: NodeId::new(id),
            span: to_span(src, range),
            kind: NodeKind::Srv(SrvNode { protocol, props }),
        });

    let mw_node = select! { Token::NodeTag(t) if t.starts_with("#MW") => t }
        .then_ignore(just(Token::Colon))
        .then(ident_parser())
        .then(ident_parser().delimited_by(just(Token::LParen), just(Token::RParen)))
        .then(
            stmt_parser(src)
                .repeated()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(move |(((id, kind), ctx_param), body), range| FlatNode {
            id: NodeId::new(id),
            span: to_span(src, range),
            kind: NodeKind::Mw(MwNode {
                kind,
                ctx_param,
                body,
            }),
        });

    let model_node =
        select! { Token::NodeTag(t) if t.starts_with("#M") && !t.starts_with("#MW") => t }
            .then_ignore(just(Token::Colon))
            .then(ident_parser())
            .then(
                field_def_parser(src)
                    .separated_by(just(Token::Comma))
                    .allow_trailing()
                    .delimited_by(just(Token::LBrace), just(Token::RBrace)),
            )
            .map_with_span(move |((id, name), fields), range| FlatNode {
                id: NodeId::new(id),
                span: to_span(src, range),
                kind: NodeKind::Model(ModelNode { name, fields }),
            });

    let api_in = select! { Token::Ident(s) if s == "IN" => () }
        .ignore_then(
            field_def_parser(src)
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .or_not();

    let api_out = select! { Token::Ident(s) if s == "OUT" => () }
        .ignore_then(
            choice((
                node_tag_parser().map(|id| ApiOutput::ModelRef(NodeId::new(id))),
                field_def_parser(src)
                    .separated_by(just(Token::Comma))
                    .at_least(1)
                    .allow_trailing()
                    .map(ApiOutput::Fields),
                type_parser().map(ApiOutput::Type),
            ))
            .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .or_not();

    let api_node = select! { Token::NodeTag(t) if t.starts_with("#API") => t }
        .then_ignore(just(Token::Colon))
        .then(node_tag_parser())
        .then_ignore(just(Token::Dot))
        .then(ident_parser().try_map(|m, span| {
            HttpMethod::from_ident(&m).ok_or_else(|| {
                Simple::custom(
                    span,
                    format!("unsupported HTTP method `{m}` (expected GET, POST, PUT, PATCH, DELETE)"),
                )
            })
        }))
        .then(
            string_parser()
                .then(just(Token::Comma).ignore_then(node_tag_parser()).repeated())
                .delimited_by(just(Token::LBracket), just(Token::RBracket)),
        )
        .then(api_in)
        .then(api_out)
        .then(
            stmt_parser(src)
                .repeated()
                .delimited_by(just(Token::LBrace), just(Token::RBrace)),
        )
        .map_with_span(
            move |((((((id, srv_ref), method), (path, mws)), input_fields), output), body), range| {
                FlatNode {
                    id: NodeId::new(id),
                    span: to_span(src, range),
                    kind: NodeKind::Api(ApiNode {
                        srv_ref: NodeId::new(srv_ref),
                        method,
                        path,
                        middlewares: mws.into_iter().map(NodeId::new).collect(),
                        input_fields: input_fields.unwrap_or_default(),
                        output,
                        body,
                    }),
                }
            },
        );

    let signal_node =
        select! { Token::NodeTag(t) if t.starts_with("#S") && !t.starts_with("#SRV") => t }
            .then_ignore(just(Token::Colon))
            .then(type_then_eq_parser())
            .then(expr_parser(src))
            .then(
                select! { Token::AtIdent(a) if a == "persist" => () }
                    .or_not()
                    .map(|o| o.is_some()),
            )
            .map_with_span(move |(((id, ty), init), persist), range| FlatNode {
                id: NodeId::new(id),
                span: to_span(src, range),
                kind: NodeKind::Signal(SignalNode { ty, init, persist }),
            });

    let neg_ui_num = just(Token::Minus).ignore_then(choice((
        select! { Token::Float(s) => s }.try_map(|s, span| {
            s.parse::<f64>()
                .map(|v| UiArg::Float(-v))
                .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
        }),
        select! { Token::Int(n) => UiArg::Int(-n) },
    )));

    let ui_arg = choice((
        neg_ui_num,
        select! { Token::Float(s) => s }
            .then_ignore(just(Token::Percent))
            .try_map(|s, span| {
                s.parse::<f64>()
                    .map(UiArg::Percent)
                    .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
            }),
        select! { Token::Int(n) => n }
            .then_ignore(just(Token::Percent))
            .map(|n| UiArg::Percent(n as f64)),
        select! { Token::Float(s) => s }.try_map(|s, span| {
            s.parse::<f64>()
                .map(UiArg::Float)
                .map_err(|e| Simple::custom(span, format!("invalid float `{s}`: {e}")))
        }),
        select! { Token::Int(n) => UiArg::Int(n) },
        select! { Token::HexColor(c) => UiArg::HexColor(c) },
        string_parser().map(UiArg::Str),
        node_tag_parser().map(|t| UiArg::NodeRef(NodeId::new(t))),
        ident_parser().map(UiArg::Ident),
    ));

    let ui_body = choice((
        string_parser()
            .separated_by(just(Token::Comma))
            .at_least(1)
            .allow_trailing()
            .map(|mut strs| {
                if strs.len() == 1 {
                    let template = strs.pop().unwrap();
                    let interpolations = extract_interpolations(&template);
                    UiBody::Text {
                        template,
                        interpolations,
                    }
                } else {
                    UiBody::Options(strs)
                }
            }),
        node_tag_parser()
            .map(NodeId::new)
            .separated_by(just(Token::Comma))
            .allow_trailing()
            .map(|children| {
                if children.is_empty() {
                    UiBody::Empty
                } else {
                    UiBody::Children(children)
                }
            }),
    ))
    .delimited_by(just(Token::LBrace), just(Token::RBrace))
    .or_not()
    .map(|opt| opt.unwrap_or(UiBody::Empty));

    let signal_assign = just(Token::FatArrow)
        .ignore_then(node_tag_parser())
        .then_ignore(just(Token::Eq))
        .then(expr_parser(src))
        .map_with_span(move |(signal, value), range| SignalAssign {
            signal: NodeId::new(signal),
            value,
            span: to_span(src, range),
        });

    let call_action = select! { Token::Ident(s) if s == "CALL" => () }
        .ignore_then(just(Token::LParen))
        .ignore_then(node_tag_parser())
        .then_ignore(just(Token::Comma))
        .then(expr_parser(src))
        .then_ignore(just(Token::RParen))
        .then(signal_assign.or_not())
        .map(|((api, payload), bind)| EventAction::Call {
            api: NodeId::new(api),
            payload,
            bind,
        });

    let toast_action = select! { Token::Ident(s) if s == "TOAST" => () }
        .ignore_then(
            expr_parser(src).delimited_by(just(Token::LParen), just(Token::RParen)),
        )
        .map(|message| EventAction::Toast { message });

    let set_action = node_tag_parser()
        .then_ignore(just(Token::Eq))
        .then(expr_parser(src))
        .map(|(signal, value)| EventAction::Set {
            signal: NodeId::new(signal),
            value,
        });

    let generic_action = ident_parser()
        .then(
            expr_parser(src)
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LParen), just(Token::RParen)),
        )
        .map(|(name, args)| EventAction::Generic { name, args });

    let event_guard = select! { Token::Ident(s) if s == "if" => () }
        .ignore_then(expr_parser(src))
        .or_not();

    let event_handler = select! { Token::AtIdent(ev) => ev }
        .then(event_guard)
        .then_ignore(just(Token::Arrow))
        .then(choice((call_action, toast_action, set_action, generic_action)))
        .map_with_span(move |((event, guard), action), range| EventHandler {
            event,
            action,
            guard,
            span: to_span(src, range),
        });

    let use_node = select! { Token::NodeTag(t) if t == "#USE" => () }
        .ignore_then(string_parser())
        .map_with_span(move |path, range| FlatNode {
            id: NodeId::new(format!("#USE:{path}")),
            span: to_span(src, range),
            kind: NodeKind::Use(UseNode { path }),
        });

    let widget_name = ident_parser()
        .then(just(Token::Dot).ignore_then(ident_parser()).or_not())
        .map(|(a, b)| match b {
            Some(w) => format!("{a}.{w}"),
            None => a,
        });

    let ui_node = select! { Token::NodeTag(t) if t.starts_with("#N") => t }
        .then_ignore(just(Token::Colon))
        .then(widget_name)
        .then(
            ui_arg
                .separated_by(just(Token::Comma))
                .allow_trailing()
                .delimited_by(just(Token::LBracket), just(Token::RBracket)),
        )
        .then(ui_body)
        .then(event_handler.repeated())
        .map_with_span(
            move |((((id, widget), args), body), events), range| FlatNode {
                id: NodeId::new(id),
                span: to_span(src, range),
                kind: NodeKind::Ui(UiNode {
                    widget,
                    args,
                    body,
                    events,
                }),
            },
        );

    let top_node = choice((
        cfg_node,
        ext_node,
        use_node,
        srv_node,
        mw_node,
        api_node,
        model_node,
        signal_node,
        ui_node,
    ));

    top_node.repeated().then_ignore(end()).map(Program::new)
}
