pub mod checker;

pub use checker::{check_program, check_to_json_report};

#[cfg(test)]
mod tests {
    use super::*;
    use aion_ext::ExtRegistry;
    use aion_parser::parse_program;
    use std::path::Path;

    fn mock_registry() -> ExtRegistry {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
        let mut reg = ExtRegistry::new();
        reg.register_manifest_file(&base.join("aion-mongo-1.2.0.json"))
            .unwrap();
        reg.register_manifest_file(&base.join("aion-firebase-auth-2.0.0.json"))
            .unwrap();
        reg
    }

    const ARCHITECTURE_EXAMPLE: &str = r#"
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
    fn validates_full_architecture_example_with_zero_errors() {
        let prog = parse_program(ARCHITECTURE_EXAMPLE).expect("example should parse");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(
            report.ok,
            "expected zero errors, got: {}",
            report.to_json_pretty().unwrap()
        );
        assert_eq!(report.error_count, 0);
    }

    #[test]
    fn emits_json_error_when_invoking_nonexistent_ext_method() {
        let src = r#"
#CFG:ENV{MONGO_URI, FB_PROJ_ID}
#EXT:db = "aion/mongo@1.2"
#EXT:auth = "aion/firebase-auth@2.0"
#SRV0:HTTP[port:8080, proto:on]
#M0:User{id:str, uid:str, pts:i32}
#API0:#SRV0.POST["/v1/pts"] IN{delta:i32} OUT{#M0} {
  db.connect(CFG.MONGO_URI)
  db.upsert_magic("users", {uid:"u1"}) -> RET
}
"#;
        let prog = parse_program(src).expect("should parse");
        let reg = mock_registry();
        let json_err = check_to_json_report(&prog, &reg).unwrap_err();
        let val: serde_json::Value = serde_json::from_str(&json_err).unwrap();

        assert_eq!(val["ok"], false);
        assert_eq!(val["error_count"], 1);
        let diag = &val["diagnostics"][0];
        assert_eq!(diag["code"], "E_EXT_METHOD_NOT_FOUND");
        assert_eq!(diag["phase"], "ext");
        assert_eq!(diag["node_id"], "#API0");
        assert_eq!(diag["found"], "db.upsert_magic");
        assert!(diag["expected"].as_str().unwrap().contains("upd("));
        assert!(diag["fix_hint"].as_str().unwrap().contains("aion/mongo"));
    }

    #[test]
    fn emits_json_error_when_ext_arg_type_mismatches() {
        let src = r#"
#CFG:ENV{FB_PROJ_ID}
#EXT:auth = "aion/firebase-auth@2.0"
#MW0:PIPE(ctx) {
  let usr = auth.verify(123, CFG.FB_PROJ_ID) ? ERR(401, "invalid_token")
  ctx.set("uid", usr.uid) -> NEXT
}
"#;
        let prog = parse_program(src).expect("should parse");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(!report.ok);
        assert_eq!(report.diagnostics[0].code, "E_EXT_ARG_TYPE");
    }

    #[test]
    fn requires_err_handler_on_ui_api_calls() {
        let src = r#"
#SRV0:HTTP[port:8080, proto:on]
#M0:User{id:str, pts:i32}
#API0:#SRV0.POST["/v1/pts"] IN{delta:i32} OUT{#M0} {
  "ok" -> RET
}
#S0:i32=0
#N0:BTN[160,48,8,#2563EB]{} @tap->CALL(#API0,{delta:10})=>#S0=res.pts
"#;
        let prog = parse_program(src).expect("should parse");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(!report.ok);
        assert_eq!(report.diagnostics[0].code, "E_MISSING_ERR_HANDLER");
        assert_eq!(report.diagnostics[0].node_id.as_deref(), Some("#N0"));
    }

    #[test]
    fn detects_circular_ui_and_model_references() {
        let src = r#"
#M0:A{b:#M1}
#M1:B{a:#M0}
#N0:FLEX[v,c,c,8]{#N1}
#N1:FLEX[v,c,c,8]{#N0}
"#;
        let prog = parse_program(src).expect("should parse");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(!report.ok);
        let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes.iter().filter(|&&c| c == "E_CIRCULAR_REF").count(), 2);
    }

    #[test]
    fn validates_all_primitive_types_and_signal_binding_mismatch() {
        let valid_src = r#"
#M0:Event{id:str, count:i32, ratio:f64, active:bool, payload:bytes, created_at:ts, tags:list<str>, meta:map<str,i32>}
#SRV0:HTTP[port:8080, proto:on]
#API0:#SRV0.GET["/v1/event"] OUT{#M0} {
  "ok" -> RET
}
#S0:str=""
#N0:BTN[100,40,4,#2563EB]{} @tap->CALL(#API0,{})=>#S0=res.id @err->TOAST("err")
"#;
        let prog = parse_program(valid_src).expect("should parse all primitive types");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(report.ok, "unexpected errors: {:?}", report.diagnostics);

        // Binding an i32 field (`res.count`) into a str signal (`#S0`) must fail typecheck.
        let mismatch_src = r#"
