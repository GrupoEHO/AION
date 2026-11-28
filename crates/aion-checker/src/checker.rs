use std::collections::{BTreeMap, HashMap, HashSet};

use aion_ast::{
    AionType, ApiNode, ApiOutput, BinaryOp, Diagnostic, DiagnosticReport, EventAction, Expr,
    ExprKind, ExtNode, FlowControl, ModelNode, MwNode, NodeId, NodeKind, NodePrefix, Phase,
    Program, SignalNode, Span, SrvNode, Stmt, UiArg, UiBody, UiNode,
};
use aion_ext::{ExtManifest, ExtRegistry};

/// Checks a parsed AION [`Program`] against type rules, cycle constraints, `#EXT` ABI manifests,
/// and mandatory `@err` handlers on `#API` calls.
pub fn check_program(program: &Program, registry: &ExtRegistry) -> DiagnosticReport {
    let mut checker = Checker::new(program, registry);
    checker.run();
    DiagnosticReport::from_diagnostics(checker.diagnostics)
}

/// Checks a parsed [`Program`] and returns `Ok(())` or a JSON-formatted error report string.
pub fn check_to_json_report(program: &Program, registry: &ExtRegistry) -> Result<(), String> {
    let report = check_program(program, registry);
    if report.ok {
        Ok(())
    } else {
        Err(report
            .to_json_pretty()
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{e}\"}}")))
    }
}

