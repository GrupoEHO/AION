pub mod ast;
pub mod diagnostic;
pub mod patch;
pub mod types;
pub mod unparse;

pub use ast::*;
pub use diagnostic::*;
pub use patch::*;
pub use types::*;
pub use unparse::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_primitive_and_composite_types() {
        assert_eq!(AionType::parse("i32").unwrap(), AionType::I32);
        assert_eq!(AionType::parse("f64").unwrap(), AionType::F64);
        assert_eq!(AionType::parse("str").unwrap(), AionType::Str);
        assert_eq!(AionType::parse("bool").unwrap(), AionType::Bool);
        assert_eq!(AionType::parse("bytes").unwrap(), AionType::Bytes);
        assert_eq!(AionType::parse("ts").unwrap(), AionType::Ts);
        assert_eq!(
            AionType::parse("list<str>").unwrap(),
            AionType::List(Box::new(AionType::Str))
        );
        assert_eq!(
            AionType::parse("map<str, list<i32>>").unwrap(),
            AionType::Map(
                Box::new(AionType::Str),
                Box::new(AionType::List(Box::new(AionType::I32)))
            )
        );
    }

    #[test]
    fn differential_patch_add_patch_del_is_transactional() {
        let mut prog = Program::default();
        let s0 = FlatNode {
            id: NodeId::new("#S0"),
            span: Span::default(),
            kind: NodeKind::Signal(SignalNode {
                ty: AionType::I32,
                init: Expr {
                    kind: ExprKind::Int(0),
                    span: Span::default(),
                },
                persist: false,
            }),
        };
        let n0 = FlatNode {
            id: NodeId::new("#N0"),
            span: Span::default(),
            kind: NodeKind::Ui(UiNode {
                widget: "TXT".to_string(),
                args: vec![UiArg::Int(16)],
                body: UiBody::Text {
                    template: "Hi".to_string(),
                    interpolations: vec![],
                },
                events: vec![],
            }),
        };

        let report = prog
            .apply_patches(&[
                PatchOp::Add {
                    node: s0,
                    after: None,
                },
                PatchOp::Add {
                    node: n0,
                    after: Some(NodeId::new("#S0")),
                },
                PatchOp::Patch {
                    id: NodeId::new("#S0"),
                    delta: PatchDelta::SetSignalInit {
                        init: Expr {
                            kind: ExprKind::Int(42),
                            span: Span::default(),
                        },
                    },
                },
            ])
            .unwrap();
        assert_eq!(report.applied, 3);
        assert_eq!(report.total_nodes, 2);

        // Transactional rollback on invalid batch
        let err = prog
            .apply_patches(&[
                PatchOp::Del {
                    id: NodeId::new("#N0"),
                },
                PatchOp::Del {
                    id: NodeId::new("#MISSING"),
                },
            ])
            .unwrap_err();
        assert_eq!(err, PatchError::NodeNotFound(NodeId::new("#MISSING")));
        assert_eq!(prog.nodes.len(), 2);
    }
}