#M0:Event{id:str, count:i32}
#SRV0:HTTP[port:8080, proto:on]
#API0:#SRV0.GET["/v1/event"] OUT{#M0} {
  "ok" -> RET
}
#S0:str=""
#N0:BTN[100,40,4,#2563EB]{} @tap->CALL(#API0,{})=>#S0=res.count @err->TOAST("err")
"#;
        let mismatch_prog = parse_program(mismatch_src).expect("should parse");
        let mismatch_report = check_program(&mismatch_prog, &reg);
        assert!(!mismatch_report.ok);
        assert_eq!(mismatch_report.diagnostics[0].code, "E_TYPE_MISMATCH");
    }

    #[test]
    fn emits_json_error_e_ui_overflow_for_500px_button_on_mobile() {
        let src = r#"
#N0:ROOT[0,0,100%,100%,#F8FAFC]{#N1}
#N1:FLEX[v,c,c,16]{#N2}
#N2:BTN[500,48,8,#2563EB]{}
"#;
        let prog = parse_program(src).expect("should parse");
        let reg = mock_registry();
        let json_err = check_to_json_report(&prog, &reg).unwrap_err();
        let val: serde_json::Value = serde_json::from_str(&json_err).unwrap();

        assert_eq!(val["ok"], false);
        let diag = &val["diagnostics"][0];
        assert_eq!(diag["code"], "E_UI_OVERFLOW");
        assert_eq!(diag["node_id"], "#N2");
        assert!(diag["message"].as_str().unwrap().contains("mobile"));
        assert!(diag["fix_hint"].as_str().unwrap().contains("390"));
    }

    #[test]
    fn validates_sel_dropdown_multi_tap_and_local_math() {
        let src = r#"
#S_moneda:str = "MXN"
#S_monto:f64 = 100.0
#S_tasa:f64 = 1.0
#S_total:f64 = 100.0
#N_root:ROOT[0,0,100%,100%,#F8FAFC]{#N_sel,#N_btn}
#N_sel:SEL[320,44,8,#FFFFFF,#S_moneda]{"MXN|Peso", "USD|Dolar"} @change->#S_total=#S_monto*#S_tasa
#N_btn:BTN[200,40,8,#2563EB]{} @tap->#S_tasa=0.055 @tap->#S_total=#S_monto*#S_tasa @tap->TOAST("ok")
"#;
        let prog = parse_program(src).expect("should parse SEL + multi tap");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(report.ok, "{}", report.to_json_pretty().unwrap());
        let sel = prog.get(&aion_ast::NodeId::new("#N_sel")).unwrap();
        match &sel.kind {
            aion_ast::NodeKind::Ui(ui) => {
                assert!(matches!(&ui.body, aion_ast::UiBody::Options(o) if o.len() == 2));
            }
            _ => panic!("expected ui"),
        }
        let btn = prog.get(&aion_ast::NodeId::new("#N_btn")).unwrap();
        match &btn.kind {
            aion_ast::NodeKind::Ui(ui) => assert_eq!(ui.events.len(), 3),
            _ => panic!("expected ui"),
        }
        // Round trip through the canonical formatter.
        let again = parse_program(&aion_ast::format_program(&prog)).expect("reparse");
        assert_eq!(again.nodes.len(), prog.nodes.len());

        // Binding SEL to an undeclared signal must fail.
        let bad = src.replace("#S_moneda]{", "#S_nope]{");
        let bad_prog = parse_program(&bad).unwrap();
        assert!(!check_program(&bad_prog, &reg).ok);
    }

    #[test]
    fn validates_semantic_ids_persist_init_and_dynamic_list_with_push_remove() {
        let src = r#"
#SRV0:HTTP[port:8080, proto:on]
#M_Gasto:Gasto{concepto:str, monto:f64}
#API_list:#SRV0.GET["/v1/gastos"] OUT{list<#M_Gasto>} {
  [] -> RET
}
#S_concepto:str=""
#S_gastos:list<#M_Gasto>=[{concepto:"Renta", monto:8500.0}] @persist

#N_root:ROOT[16,16,100%,100%,#F8FAFC]{#N_main} @init->CALL(#API_list,{})=>#S_gastos=res @err->TOAST("Error cargando")
#N_main:FLEX[v,s,st,12]{#N_inp,#N_add,#N_list}
#N_inp:INP[280,44,8,#FFFFFF,#S_concepto]{"Concepto..."}
#N_add:BTN[180,44,8,#2563EB]{#N_add_txt} @tap->PUSH(#S_gastos,{concepto:#S_concepto, monto:150.0})
#N_add_txt:TXT[14,600,#FFFFFF]{"Agregar"}
#N_list:LIST[v,8,#S_gastos]{#N_row}
#N_row:FLEX[h,sb,c,8]{#N_item_lbl,#N_del}
#N_item_lbl:TXT[14,500,#0F172A]{"{item.concepto}: ${item.monto}"}
#N_del:BTN[80,32,6,#DC2626]{#N_del_txt} @tap->REMOVE(#S_gastos,index)
#N_del_txt:TXT[12,600,#FFFFFF]{"Borrar"}
"#;
        let prog = parse_program(src).expect("should parse v0.2 features");
        let reg = mock_registry();
        let report = check_program(&prog, &reg);
        assert!(
            report.ok,
            "expected zero errors, got: {}",
            report.to_json_pretty().unwrap()
        );

        // Round-trip canonical unparser test
        let formatted = aion_ast::format_program(&prog);
        let reparsed = parse_program(&formatted).expect("formatted output must re-parse cleanly");
        let report2 = check_program(&reparsed, &reg);
        assert!(report2.ok, "reparsed program must pass static checker");

        // Typo in `{item.inexistente}` must trigger E_UNKNOWN_FIELD
        let bad = src.replace("{item.monto}", "{item.inexistente}");
        let bad_prog = parse_program(&bad).expect("should parse");
        let bad_report = check_program(&bad_prog, &reg);
        assert!(!bad_report.ok);
        assert_eq!(bad_report.diagnostics[0].code, "E_UNKNOWN_FIELD");
    }
}
