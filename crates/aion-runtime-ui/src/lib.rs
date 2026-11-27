#[cfg(feature = "bridge")]
pub mod bridge;
pub mod layout;
#[cfg(feature = "gpu")]
pub mod renderer;
pub mod signals;

use aion_ast::NodeId;
use thiserror::Error;

#[cfg(feature = "bridge")]
pub use bridge::{
    DispatchOutcome, ReqwestTransport, UiHttpRequest, UiHttpResponse, UiHttpTransport,
    UiLifecycleEvent, UiRuntime,
};
pub use layout::{compute_layout, LayoutBox, LayoutTree, OverflowIssue, Viewport};
#[cfg(feature = "gpu")]
pub use renderer::{
    build_display_list, extract_rect_instances, parse_hex_color, request_wgpu_device,
    RectInstance, RenderPrimitive, TextShaper, ViewportUniform, WgpuRectPipeline,
    ROUNDED_RECT_WGSL,
};
pub use signals::{SignalStore, SignalValue};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UiError {
    #[error("unknown signal `{0}`")]
    UnknownSignal(NodeId),
    #[error("circular layout reference involving `{0}`")]
    CircularLayout(NodeId),
    #[error("layout computation error: {0}")]
    Layout(String),
    #[error("bridge error: {0}")]
    Bridge(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_parser::parse_program;

    const ARCHITECTURE_EXAMPLE: &str = r#"
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"
#SRV0:HTTP[port:8080, proto:on]
#MW0:PIPE(ctx) {
  let tok = ctx.hdr("Authorization")
  let usr = auth.verify(tok, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}
#M0:User{id:str, uid:str, pts:i32}
#API0:#SRV0.POST["/v1/pts", #MW0] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upd("users", {uid:ctx.get("uid")}, {$inc:{pts:in.delta}}) -> RET
}
#S0:i32=0
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2,#N3}
#N2:TXT[24,700,#0F172A]{"Puntos: {#S0}"}
#N3:BTN[160,48,8,#2563EB]{#N4} @tap->CALL(#API0,{delta:10})=>#S0=res.pts @err->TOAST("Error")
#N4:TXT[16,600,#FFFFFF]{"Sumar +10"}
"#;

    #[test]
    fn signals_mark_only_subscribed_ui_nodes_dirty() {
        let prog = parse_program(ARCHITECTURE_EXAMPLE).unwrap();
        let mut store = SignalStore::from_program(&prog);

        assert_eq!(store.get(&NodeId::new("#S0")), Some(&SignalValue::I32(0)));
        assert_eq!(store.resolve_text("Puntos: {#S0}"), "Puntos: 0");
        assert!(store.dirty_nodes().is_empty());

        let dirtied = store
            .set(&NodeId::new("#S0"), SignalValue::I32(10))
            .unwrap();
        assert_eq!(dirtied, vec![NodeId::new("#N2")]);
        assert!(store.is_dirty(&NodeId::new("#N2")));
        assert!(!store.is_dirty(&NodeId::new("#N0")));
        assert!(!store.is_dirty(&NodeId::new("#N3")));
        assert_eq!(store.resolve_text("Puntos: {#S0}"), "Puntos: 10");
    }

    #[test]
    fn computes_architecture_example_layout_for_mobile_and_desktop() {
        let prog = parse_program(ARCHITECTURE_EXAMPLE).unwrap();
        let store = SignalStore::from_program(&prog);

        let mobile = compute_layout(&prog, Viewport::MOBILE, Some(&store)).unwrap();
        assert!(mobile.detect_overflows().is_empty());
        let root_m = mobile.get(&NodeId::new("#N0")).unwrap();
        assert_eq!((root_m.width, root_m.height), (390.0, 844.0));
        let btn_m = mobile.get(&NodeId::new("#N3")).unwrap();
        assert_eq!((btn_m.width, btn_m.height), (160.0, 48.0));
        // Centered horizontally inside 390px: (390 - 160) / 2 = 115.0
        assert!((btn_m.x - 115.0).abs() < 1.0);

        let desktop = compute_layout(&prog, Viewport::DESKTOP, Some(&store)).unwrap();
        assert!(desktop.detect_overflows().is_empty());
        let btn_d = desktop.get(&NodeId::new("#N3")).unwrap();
        // Centered horizontally inside 1440px: (1440 - 160) / 2 = 640.0
        assert!((btn_d.x - 640.0).abs() < 1.0);
    }

    #[test]
    fn detects_mobile_overflow_for_500px_button() {
        let src = r#"
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2}
#N2:BTN[500,48,8,#2563EB]{}
"#;
        let prog = parse_program(src).unwrap();
        let mobile = compute_layout(&prog, Viewport::MOBILE, None).unwrap();
        let issues = mobile.detect_overflows();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].child_id, NodeId::new("#N2"));
        assert_eq!(issues[0].axis, "horizontal");
        assert_eq!(issues[0].container_width, 390.0);
        assert_eq!(issues[0].child_width, 500.0);
    }

    #[test]
    fn router_overlays_multiple_screens_without_vertical_overflow() {
        let src = r#"
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,s,s,0]{#N2,#N3}
#N2:NAV[100%,56,#0F172A]{#N10,#N11}
#N3:ROUTER[#N20]{#N20,#N30}
#N10:BTN[120,36,6,#2563EB]{} @tap->NAV(#N20)
#N11:BTN[120,36,6,#334155]{} @tap->NAV(#N30)
#N20:FLEX[v,c,c,16]{#N21}
#N21:TXT[20,700,#0F172A]{"Presupuesto"}
#N30:FLEX[v,c,c,16]{#N31}
#N31:TXT[20,700,#0F172A]{"Tarjetas"}
"#;
        let prog = parse_program(src).unwrap();
        let mobile = compute_layout(&prog, Viewport::MOBILE, None).unwrap();
        assert!(
            mobile.detect_overflows().is_empty(),
            "unexpected overflows: {:?}",
            mobile.detect_overflows()
        );
        let s1 = mobile.get(&NodeId::new("#N20")).unwrap();
        let s2 = mobile.get(&NodeId::new("#N30")).unwrap();
        assert_eq!((s1.x, s1.y, s1.width, s1.height), (s2.x, s2.y, s2.width, s2.height));
        assert_eq!(s1.y, 56.0);
    }

    #[cfg(feature = "gpu")]
    #[test]
    fn builds_gpu_display_list_and_shapes_text() {
        let prog = parse_program(ARCHITECTURE_EXAMPLE).unwrap();
        let store = SignalStore::from_program(&prog);
        let layout = compute_layout(&prog, Viewport::MOBILE, Some(&store)).unwrap();
        let display_list = build_display_list(&prog, &layout, &store);

        let rects = extract_rect_instances(&display_list);
        assert_eq!(rects.len(), 2); // #N0 (ROOT bg) and #N3 (BTN bg)

        let texts: Vec<&str> = display_list
            .iter()
            .filter_map(|p| match p {
                RenderPrimitive::Text { content, .. } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Puntos: 0", "Sumar +10"]);

        let mut shaper = TextShaper::new();
        let _glyphs = shaper.shape_text("Puntos: 0", 24.0, 390.0, [15, 23, 42, 255]);
    }
}