struct Checker<'a> {
    program: &'a Program,
    registry: &'a ExtRegistry,
    diagnostics: Vec<Diagnostic>,
    cfg_vars: HashMap<String, AionType>,
    ext_nodes: HashMap<String, (&'a ExtNode, Option<&'a ExtManifest>, Span)>,
    srvs: HashMap<NodeId, (&'a SrvNode, Span)>,
    mws: HashMap<NodeId, (&'a MwNode, Span)>,
    models: HashMap<NodeId, (&'a ModelNode, Span)>,
    apis: HashMap<NodeId, (&'a ApiNode, Span)>,
    signals: HashMap<NodeId, (&'a SignalNode, Span)>,
    uis: HashMap<NodeId, (&'a UiNode, Span)>,
}

impl<'a> Checker<'a> {
    fn new(program: &'a Program, registry: &'a ExtRegistry) -> Self {
        Self {
            program,
            registry,
            diagnostics: Vec::new(),
            cfg_vars: HashMap::new(),
            ext_nodes: HashMap::new(),
            srvs: HashMap::new(),
            mws: HashMap::new(),
            models: HashMap::new(),
            apis: HashMap::new(),
            signals: HashMap::new(),
            uis: HashMap::new(),
        }
    }

    fn run(&mut self) {
        self.collect_symbols();
        self.check_circular_ui_refs();
        self.check_circular_model_refs();
        self.check_signals();
        self.check_middlewares();
        self.check_apis();
        self.check_ui_nodes();
        self.check_ui_layout();
    }

    fn collect_symbols(&mut self) {
        let mut seen_ids: HashSet<NodeId> = HashSet::new();

        for node in &self.program.nodes {
            if !seen_ids.insert(node.id.clone()) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "E_DUPLICATE_NODE",
                        Phase::Checker,
                        format!("duplicate node id `{}`", node.id),
                        node.span,
                    )
                    .with_node_id(node.id.as_str())
                    .with_fix_hint(format!(
                        "Rename or remove the duplicate declaration of `{}`.",
                        node.id
                    )),
                );
                continue;
            }

            match &node.kind {
                NodeKind::Cfg(cfg) => {
                    for var in &cfg.vars {
                        self.cfg_vars.insert(var.name.clone(), var.ty.clone());
                    }
                }
                NodeKind::Ext(ext) => {
                    let manifest = self.registry.resolve(&ext.module_id, &ext.version_req);
                    if manifest.is_none() {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_EXT_NOT_FOUND",
                                Phase::Ext,
                                format!(
                                    "external module `{}@{}` (alias `{}`) was not found in registry",
                                    ext.module_id, ext.version_req, ext.alias
                                ),
                                node.span,
                            )
                            .with_node_id(node.id.as_str())
                            .with_found(ext.raw_spec.clone())
                            .with_fix_hint(format!(
                                "Register an ABI manifest for `{}@{}` or update `#EXT:{}` to an available version.",
                                ext.module_id, ext.version_req, ext.alias
                            )),
                        );
                    }
                    self.ext_nodes
                        .insert(ext.alias.clone(), (ext, manifest, node.span));
                }
                NodeKind::Use(_) => {}
                NodeKind::Srv(srv) => {
                    self.srvs.insert(node.id.clone(), (srv, node.span));
                }
                NodeKind::Mw(mw) => {
                    self.mws.insert(node.id.clone(), (mw, node.span));
                }
                NodeKind::Model(m) => {
                    self.models.insert(node.id.clone(), (m, node.span));
                }
                NodeKind::Api(api) => {
                    self.apis.insert(node.id.clone(), (api, node.span));
                }
                NodeKind::Signal(sig) => {
                    self.signals.insert(node.id.clone(), (sig, node.span));
                }
                NodeKind::Ui(ui) => {
                    self.uis.insert(node.id.clone(), (ui, node.span));
                }
            }
        }
    }

    fn check_circular_ui_refs(&mut self) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Color {
            Visiting,
            Visited,
        }

        let mut colors: HashMap<NodeId, Color> = HashMap::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let ui_ids: Vec<NodeId> = self.uis.keys().cloned().collect();

        fn dfs(
            id: &NodeId,
            uis: &HashMap<NodeId, (&UiNode, Span)>,
            colors: &mut HashMap<NodeId, Color>,
            stack: &mut Vec<NodeId>,
            diagnostics: &mut Vec<Diagnostic>,
        ) {
            if matches!(colors.get(id), Some(Color::Visited)) {
                return;
            }
            colors.insert(id.clone(), Color::Visiting);
            stack.push(id.clone());

            if let Some((ui, span)) = uis.get(id) {
                if let UiBody::Children(children) = &ui.body {
                    for child in children {
                        if matches!(colors.get(child), Some(Color::Visiting)) {
                            let start_idx =
                                stack.iter().position(|x| x == child).unwrap_or(0);
                            let mut cycle: Vec<String> = stack[start_idx..]
                                .iter()
                                .map(|n| n.as_str().to_string())
                                .collect();
                            cycle.push(child.as_str().to_string());
                            diagnostics.push(
                                Diagnostic::error(
                                    "E_CIRCULAR_REF",
                                    Phase::Checker,
                                    format!(
                                        "circular UI reference detected: {}",
                                        cycle.join(" -> ")
                                    ),
                                    *span,
                                )
                                .with_node_id(id.as_str())
                                .with_fix_hint(format!(
                                    "Remove `{child}` from `{id}` children to break the cycle."
                                )),
                            );
                        } else if !colors.contains_key(child) && uis.contains_key(child) {
                            dfs(child, uis, colors, stack, diagnostics);
                        }
                    }
                }
            }

            stack.pop();
            colors.insert(id.clone(), Color::Visited);
        }

        for id in ui_ids {
            if !colors.contains_key(&id) {
                dfs(&id, &self.uis, &mut colors, &mut stack, &mut self.diagnostics);
            }
        }
    }

    fn check_circular_model_refs(&mut self) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Color {
            Visiting,
            Visited,
        }

        fn collect_model_refs(ty: &AionType, out: &mut Vec<NodeId>) {
            match ty {
                AionType::ModelRef(id) => out.push(id.clone()),
                AionType::List(inner) => collect_model_refs(inner, out),
                AionType::Map(k, v) => {
                    collect_model_refs(k, out);
                    collect_model_refs(v, out);
                }
                _ => {}
            }
        }

        let mut colors: HashMap<NodeId, Color> = HashMap::new();
        let mut stack: Vec<NodeId> = Vec::new();
        let model_ids: Vec<NodeId> = self.models.keys().cloned().collect();

        fn dfs(
            id: &NodeId,
            models: &HashMap<NodeId, (&ModelNode, Span)>,
            colors: &mut HashMap<NodeId, Color>,
            stack: &mut Vec<NodeId>,
            diagnostics: &mut Vec<Diagnostic>,
        ) {
            if matches!(colors.get(id), Some(Color::Visited)) {
                return;
            }
            colors.insert(id.clone(), Color::Visiting);
            stack.push(id.clone());

            if let Some((model, span)) = models.get(id) {
                for field in &model.fields {
                    let mut refs = Vec::new();
                    collect_model_refs(&field.ty, &mut refs);
                    for dep in refs {
                        if matches!(colors.get(&dep), Some(Color::Visiting)) {
                            let start_idx =
                                stack.iter().position(|x| x == &dep).unwrap_or(0);
                            let mut cycle: Vec<String> = stack[start_idx..]
                                .iter()
                                .map(|n| n.as_str().to_string())
                                .collect();
                            cycle.push(dep.as_str().to_string());
                            diagnostics.push(
                                Diagnostic::error(
                                    "E_CIRCULAR_REF",
                                    Phase::Checker,
                                    format!(
                                        "circular model reference detected: {}",
                                        cycle.join(" -> ")
                                    ),
                                    field.span,
                                )
                                .with_node_id(id.as_str())
                                .with_fix_hint(format!(
                                    "Break the cyclic reference from `{id}.{}` to `{dep}`.",
                                    field.name
                                )),
                            );
                        } else if !colors.contains_key(&dep) {
                            if models.contains_key(&dep) {
                                dfs(&dep, models, colors, stack, diagnostics);
                            } else {
                                diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_NODE",
                                        Phase::Checker,
                                        format!("model `{id}` field `{}` references unknown model `{dep}`", field.name),
                                        *span,
                                    )
                                    .with_node_id(id.as_str())
                                    .with_fix_hint(format!("Declare model `{dep}` or update field `{}` type.", field.name)),
                                );
                            }
                        }
                    }
                }
            }

            stack.pop();
            colors.insert(id.clone(), Color::Visited);
        }

        for id in model_ids {
            if !colors.contains_key(&id) {
                dfs(&id, &self.models, &mut colors, &mut stack, &mut self.diagnostics);
            }
        }
    }

    fn check_signals(&mut self) {
        for node in &self.program.nodes {
            if let NodeKind::Signal(sig) = &node.kind {
                let locals = HashMap::new();
                if !self.check_expr_compatible(node.id.as_str(), &sig.init, &sig.ty, &locals) {
                    let init_ty = self.eval_expr(node.id.as_str(), &sig.init, &locals, None);
                    if init_ty != AionType::Unknown && !init_ty.is_assignable_to(&sig.ty) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_TYPE_MISMATCH",
                                Phase::Checker,
                                format!(
                                    "signal `{}` expects initializer of type `{}`, found `{}`",
                                    node.id, sig.ty, init_ty
                                ),
                                sig.init.span,
                            )
                            .with_node_id(node.id.as_str())
                            .with_expected(sig.ty.to_string())
                            .with_found(init_ty.to_string())
                            .with_fix_hint(format!(
                                "Change `{}` initializer to match `{}` or update the signal type.",
                                node.id, sig.ty
                            )),
                        );
                    }
                }
            }
        }
    }

    /// Validates `expr` against `expected` including structural object-literal matching against `#M` models.
    /// Returns `true` if `expr` was structurally validated (either valid or already emitted specific field errors).
    fn check_expr_compatible(
        &mut self,
        node_id: &str,
        expr: &Expr,
        expected: &AionType,
        locals: &HashMap<String, AionType>,
    ) -> bool {
        match (&expr.kind, expected) {
            (ExprKind::List(items), AionType::List(elem_expected)) => {
                if items.is_empty() {
                    return true;
                }
                for item in items {
                    if !self.check_expr_compatible(node_id, item, elem_expected, locals) {
                        let actual = self.eval_expr(node_id, item, locals, None);
                        if actual != AionType::Unknown && !actual.is_assignable_to(elem_expected) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_TYPE_MISMATCH",
                                    Phase::Checker,
                                    format!(
                                        "list element in `{node_id}` expects `{elem_expected}`, found `{actual}`"
                                    ),
                                    item.span,
                                )
                                .with_node_id(node_id)
                                .with_expected(elem_expected.to_string())
                                .with_found(actual.to_string()),
                            );
                        }
                    }
                }
                true
            }
            (ExprKind::Object(entries), AionType::ModelRef(model_id)) => {
                let Some((model, _)) = self.models.get(model_id).copied() else {
                    return false;
                };
                let known: Vec<&str> = model.fields.iter().map(|f| f.name.as_str()).collect();
                for (k, v) in entries {
                    if let Some(fdef) = model.fields.iter().find(|f| &f.name == k) {
                        if !self.check_expr_compatible(node_id, v, &fdef.ty, locals) {
                            let actual = self.eval_expr(node_id, v, locals, None);
                            if actual != AionType::Unknown && !actual.is_assignable_to(&fdef.ty) {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_TYPE_MISMATCH",
                                        Phase::Checker,
                                        format!(
                                            "field `{k}` on model `{}` ({model_id}) expects `{}`, found `{actual}`",
                                            model.name, fdef.ty
                                        ),
                                        v.span,
                                    )
                                    .with_node_id(node_id)
                                    .with_expected(fdef.ty.to_string())
                                    .with_found(actual.to_string()),
                                );
                            }
                        }
                    } else {
                        let _ = self.eval_expr(node_id, v, locals, None);
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_FIELD",
                                Phase::Checker,
                                format!(
                                    "object literal for model `{}` ({model_id}) has unknown field `{k}`",
                                    model.name
                                ),
                                v.span,
                            )
                            .with_node_id(node_id)
                            .with_expected(known.join(", "))
                            .with_found(k.clone())
                            .with_fix_hint(format!(
                                "Valid fields on `{}` ({model_id}): {}.",
                                model.name,
                                known.join(", ")
                            )),
                        );
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn check_middlewares(&mut self) {
        for node in &self.program.nodes {
            if let NodeKind::Mw(mw) = &node.kind {
                let mut locals = HashMap::new();
                locals.insert(mw.ctx_param.clone(), AionType::Named("__ctx__".to_string()));
                for stmt in &mw.body {
                    self.check_stmt(node.id.as_str(), stmt, &mut locals, None);
                }
                let has_flow = mw.body.iter().any(|s| match s {
                    Stmt::Expr { flow: Some(_), .. } => true,
                    Stmt::Expr {
                        fallback_err: Some(_),
                        ..
                    }
                    | Stmt::Let {
                        fallback_err: Some(_),
                        ..
                    } => false,
                    _ => false,
                });
                if !has_flow {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_MW_MISSING_FLOW",
                            Phase::Checker,
                            format!(
                                "middleware `{}` must terminate with `-> NEXT` or `-> ERR(...)`",
                                node.id
                            ),
                            node.span,
                        )
                        .with_node_id(node.id.as_str())
                        .with_expected("-> NEXT | -> ERR(status, msg)")
                        .with_fix_hint(format!(
                            "Append `-> NEXT` to the final statement of `{}`.",
                            node.id
                        )),
                    );
                }
            }
        }
    }

    fn check_apis(&mut self) {
        for node in &self.program.nodes {
            if let NodeKind::Api(api) = &node.kind {
                if !self.srvs.contains_key(&api.srv_ref) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_UNKNOWN_NODE",
                            Phase::Checker,
                            format!(
                                "endpoint `{}` references undefined server `{}`",
                                node.id, api.srv_ref
                            ),
                            node.span,
                        )
                        .with_node_id(node.id.as_str())
                        .with_fix_hint(format!("Declare `{}` (e.g. `{}:HTTP[port:8080]`).", api.srv_ref, api.srv_ref)),
                    );
                }

                for mw_id in &api.middlewares {
                    if !self.mws.contains_key(mw_id) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_NODE",
                                Phase::Checker,
                                format!(
                                    "endpoint `{}` references undefined middleware `{}`",
                                    node.id, mw_id
                                ),
                                node.span,
                            )
                            .with_node_id(node.id.as_str())
                            .with_fix_hint(format!("Declare middleware `{mw_id}` or remove it from `{}`.", node.id)),
                        );
                    }
                }

                if let Some(ApiOutput::ModelRef(model_id)) = &api.output {
                    if !self.models.contains_key(model_id) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_NODE",
                                Phase::Checker,
                                format!(
                                    "endpoint `{}` OUT references undefined model `{}`",
                                    node.id, model_id
                                ),
                                node.span,
                            )
                            .with_node_id(node.id.as_str())
                            .with_fix_hint(format!("Declare model `{model_id}` before referencing it in `OUT{{{model_id}}}`.")),
                        );
                    }
                }

                let mut locals = HashMap::new();
                locals.insert("ctx".to_string(), AionType::Named("__ctx__".to_string()));
                locals.insert("in".to_string(), AionType::Named("__api_in__".to_string()));
                for stmt in &api.body {
                    self.check_stmt(node.id.as_str(), stmt, &mut locals, Some(api));
                }
            }
        }
    }

    fn collect_list_item_scopes(&mut self) -> HashMap<NodeId, AionType> {
        let mut item_scopes: HashMap<NodeId, AionType> = HashMap::new();

        fn propagate(
            id: &NodeId,
            elem_ty: &AionType,
            uis: &HashMap<NodeId, (&UiNode, Span)>,
            out: &mut HashMap<NodeId, AionType>,
            visited: &mut HashSet<NodeId>,
        ) {
            if !visited.insert(id.clone()) {
                return;
            }
            out.insert(id.clone(), elem_ty.clone());
            if let Some((ui, _)) = uis.get(id) {
                // Stop if nested LIST defines its own data source
                let own_list_sig = ui.widget == "LIST"
                    && ui
                        .args
                        .iter()
                        .any(|a| matches!(a, UiArg::NodeRef(r) if r.prefix() == Some(NodePrefix::Signal)));
                if !own_list_sig {
                    if let UiBody::Children(children) = &ui.body {
                        for ch in children {
                            propagate(ch, elem_ty, uis, out, visited);
                        }
                    }
                }
            }
        }

        let list_bindings: Vec<(NodeId, Span, NodeId, Vec<NodeId>)> = self
            .program
            .nodes
            .iter()
            .filter_map(|n| {
                let NodeKind::Ui(ui) = &n.kind else {
                    return None;
                };
                if ui.widget != "LIST" {
                    return None;
                }
                let sig_id = ui.args.iter().find_map(|a| match a {
                    UiArg::NodeRef(id) => Some(id.clone()),
                    _ => None,
                })?;
                let children = match &ui.body {
                    UiBody::Children(ch) => ch.clone(),
                    _ => Vec::new(),
                };
                Some((n.id.clone(), n.span, sig_id, children))
            })
            .collect();

        for (list_id, span, sig_id, children) in list_bindings {
            match self.signals.get(&sig_id).copied() {
                Some((sig, _)) => match &sig.ty {
                    AionType::List(elem_ty) => {
                        let mut visited = HashSet::new();
                        for ch in &children {
                            propagate(ch, elem_ty, &self.uis, &mut item_scopes, &mut visited);
                        }
                    }
                    other => {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_TYPE_MISMATCH",
                                Phase::Checker,
                                format!(
                                    "dynamic `LIST` `{list_id}` requires a `list<T>` signal, but `{sig_id}` has type `{other}`"
                                ),
                                span,
                            )
                            .with_node_id(list_id.as_str())
                            .with_expected("list<T>")
                            .with_found(other.to_string()),
                        );
                    }
                },
                None => {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_UNKNOWN_NODE",
                            Phase::Checker,
                            format!(
                                "dynamic `LIST` `{list_id}` references undefined signal `{sig_id}`"
                            ),
                            span,
                        )
                        .with_node_id(list_id.as_str())
                        .with_fix_hint(format!(
                            "Declare `{sig_id}:list<#M0>=[]` before referencing it in `{list_id}`."
                        )),
                    );
                }
            }
        }

        item_scopes
    }

    fn check_item_template_interpolations(
        &mut self,
        node_id: &NodeId,
        template: &str,
        item_ty: Option<&AionType>,
        span: Span,
    ) {
        let mut rest = template;
        while let Some(open) = rest.find("{item") {
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                break;
            };
            let expr_str = after[..close].trim();
            rest = &after[close + 1..];

            if expr_str != "item" && !expr_str.starts_with("item.") {
                continue;
            }
            let Some(elem_ty) = item_ty else {
                self.diagnostics.push(
                    Diagnostic::error(
                        "E_UNKNOWN_IDENT",
                        Phase::Checker,
                        format!(
                            "UI node `{node_id}` uses `{{{expr_str}}}` outside of a dynamic `LIST[..., #S_list]`"
                        ),
                        span,
                    )
                    .with_node_id(node_id.as_str())
                    .with_fix_hint(
                        "Place this node inside a `LIST[v, 8, #S_items]{...}` bound to a `list<T>` signal.",
                    ),
                );
                continue;
            };

            if let Some(field) = expr_str.strip_prefix("item.") {
                if let AionType::ModelRef(model_id) = elem_ty {
                    if let Some((model, _)) = self.models.get(model_id) {
                        if !model.fields.iter().any(|f| f.name == field) {
                            let known: Vec<&str> =
                                model.fields.iter().map(|f| f.name.as_str()).collect();
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_FIELD",
                                    Phase::Checker,
                                    format!(
                                        "UI template `{{item.{field}}}` in `{node_id}` references unknown field `{field}` on `{}` ({model_id})",
                                        model.name
                                    ),
                                    span,
                                )
                                .with_node_id(node_id.as_str())
                                .with_expected(known.join(", "))
                                .with_found(field.to_string())
                                .with_fix_hint(format!(
                                    "Available fields on `{}`: {}.",
                                    model.name,
                                    known.join(", ")
                                )),
                            );
                        }
                    }
                }
            }
        }
    }

    fn check_ui_nodes(&mut self) {
        let item_scopes = self.collect_list_item_scopes();

        for node in &self.program.nodes {
            if let NodeKind::Ui(ui) = &node.kind {
                let mut ui_locals: HashMap<String, AionType> = HashMap::new();
                if let Some(elem_ty) = item_scopes.get(&node.id) {
                    ui_locals.insert("item".to_string(), elem_ty.clone());
                    ui_locals.insert("index".to_string(), AionType::I32);
                } else if ui.widget == "CANVAS" {
                    ui_locals.insert(
                        "item".to_string(),
                        AionType::Map(Box::new(AionType::Str), Box::new(AionType::F64)),
                    );
                }

                match &ui.body {
                    UiBody::Children(children) => {
                        for child in children {
                            if !self.uis.contains_key(child) {
                                let msg = if self.program.get(child).is_some() {
                                    format!(
                                        "UI node `{}` child `{child}` is not a `#N` UI node",
                                        node.id
                                    )
                                } else {
                                    format!(
                                        "UI node `{}` references undefined child node `{child}`",
                                        node.id
                                    )
                                };
                                self.diagnostics.push(
                                    Diagnostic::error("E_UNKNOWN_NODE", Phase::Checker, msg, node.span)
                                        .with_node_id(node.id.as_str())
                                        .with_fix_hint(format!(
                                            "Declare `{child}` as a `#N` node or remove it from `{}`.",
                                            node.id
                                        )),
                                );
                            }
                        }
                    }
                    UiBody::Text {
                        template,
                        interpolations,
                    } => {
                        for sig_id in interpolations {
                            if !self.signals.contains_key(sig_id) {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_NODE",
                                        Phase::Checker,
                                        format!(
                                            "UI node `{}` text interpolation references undefined signal `{sig_id}`",
                                            node.id
                                        ),
                                        node.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_fix_hint(format!(
                                        "Declare signal `{sig_id}` (e.g. `{sig_id}:str=\"\"`)."
                                    )),
                                );
                            }
                        }
                        self.check_item_template_interpolations(
                            &node.id,
                            template,
                            item_scopes.get(&node.id),
                            node.span,
                        );
                    }
                    UiBody::Empty | UiBody::Options(_) => {}
                }

                // Validate widget is either one of the 14 frozen base primitives or a declared #EXT widget (`alias.Widget`).
                if let Some((alias, widget_name)) = ui.widget.split_once('.') {
                    match self.ext_nodes.get(alias).copied() {
                        Some((_, Some(manifest), _)) => {
                            if let Some(wspec) = manifest.widgets.get(widget_name) {
                                let min_args = wspec.args.iter().filter(|a| !a.opt).count();
                                let max_args = wspec.args.len();
                                if ui.args.len() < min_args || ui.args.len() > max_args {
                                    self.diagnostics.push(
                                        Diagnostic::error(
                                            "E_EXT_ARG_COUNT",
                                            Phase::Ext,
                                            format!(
                                                "external widget `{alias}.{widget_name}` in `{}` expects {min_args}..={max_args} args, found {}",
                                                node.id, ui.args.len()
                                            ),
                                            node.span,
                                        )
                                        .with_node_id(node.id.as_str()),
                                    );
                                }
                            } else {
                                let known: Vec<&str> = manifest.widgets.keys().map(String::as_str).collect();
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_EXT_WIDGET_NOT_FOUND",
                                        Phase::Ext,
                                        format!(
                                            "widget `{widget_name}` does not exist on external module `{}@{}` (alias `{alias}`)",
                                            manifest.id, manifest.ver
                                        ),
                                        node.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_expected(known.join(", "))
                                    .with_found(ui.widget.clone())
                                    .with_fix_hint(format!(
                                        "Use a widget declared in `{}` manifest ({}) or compose from the 14 base primitives.",
                                        manifest.id, known.join(", ")
                                    )),
                                );
                            }
                        }
                        Some((_, None, _)) => {}
                        None => {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_EXT",
                                    Phase::Ext,
                                    format!("UI node `{}` references undeclared `#EXT` alias `{alias}`", node.id),
                                    node.span,
                                )
                                .with_node_id(node.id.as_str())
                                .with_fix_hint(format!("Declare `#EXT:{alias} = \"org/pkg@1.0\"` at the top of the file.")),
                            );
                        }
                    }
                } else if !matches!(
                    ui.widget.as_str(),
                    "ROOT" | "FLEX" | "LIST" | "ROUTER" | "MODAL" | "NAV"
                        | "BTN" | "INP" | "SEL" | "SELECT" | "SW" | "SWITCH"
                        | "TXT" | "IMG" | "ICON" | "PROG" | "RING" | "CANVAS"
                ) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_UNKNOWN_WIDGET",
                            Phase::Checker,
                            format!(
                                "unknown widget `{}` in `{}` (not one of the 16 base primitives)",
                                ui.widget, node.id
                            ),
                            node.span,
                        )
                        .with_node_id(node.id.as_str())
                        .with_expected("ROOT, FLEX, LIST, ROUTER, MODAL, NAV, BTN, INP, SEL, SW, TXT, IMG, ICON, PROG, RING, CANVAS, or alias.ExtWidget")
                        .with_found(ui.widget.clone())
                        .with_fix_hint("Use one of the 16 base primitives, import a pure-AION library with `#USE`, or declare a native `#EXT` widget."),
                    );
                }

                if ui.widget == "ICON" {
                    if let UiBody::Text { template, .. } = &ui.body {
                        if !template.contains('{') {
                            let icon_ref = template.trim();
                            if let Some((alias, icon_name)) = icon_ref.split_once(':') {
                                match self.ext_nodes.get(alias) {
                                    Some((_, Some(manifest), _)) => {
                                        if !manifest.icons.contains_key(icon_name) {
                                            self.diagnostics.push(
                                                Diagnostic::error(
                                                    "E_UNKNOWN_ICON",
                                                    Phase::Ext,
                                                    format!(
                                                        "icon `{icon_name}` is not exported by `#EXT:{alias}` ({})",
                                                        manifest.id
                                                    ),
                                                    node.span,
                                                )
                                                .with_node_id(node.id.as_str())
                                                .with_found(icon_ref.to_string())
                                                .with_fix_hint(format!(
                                                    "Add `{icon_name}` to `icons` in `{}` or use an available icon.",
                                                    manifest.id
                                                )),
                                            );
                                        }
                                    }
                                    Some((_, None, _)) => {}
                                    None => {
                                        self.diagnostics.push(
                                            Diagnostic::error(
                                                "E_UNKNOWN_EXT",
                                                Phase::Ext,
                                                format!(
                                                    "ICON `{}` references undeclared `#EXT` icon pack alias `{alias}`",
                                                    node.id
                                                ),
                                                node.span,
                                            )
                                            .with_node_id(node.id.as_str())
                                            .with_fix_hint(format!(
                                                "Declare `#EXT:{alias} = \"org/icons@1.0\"` at the top of the file."
                                            )),
                                        );
                                    }
                                }
                            } else if !icon_ref.is_empty() && !aion_ext::is_std_icon(icon_ref) {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_ICON",
                                        Phase::Checker,
                                        format!(
                                            "unknown built-in icon `{icon_ref}` in `{}`",
                                            node.id
                                        ),
                                        node.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_found(icon_ref.to_string())
                                    .with_fix_hint("Use a standard icon from `aion/std` (e.g. `home`, `add`, `edit`, `trash`, `play`, `settings`) or an external pack `\"alias:name\"`."),
                                );
                            }
                        }
                    }
                }

                let bound_idx = match ui.widget.as_str() {
                    "INP" | "SEL" | "SELECT" | "SW" | "SWITCH" | "RING" => Some(4),
                    "PROG" => Some(5),
                    "CANVAS" => Some(3),
                    "MODAL" => Some(0),
                    _ => None,
                };
                if let Some(bi) = bound_idx {
                    if let Some(UiArg::NodeRef(sig_id)) = ui.args.get(bi) {
                        if !self.signals.contains_key(sig_id) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_NODE",
                                    Phase::Checker,
                                    format!(
                                        "UI widget `{}` ({}) binds to undefined signal `{sig_id}`",
                                        node.id, ui.widget
                                    ),
                                    node.span,
                                )
                                .with_node_id(node.id.as_str())
                                .with_fix_hint(format!(
                                    "Declare signal `{sig_id}` before binding it to `{}`.",
                                    node.id
                                )),
                            );
                        }
                    }
                }

                let has_err_handler = ui.events.iter().any(|h| h.event == "err");

                for handler in &ui.events {
                    if let Some(g) = &handler.guard {
                        let _ = self.eval_expr(node.id.as_str(), g, &ui_locals, None);
                    }
                    match &handler.action {
                        EventAction::Call { api, payload, bind } => {
                            if !has_err_handler {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_MISSING_ERR_HANDLER",
                                        Phase::Checker,
                                        format!(
                                            "UI node `{}` invokes API `{api}` in `@{}` without a mandatory `@err` handler",
                                            node.id, handler.event
                                        ),
                                        handler.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_expected("@err->TOAST(\"...\")")
                                    .with_found(format!("@{}->CALL({api}, ...)", handler.event))
                                    .with_fix_hint(format!(
                                        "Append `@err->TOAST(\"Error\")` to node `{}`.",
                                        node.id
                                    )),
                                );
                            }

                            if let Some((api_node, _)) = self.apis.get(api).copied() {
                                self.check_api_call_payload(
                                    node.id.as_str(),
                                    api,
                                    api_node,
                                    payload,
                                    &ui_locals,
                                );
                                if let Some(assign) = bind {
                                    self.check_api_bind(
                                        node.id.as_str(),
                                        api_node,
                                        &assign.signal,
                                        &assign.value,
                                        assign.span,
                                        &ui_locals,
                                    );
                                }
                            } else {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_NODE",
                                        Phase::Checker,
                                        format!(
                                            "UI node `{}` calls undefined API `{api}`",
                                            node.id
                                        ),
                                        handler.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_fix_hint(format!(
                                        "Declare `{api}` as an `#API` node or reference an existing endpoint."
                                    )),
                                );
                            }
                        }
                        EventAction::Set { signal, value } => {
                            if let Some((sig, _)) = self.signals.get(signal).copied() {
                                if !self.check_expr_compatible(
                                    node.id.as_str(),
                                    value,
                                    &sig.ty,
                                    &ui_locals,
                                ) {
                                    let val_ty = self.eval_expr(
                                        node.id.as_str(),
                                        value,
                                        &ui_locals,
                                        None,
                                    );
                                    if val_ty != AionType::Unknown
                                        && !val_ty.is_assignable_to(&sig.ty)
                                    {
                                        self.diagnostics.push(
                                            Diagnostic::error(
                                                "E_TYPE_MISMATCH",
                                                Phase::Checker,
                                                format!(
                                                    "cannot assign `{val_ty}` to signal `{signal}` of type `{}`",
                                                    sig.ty
                                                ),
                                                handler.span,
                                            )
                                            .with_node_id(node.id.as_str())
                                            .with_expected(sig.ty.to_string())
                                            .with_found(val_ty.to_string()),
                                        );
                                    }
                                }
                            } else {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_NODE",
                                        Phase::Checker,
                                        format!(
                                            "UI node `{}` assigns to undefined signal `{signal}`",
                                            node.id
                                        ),
                                        handler.span,
                                    )
                                    .with_node_id(node.id.as_str()),
                                );
                            }
                        }
                        EventAction::Toast { message } => {
                            let msg_ty = self.eval_expr(
                                node.id.as_str(),
                                message,
                                &ui_locals,
                                None,
                            );
                            if msg_ty != AionType::Unknown && !msg_ty.is_assignable_to(&AionType::Str)
                            {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_TYPE_MISMATCH",
                                        Phase::Checker,
                                        format!("TOAST expects `str`, found `{msg_ty}`"),
                                        message.span,
                                    )
                                    .with_node_id(node.id.as_str())
                                    .with_expected("str")
                                    .with_found(msg_ty.to_string()),
                                );
                            }
                        }
                        EventAction::Generic { name, args } => {
                            self.check_generic_ui_action(
                                node.id.as_str(),
                                name,
                                args,
                                handler.span,
                                &ui_locals,
                            );
                        }
                    }
                }
            }
        }
    }

    fn check_generic_ui_action(
        &mut self,
        node_id: &str,
        name: &str,
        args: &[Expr],
        span: Span,
        ui_locals: &HashMap<String, AionType>,
    ) {
        match name {
            "NAV" => {
                if let Some(first) = args.first() {
                    if let ExprKind::NodeRef(target) = &first.kind {
                        if !self.uis.contains_key(target) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_NODE",
                                    Phase::Checker,
                                    format!("NAV in `{node_id}` targets undefined UI screen `{target}`"),
                                    first.span,
                                )
                                .with_node_id(node_id)
                                .with_fix_hint(format!(
                                    "Declare `{target}` as a `#N` screen inside a `ROUTER`."
                                )),
                            );
                        }
                    }
                } else {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`NAV` in `{node_id}` expects 1 screen node argument"),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("NAV(#N_screen)"),
                    );
                }
            }
            "PUSH" => {
                if args.len() != 2 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`PUSH` in `{node_id}` expects 2 arguments `(#S_list, item)`"),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("PUSH(#S_list, value)"),
                    );
                    return;
                }
                if let ExprKind::NodeRef(sig_id) = &args[0].kind {
                    if let Some((sig, _)) = self.signals.get(sig_id).copied() {
                        if let AionType::List(elem_ty) = &sig.ty {
                            if !self.check_expr_compatible(node_id, &args[1], elem_ty, ui_locals) {
                                let actual = self.eval_expr(node_id, &args[1], ui_locals, None);
                                if actual != AionType::Unknown && !actual.is_assignable_to(elem_ty) {
                                    self.diagnostics.push(
                                        Diagnostic::error(
                                            "E_TYPE_MISMATCH",
                                            Phase::Checker,
                                            format!(
                                                "`PUSH({sig_id}, ...)` expects element of type `{elem_ty}`, found `{actual}`"
                                            ),
                                            args[1].span,
                                        )
                                        .with_node_id(node_id)
                                        .with_expected(elem_ty.to_string())
                                        .with_found(actual.to_string()),
                                    );
                                }
                            }
                        } else {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_TYPE_MISMATCH",
                                    Phase::Checker,
                                    format!(
                                        "`PUSH` target `{sig_id}` must be `list<T>`, found `{}`",
                                        sig.ty
                                    ),
                                    args[0].span,
                                )
                                .with_node_id(node_id)
                                .with_expected("list<T>")
                                .with_found(sig.ty.to_string()),
                            );
                        }
                    } else {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_NODE",
                                Phase::Checker,
                                format!("`PUSH` in `{node_id}` references undefined signal `{sig_id}`"),
                                args[0].span,
                            )
                            .with_node_id(node_id),
                        );
                    }
                }
            }
            "REMOVE" => {
                if args.len() != 2 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`REMOVE` in `{node_id}` expects 2 arguments `(#S_list, index_or_item)`"),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("REMOVE(#S_list, index)"),
                    );
                    return;
                }
                if let ExprKind::NodeRef(sig_id) = &args[0].kind {
                    if let Some((sig, _)) = self.signals.get(sig_id).copied() {
                        if !matches!(sig.ty, AionType::List(_)) {
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_TYPE_MISMATCH",
                                    Phase::Checker,
                                    format!(
                                        "`REMOVE` target `{sig_id}` must be `list<T>`, found `{}`",
                                        sig.ty
                                    ),
                                    args[0].span,
                                )
                                .with_node_id(node_id),
                            );
                        }
                    } else {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_NODE",
                                Phase::Checker,
                                format!("`REMOVE` in `{node_id}` references undefined signal `{sig_id}`"),
                                args[0].span,
                            )
                            .with_node_id(node_id),
                        );
                    }
                }
                let _ = self.eval_expr(node_id, &args[1], ui_locals, None);
            }
            "BEEP" => {}
            "NOTIFY" => {
                if args.len() != 2 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`NOTIFY` in `{node_id}` expects 2 arguments `(title, body)`"),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("NOTIFY(\"Title\", \"Body\")"),
                    );
                }
                for a in args {
                    let _ = self.eval_expr(node_id, a, ui_locals, None);
                }
            }
            "SETAT" => {
                if args.len() != 3 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`SETAT` in `{node_id}` expects 3 arguments `(#S_list, index, value)`"),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("SETAT(#S_list, index, value)"),
                    );
                    return;
                }
                if let ExprKind::NodeRef(sig_id) = &args[0].kind {
                    match self.signals.get(sig_id).copied() {
                        Some((sig, _)) => match &sig.ty {
                            AionType::List(elem_ty) => {
                                if !self.check_expr_compatible(node_id, &args[2], elem_ty, ui_locals) {
                                    let actual = self.eval_expr(node_id, &args[2], ui_locals, None);
                                    if actual != AionType::Unknown && !actual.is_assignable_to(elem_ty) {
                                        self.diagnostics.push(
                                            Diagnostic::error(
                                                "E_TYPE_MISMATCH",
                                                Phase::Checker,
                                                format!("`SETAT({sig_id}, ...)` expects `{elem_ty}`, found `{actual}`"),
                                                args[2].span,
                                            )
                                            .with_node_id(node_id)
                                            .with_expected(elem_ty.to_string())
                                            .with_found(actual.to_string()),
                                        );
                                    }
                                }
                            }
                            other => self.diagnostics.push(
                                Diagnostic::error(
                                    "E_TYPE_MISMATCH",
                                    Phase::Checker,
                                    format!("`SETAT` target `{sig_id}` must be `list<T>`, found `{other}`"),
                                    args[0].span,
                                )
                                .with_node_id(node_id),
                            ),
                        },
                        None => self.diagnostics.push(
                            Diagnostic::error(
                                "E_UNKNOWN_NODE",
                                Phase::Checker,
                                format!("`SETAT` in `{node_id}` references undefined signal `{sig_id}`"),
                                args[0].span,
                            )
                            .with_node_id(node_id),
                        ),
                    }
                }
                let _ = self.eval_expr(node_id, &args[1], ui_locals, None);
            }
            _ => {
                for a in args {
                    let _ = self.eval_expr(node_id, a, ui_locals, None);
                }
            }
        }
    }

    fn check_api_call_payload(
        &mut self,
        node_id: &str,
        api_id: &NodeId,
        api: &ApiNode,
        payload: &Expr,
        ui_locals: &HashMap<String, AionType>,
    ) {
        if let ExprKind::Object(entries) = &payload.kind {
            let provided: HashMap<&str, &Expr> =
                entries.iter().map(|(k, v)| (k.as_str(), v)).collect();

            for field in &api.input_fields {
                match provided.get(field.name.as_str()) {
                    None => {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_API_INPUT_MISSING",
                                Phase::Checker,
                                format!(
                                    "CALL({api_id}, ...) in `{node_id}` is missing required input field `{}`: `{}`",
                                    field.name, field.ty
                                ),
                                payload.span,
                            )
                            .with_node_id(node_id)
                            .with_expected(format!("{}: {}", field.name, field.ty))
                            .with_fix_hint(format!(
                                "Add `{}: <{}>` to the payload passed to `{api_id}`.",
                                field.name, field.ty
                            )),
                        );
                    }
                    Some(val_expr) => {
                        if !self.check_expr_compatible(node_id, val_expr, &field.ty, ui_locals) {
                            let actual_ty =
                                self.eval_expr(node_id, val_expr, ui_locals, None);
                            if actual_ty != AionType::Unknown && !actual_ty.is_assignable_to(&field.ty)
                            {
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_TYPE_MISMATCH",
                                        Phase::Checker,
                                        format!(
                                            "CALL({api_id}, ...) field `{}` expects `{}`, found `{actual_ty}`",
                                            field.name, field.ty
                                        ),
                                        val_expr.span,
                                    )
                                    .with_node_id(node_id)
                                    .with_expected(field.ty.to_string())
                                    .with_found(actual_ty.to_string()),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            let _ = self.eval_expr(node_id, payload, ui_locals, None);
        }
    }

    fn check_api_bind(
        &mut self,
        node_id: &str,
        api: &ApiNode,
        signal_id: &NodeId,
        value_expr: &Expr,
        span: Span,
        ui_locals: &HashMap<String, AionType>,
    ) {
        let Some((sig, _)) = self.signals.get(signal_id).copied() else {
            self.diagnostics.push(
                Diagnostic::error(
                    "E_UNKNOWN_NODE",
                    Phase::Checker,
                    format!(
                        "UI node `{node_id}` binds result to undefined signal `{signal_id}`"
                    ),
                    span,
                )
                .with_node_id(node_id)
                .with_fix_hint(format!("Declare signal `{signal_id}` before binding to it.")),
            );
            return;
        };

        let mut scope = ui_locals.clone();
        let res_ty = match &api.output {
            Some(ApiOutput::ModelRef(id)) => AionType::ModelRef(id.clone()),
            Some(ApiOutput::Type(ty)) => ty.clone(),
            Some(ApiOutput::Fields(_)) => AionType::Any,
            None => AionType::Void,
        };
        scope.insert("res".to_string(), res_ty);

        let val_ty = self.eval_expr(node_id, value_expr, &scope, None);
        if val_ty != AionType::Unknown && !val_ty.is_assignable_to(&sig.ty) {
            self.diagnostics.push(
                Diagnostic::error(
                    "E_TYPE_MISMATCH",
                    Phase::Checker,
                    format!(
                        "cannot bind `{val_ty}` to signal `{signal_id}` of type `{}`",
                        sig.ty
                    ),
                    span,
                )
                .with_node_id(node_id)
                .with_expected(sig.ty.to_string())
                .with_found(val_ty.to_string()),
            );
        }
    }

    fn check_stmt(
        &mut self,
        node_id: &str,
        stmt: &Stmt,
        locals: &mut HashMap<String, AionType>,
        api_ctx: Option<&ApiNode>,
    ) {
        match stmt {
            Stmt::Let {
                name,
                ty,
                value,
                fallback_err,
                span,
            } => {
                let val_ty = self.eval_expr(node_id, value, locals, api_ctx);
                if let Some(annotated) = ty {
                    if val_ty != AionType::Unknown && !val_ty.is_assignable_to(annotated) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_TYPE_MISMATCH",
                                Phase::Checker,
                                format!(
                                    "let `{name}` annotated as `{annotated}`, but expression has type `{val_ty}`"
                                ),
                                *span,
                            )
                            .with_node_id(node_id)
                            .with_expected(annotated.to_string())
                            .with_found(val_ty.to_string()),
                        );
                    }
                    locals.insert(name.clone(), annotated.clone());
                } else {
                    locals.insert(name.clone(), val_ty);
                }
                if let Some(err) = fallback_err {
                    let msg_ty = self.eval_expr(node_id, &err.message, locals, api_ctx);
                    if msg_ty != AionType::Unknown && !msg_ty.is_assignable_to(&AionType::Str) {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E_TYPE_MISMATCH",
                                Phase::Checker,
                                format!("ERR message must be `str`, found `{msg_ty}`"),
                                err.message.span,
                            )
                            .with_node_id(node_id),
                        );
                    }
                }
            }
            Stmt::Expr {
                expr,
                fallback_err,
                flow,
                ..
            } => {
                let _ = self.eval_expr(node_id, expr, locals, api_ctx);
                if let Some(err) = fallback_err {
                    let _ = self.eval_expr(node_id, &err.message, locals, api_ctx);
                }
                if let Some(FlowControl::Err(err)) = flow {
                    let _ = self.eval_expr(node_id, &err.message, locals, api_ctx);
                }
            }
        }
    }

    fn eval_expr(
        &mut self,
        node_id: &str,
        expr: &Expr,
        locals: &HashMap<String, AionType>,
        api_ctx: Option<&ApiNode>,
    ) -> AionType {
        match &expr.kind {
            ExprKind::Int(_) => AionType::I32,
            ExprKind::Float(_) => AionType::F64,
            ExprKind::Str(_) => AionType::Str,
            ExprKind::Bool(_) => AionType::Bool,
            ExprKind::Null => AionType::Any,
            ExprKind::NodeRef(id) => {
                if let Some((sig, _)) = self.signals.get(id) {
                    sig.ty.clone()
                } else if id.prefix() == Some(NodePrefix::Model) && self.models.contains_key(id) {
                    AionType::ModelRef(id.clone())
                } else {
                    AionType::Unknown
                }
            }
            ExprKind::Ident(name) => {
                if name == "CFG" {
                    AionType::Named("__cfg__".to_string())
                } else if let Some(ty) = locals.get(name) {
                    ty.clone()
                } else if self.ext_nodes.contains_key(name) {
                    AionType::Named(format!("__ext__:{name}"))
                } else {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_UNKNOWN_IDENT",
                            Phase::Checker,
                            format!("undefined identifier `{name}` in `{node_id}`"),
                            expr.span,
                        )
                        .with_node_id(node_id)
                        .with_found(name.clone())
                        .with_fix_hint(format!(
                            "Declare `{name}` with `let {name} = ...` or check spelling."
                        )),
                    );
                    AionType::Unknown
                }
            }
            ExprKind::Member { object, field } => {
                let obj_ty = self.eval_expr(node_id, object, locals, api_ctx);
                match obj_ty {
                    AionType::Named(ref tag) if tag == "__cfg__" => {
                        if let Some(ty) = self.cfg_vars.get(field) {
                            ty.clone()
                        } else {
                            let known: Vec<&str> =
                                self.cfg_vars.keys().map(String::as_str).collect();
                            self.diagnostics.push(
                                Diagnostic::error(
                                    "E_UNKNOWN_CFG_VAR",
                                    Phase::Checker,
                                    format!(
                                        "configuration variable `CFG.{field}` is not declared in `#CFG`"
                                    ),
                                    expr.span,
                                )
                                .with_node_id(node_id)
                                .with_expected(known.join(", "))
                                .with_found(field.clone())
                                .with_fix_hint(format!(
                                    "Add `{field}` to `#CFG:ENV{{...}}` or use one of: {}.",
                                    known.join(", ")
                                )),
                            );
                            AionType::Unknown
                        }
                    }
                    AionType::Named(ref tag) if tag == "__api_in__" => {
                        if let Some(api) = api_ctx {
                            if let Some(f) = api.input_fields.iter().find(|f| &f.name == field) {
                                f.ty.clone()
                            } else {
                                let known: Vec<&str> = api
                                    .input_fields
                                    .iter()
                                    .map(|f| f.name.as_str())
                                    .collect();
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_INPUT_FIELD",
                                        Phase::Checker,
                                        format!(
                                            "input field `in.{field}` is not declared in `{node_id}` IN{{...}}"
                                        ),
                                        expr.span,
                                    )
                                    .with_node_id(node_id)
                                    .with_expected(known.join(", "))
                                    .with_found(field.clone()),
                                );
                                AionType::Unknown
                            }
                        } else {
                            AionType::Unknown
                        }
                    }
                    AionType::ModelRef(ref model_id) => {
                        if let Some((model, _)) = self.models.get(model_id) {
                            if let Some(f) = model.fields.iter().find(|f| &f.name == field) {
                                f.ty.clone()
                            } else {
                                let known: Vec<&str> =
                                    model.fields.iter().map(|f| f.name.as_str()).collect();
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_FIELD",
                                        Phase::Checker,
                                        format!(
                                            "model `{}` ({model_id}) has no field `{field}`",
                                            model.name
                                        ),
                                        expr.span,
                                    )
                                    .with_node_id(node_id)
                                    .with_expected(known.join(", "))
                                    .with_found(field.clone())
                                    .with_fix_hint(format!(
                                        "Use an existing field on `{}` ({}) or add `{field}: <type>` to `{model_id}`.",
                                        model.name,
                                        known.join(", ")
                                    )),
                                );
                                AionType::Unknown
                            }
                        } else {
                            AionType::Unknown
                        }
                    }
                    AionType::ExtType {
                        ext_alias: Some(ref alias),
                        ref name,
                    } => {
                        if let Some((_, Some(manifest), _)) = self.ext_nodes.get(alias) {
                            if let Some(fields) = manifest.types.get(name) {
                                if let Some(raw_ty) = fields.get(field) {
                                    return AionType::parse(raw_ty).unwrap_or(AionType::Unknown);
                                }
                                let known: Vec<&str> =
                                    fields.keys().map(String::as_str).collect();
                                self.diagnostics.push(
                                    Diagnostic::error(
                                        "E_UNKNOWN_FIELD",
                                        Phase::Ext,
                                        format!(
                                            "external type `@{name}` in `{}` has no field `{field}`",
                                            manifest.id
                                        ),
                                        expr.span,
                                    )
                                    .with_node_id(node_id)
                                    .with_expected(known.join(", "))
                                    .with_found(field.clone())
                                    .with_fix_hint(format!(
                                        "Available fields on `@{name}`: {}.",
                                        known.join(", ")
                                    )),
                                );
                            }
                        }
                        AionType::Unknown
                    }
                    AionType::Any => AionType::Any,
                    AionType::Unknown => AionType::Unknown,
                    _ => AionType::Unknown,
                }
            }
            ExprKind::Call { callee, args } => {
                // Check `<alias>.<method>(...)` or `ctx.<method>(...)`
                if let ExprKind::Member { object, field } = &callee.kind {
                    if let ExprKind::Ident(obj_name) = &object.kind {
                        if obj_name == "ctx"
                            || matches!(locals.get(obj_name), Some(AionType::Named(t)) if t == "__ctx__")
                        {
                            return self.check_ctx_call(node_id, field, args, expr.span, locals, api_ctx);
                        }
                        if self.ext_nodes.contains_key(obj_name) {
                            return self.check_ext_call(
                                node_id, obj_name, field, args, expr.span, locals, api_ctx,
                            );
                        }
                    }
                }
                if let ExprKind::Ident(fname) = &callee.kind {
                    match fname.as_str() {
                        "UID" => return AionType::Str,
                        "NOW" => return AionType::F64,
                        "FMT_TIME" | "FMT_MONEY" => {
                            for arg in args {
                                let _ = self.eval_expr(node_id, arg, locals, api_ctx);
                            }
                            return AionType::Str;
                        }
                        "SUM" | "ROUND" => {
                            for arg in args {
                                let _ = self.eval_expr(node_id, arg, locals, api_ctx);
                            }
                            return AionType::F64;
                        }
                        "COUNT" => {
                            for arg in args {
                                let _ = self.eval_expr(node_id, arg, locals, api_ctx);
                            }
                            return AionType::I32;
                        }
                        _ => {}
                    }
                }
                let _ = self.eval_expr(node_id, callee, locals, api_ctx);
                for arg in args {
                    let _ = self.eval_expr(node_id, arg, locals, api_ctx);
                }
                AionType::Any
            }
            ExprKind::Object(entries) => {
                for (_, val) in entries {
                    let _ = self.eval_expr(node_id, val, locals, api_ctx);
                }
                AionType::Map(Box::new(AionType::Str), Box::new(AionType::Any))
            }
            ExprKind::List(items) => {
                let mut elem_ty = AionType::Any;
                if let Some(first) = items.first() {
                    elem_ty = self.eval_expr(node_id, first, locals, api_ctx);
                }
                for item in items.iter().skip(1) {
                    let _ = self.eval_expr(node_id, item, locals, api_ctx);
                }
                AionType::List(Box::new(elem_ty))
            }
            ExprKind::Binary { op, left, right } => {
                let lty = self.eval_expr(node_id, left, locals, api_ctx);
                let rty = self.eval_expr(node_id, right, locals, api_ctx);
                match op {
                    BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
                    | BinaryOp::And
                    | BinaryOp::Or => AionType::Bool,
                    BinaryOp::Add if lty == AionType::Str || rty == AionType::Str => AionType::Str,
                    BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
                        if lty == AionType::F64 || rty == AionType::F64 =>
                    {
                        AionType::F64
                    }
                    _ => lty,
                }
            }
        }
    }

    fn check_ctx_call(
        &mut self,
        node_id: &str,
        method: &str,
        args: &[Expr],
        span: Span,
        locals: &HashMap<String, AionType>,
        api_ctx: Option<&ApiNode>,
    ) -> AionType {
        for arg in args {
            let _ = self.eval_expr(node_id, arg, locals, api_ctx);
        }
        match method {
            "hdr" => {
                if args.len() != 1 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`ctx.hdr` expects 1 argument (header name), found {}", args.len()),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("ctx.hdr(name: str) -> str"),
                    );
                }
                AionType::Str
            }
            "get" => {
                if args.len() != 1 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`ctx.get` expects 1 argument (key), found {}", args.len()),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("ctx.get(key: str) -> any"),
                    );
                }
                AionType::Any
            }
            "set" => {
                if args.len() != 2 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`ctx.set` expects 2 arguments (key, value), found {}", args.len()),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("ctx.set(key: str, value: any) -> void"),
                    );
                }
                AionType::Void
            }
            "param" => {
                if args.len() != 1 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`ctx.param` expects 1 argument (route param name), found {}", args.len()),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("ctx.param(name: str) -> str"),
                    );
                }
                AionType::Str
            }
            "query" => {
                if args.len() != 1 {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_ARG_COUNT",
                            Phase::Checker,
                            format!("`ctx.query` expects 1 argument (query param name), found {}", args.len()),
                            span,
                        )
                        .with_node_id(node_id)
                        .with_expected("ctx.query(name: str) -> str"),
                    );
                }
                AionType::Str
            }
            other => {
                self.diagnostics.push(
                    Diagnostic::error(
                        "E_UNKNOWN_METHOD",
                        Phase::Checker,
                        format!("context `ctx` has no method `{other}`"),
                        span,
                    )
                    .with_node_id(node_id)
                    .with_expected("hdr, get, set, param, query")
                    .with_found(other.to_string())
                    .with_fix_hint("Use one of `ctx.hdr(name)`, `ctx.get(key)`, `ctx.set(key, value)`, `ctx.param(name)`, or `ctx.query(name)`."),
                );
                AionType::Unknown
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_ext_call(
        &mut self,
        node_id: &str,
        alias: &str,
        method: &str,
        args: &[Expr],
        span: Span,
        locals: &HashMap<String, AionType>,
        api_ctx: Option<&ApiNode>,
    ) -> AionType {
        let arg_types: Vec<AionType> = args
            .iter()
            .map(|a| self.eval_expr(node_id, a, locals, api_ctx))
            .collect();

        let Some((ext_node, manifest_opt, _)) = self.ext_nodes.get(alias).copied() else {
            return AionType::Unknown;
        };

        let Some(manifest) = manifest_opt else {
            // Already reported E_EXT_NOT_FOUND in collect_symbols.
            return AionType::Unknown;
        };

        let Some(sig) = manifest.fns.get(method) else {
            let available: BTreeMap<&str, String> = manifest
                .fns
                .iter()
                .map(|(name, s)| (name.as_str(), s.format_signature(name)))
                .collect();
            let sig_list: Vec<String> = available.values().cloned().collect();
            let names_list: Vec<&str> = available.keys().copied().collect();

            self.diagnostics.push(
                Diagnostic::error(
                    "E_EXT_METHOD_NOT_FOUND",
                    Phase::Ext,
                    format!(
                        "method `{method}` does not exist on external module `{}@{}` (alias `{alias}`)",
                        ext_node.module_id, manifest.ver
                    ),
                    span,
                )
                .with_node_id(node_id)
                .with_expected(sig_list.join("; "))
                .with_found(format!("{alias}.{method}"))
                .with_fix_hint(format!(
                    "Replace `{alias}.{method}` with a valid method from `{}`: {}.",
                    manifest.id,
                    names_list.join(", ")
                )),
            );
            return AionType::Unknown;
        };

        let min_args = sig.args.iter().filter(|a| !a.opt).count();
        let max_args = sig.args.len();
        if args.len() < min_args || args.len() > max_args {
            self.diagnostics.push(
                Diagnostic::error(
                    "E_EXT_ARG_COUNT",
                    Phase::Ext,
                    format!(
                        "`{alias}.{method}` expects {min_args}..={max_args} argument(s), found {}",
                        args.len()
                    ),
                    span,
                )
                .with_node_id(node_id)
                .with_expected(sig.format_signature(method))
                .with_found(format!("{} argument(s)", args.len()))
                .with_fix_hint(format!(
                    "Update call to match `{}`.",
                    sig.format_signature(method)
                )),
            );
            return AionType::Unknown;
        }

        for (idx, (arg_expr, actual_ty)) in args.iter().zip(arg_types.iter()).enumerate() {
            let expected_spec = &sig.args[idx];
            if let Ok(mut expected_ty) = AionType::parse(&expected_spec.t) {
                attach_ext_alias(&mut expected_ty, alias);
                if *actual_ty != AionType::Unknown && !actual_ty.is_assignable_to(&expected_ty) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_EXT_ARG_TYPE",
                            Phase::Ext,
                            format!(
                                "argument `{}` (#{}) of `{alias}.{method}` expects `{}`, found `{}`",
                                expected_spec.n,
                                idx + 1,
                                expected_spec.t,
                                actual_ty
                            ),
                            arg_expr.span,
                        )
                        .with_node_id(node_id)
                        .with_expected(sig.format_signature(method))
                        .with_found(format!("{}: {}", expected_spec.n, actual_ty))
                        .with_fix_hint(format!(
                            "Pass a `{}` value for `{}` in `{alias}.{method}`.",
                            expected_spec.t, expected_spec.n
                        )),
                    );
                }
            }
        }

        let mut ret_ty = AionType::parse(&sig.ret).unwrap_or(AionType::Unknown);
        attach_ext_alias(&mut ret_ty, alias);
        ret_ty
    }

    fn check_ui_layout(&mut self) {
        if self.uis.is_empty() {
            return;
        }
        // Skip layout calculation if a UI cycle was already reported.
        if self
            .diagnostics
            .iter()
            .any(|d| d.code == "E_CIRCULAR_REF")
        {
            return;
        }

        let signals = aion_runtime_ui::SignalStore::from_program(self.program);
        for viewport in [
            aion_runtime_ui::Viewport::MOBILE,
            aion_runtime_ui::Viewport::DESKTOP,
        ] {
            if let Ok(layout_tree) =
                aion_runtime_ui::compute_layout(self.program, viewport, Some(&signals))
            {
                for issue in layout_tree.detect_overflows() {
                    let span = self
                        .uis
                        .get(&issue.child_id)
                        .map(|(_, s)| *s)
                        .unwrap_or_default();
                    let parent_desc = issue
                        .parent_id
                        .as_ref()
                        .map(|p| format!("container `{p}`"))
                        .unwrap_or_else(|| format!("viewport `{}`", issue.viewport_name));
                    let fix_hint = match &issue.parent_id {
                        Some(pid) => format!(
                            "Reduce `{}` size (currently {:.0}x{:.0}px) to fit within {:.0}x{:.0}px on {} viewport, use percentage width, or add `scroll` to `{pid}`.",
                            issue.child_id,
                            issue.child_width,
                            issue.child_height,
                            issue.container_width,
                            issue.container_height,
                            issue.viewport_name
                        ),
                        None => format!(
                            "Reduce `{}` size (currently {:.0}x{:.0}px) to fit within {:.0}x{:.0}px on {} viewport.",
                            issue.child_id,
                            issue.child_width,
                            issue.child_height,
                            issue.container_width,
                            issue.container_height,
                            issue.viewport_name
                        ),
                    };

                    self.diagnostics.push(
                        Diagnostic::error(
                            "E_UI_OVERFLOW",
                            Phase::Checker,
                            format!(
                                "UI node `{}` overflows {} ({}) on {} viewport ({:.0}x{:.0}px > {:.0}x{:.0}px)",
                                issue.child_id,
                                parent_desc,
                                issue.axis,
                                issue.viewport_name,
                                issue.child_width,
                                issue.child_height,
                                issue.container_width,
                                issue.container_height,
                            ),
                            span,
                        )
                        .with_node_id(issue.child_id.as_str())
                        .with_expected(format!(
                            "<= {:.0}x{:.0}px",
                            issue.container_width, issue.container_height
                        ))
                        .with_found(format!("{:.0}x{:.0}px", issue.child_width, issue.child_height))
                        .with_fix_hint(fix_hint),
                    );
                }
            }
        }
    }
}

fn attach_ext_alias(ty: &mut AionType, alias: &str) {
    match ty {
        AionType::ExtType { ext_alias, .. } => {
            *ext_alias = Some(alias.to_string());
        }
        AionType::List(inner) => attach_ext_alias(inner, alias),
        AionType::Map(k, v) => {
            attach_ext_alias(k, alias);
            attach_ext_alias(v, alias);
        }
        _ => {}
    }
}
