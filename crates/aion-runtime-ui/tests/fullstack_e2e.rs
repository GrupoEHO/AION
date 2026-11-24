use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aion_ast::{HttpMethod, NodeId};
use aion_ext::{ExtManifest, ExtPlugin, ExtRegistry, ExtRuntimeError, ExtValue};
use aion_parser::parse_program;
use aion_runtime_net::{build_router, RuntimeConfig, JSON_MIME};
use aion_runtime_ui::{
    DispatchOutcome, SignalValue, UiHttpRequest, UiHttpResponse, UiHttpTransport, UiLifecycleEvent,
    UiRuntime,
};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

const FULLSTACK_EXAMPLE: &str = r#"
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

struct MockAuthPlugin {
    manifest: ExtManifest,
}

impl ExtPlugin for MockAuthPlugin {
    fn manifest(&self) -> &ExtManifest {
        &self.manifest
    }

    fn invoke(&self, fn_name: &str, args: Vec<ExtValue>) -> Result<ExtValue, ExtRuntimeError> {
        match fn_name {
            "verify" => {
                let tok = match args.first() {
                    Some(ExtValue::Str(s)) => s.as_str(),
                    _ => "",
                };
                if tok == "Bearer valid-jwt" {
                    let mut user = BTreeMap::new();
                    user.insert("uid".to_string(), ExtValue::Str("u-99".to_string()));
                    user.insert("email".to_string(), ExtValue::Str("ai@aion.dev".to_string()));
                    user.insert("verified".to_string(), ExtValue::Bool(true));
                    Ok(ExtValue::Map(user))
                } else {
                    Err(ExtRuntimeError::InvocationFailed {
                        module_id: self.manifest.id.clone(),
                        fn_name: fn_name.to_string(),
                        message: "invalid_token".to_string(),
                    })
                }
            }
            _ => Err(ExtRuntimeError::InvocationFailed {
                module_id: self.manifest.id.clone(),
                fn_name: fn_name.to_string(),
                message: "unsupported".to_string(),
            }),
        }
    }
}

struct MockMongoPlugin {
    manifest: ExtManifest,
    pts: Mutex<i32>,
}

impl ExtPlugin for MockMongoPlugin {
    fn manifest(&self) -> &ExtManifest {
        &self.manifest
    }

    fn invoke(&self, fn_name: &str, args: Vec<ExtValue>) -> Result<ExtValue, ExtRuntimeError> {
        match fn_name {
            "connect" => Ok(ExtValue::Void),
            "upd" => {
                let uid = match args.get(1) {
                    Some(ExtValue::Map(f)) => match f.get("uid") {
                        Some(ExtValue::Str(u)) => u.clone(),
                        _ => "unknown".to_string(),
                    },
                    _ => "unknown".to_string(),
                };
                let delta = match args.get(2) {
                    Some(ExtValue::Map(u)) => match u.get("$inc") {
                        Some(ExtValue::Map(inc)) => match inc.get("pts") {
                            Some(ExtValue::I32(d)) => *d,
                            _ => 0,
                        },
                        _ => 0,
                    },
                    _ => 0,
                };
                let mut total = self.pts.lock().unwrap();
                *total += delta;
                let mut user = BTreeMap::new();
                user.insert("id".to_string(), ExtValue::Str(format!("doc_{uid}")));
                user.insert("uid".to_string(), ExtValue::Str(uid));
                user.insert("pts".to_string(), ExtValue::I32(*total));
                Ok(ExtValue::Map(user))
            }
            _ => Err(ExtRuntimeError::InvocationFailed {
                module_id: self.manifest.id.clone(),
                fn_name: fn_name.to_string(),
                message: "unsupported".to_string(),
            }),
        }
    }
}

struct InMemoryRouterTransport {
    router: axum::Router,
}

#[async_trait]
impl UiHttpTransport for InMemoryRouterTransport {
    async fn send(&self, req: UiHttpRequest) -> Result<UiHttpResponse, String> {
        let method = match req.method {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        };
        let mut builder = Request::builder().method(method).uri(&req.path);
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let http_req = builder
            .body(Body::from(req.body))
            .map_err(|e| e.to_string())?;

        let resp = self
            .router
            .clone()
            .oneshot(http_req)
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(JSON_MIME)
            .to_string();
        let body = resp
            .into_body()
            .collect()
            .await
            .map_err(|e| e.to_string())?
            .to_bytes()
            .to_vec();

        Ok(UiHttpResponse {
            status,
            content_type,
            body,
        })
    }
}

struct FailingTransport;

#[async_trait]
impl UiHttpTransport for FailingTransport {
    async fn send(&self, _req: UiHttpRequest) -> Result<UiHttpResponse, String> {
        Err("connection refused".to_string())
    }
}

fn setup_in_memory_runtime() -> UiRuntime {
    let manifests = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mongo_manifest =
        ExtManifest::from_file(&manifests.join("aion-mongo-1.2.0.json")).unwrap();
    let auth_manifest =
        ExtManifest::from_file(&manifests.join("aion-firebase-auth-2.0.0.json")).unwrap();

    let mut reg = ExtRegistry::new();
    reg.register_plugin(Arc::new(MockAuthPlugin {
        manifest: auth_manifest,
    }))
    .unwrap();
    reg.register_plugin(Arc::new(MockMongoPlugin {
        manifest: mongo_manifest,
        pts: Mutex::new(0),
    }))
    .unwrap();

    let program = parse_program(FULLSTACK_EXAMPLE).unwrap();
    let config = RuntimeConfig::new()
        .with_env("MONGO_URI", "mongodb://localhost:27017/aion")
        .with_env("FB_PROJ_ID", "aion-e2e");

    let router = build_router(program.clone(), Arc::new(reg), config).unwrap();
    let transport = Arc::new(InMemoryRouterTransport { router });
    UiRuntime::new(program, transport).unwrap()
}

#[tokio::test]
async fn synthetic_tap_on_n3_calls_api0_via_protobuf_and_updates_s0() {
    let mut runtime = setup_in_memory_runtime();
    runtime.set_header("Authorization", "Bearer valid-jwt");

    assert_eq!(
        runtime.signals.get(&NodeId::new("#S0")),
        Some(&SignalValue::I32(0))
    );

    // First synthetic @tap on #N3 -> #API0 increments pts by 10 -> #S0 = 10
    let outcome = runtime
        .dispatch_event(&NodeId::new("#N3"), "tap")
        .await
        .unwrap();
    match outcome {
        DispatchOutcome::Ok { dirtied_nodes, .. } => {
            assert_eq!(dirtied_nodes, vec![NodeId::new("#N2")]);
        }
        other => panic!("expected DispatchOutcome::Ok, got {other:?}"),
    }

    assert_eq!(
        runtime.signals.get(&NodeId::new("#S0")),
        Some(&SignalValue::I32(10))
    );
    assert_eq!(runtime.signals.resolve_text("Puntos: {#S0}"), "Puntos: 10");
    assert!(runtime.loading_nodes.is_empty());
    assert!(runtime
        .events_log
        .contains(&UiLifecycleEvent::LoadingStarted(NodeId::new("#N3"))));
    assert!(runtime
        .events_log
        .contains(&UiLifecycleEvent::LoadingFinished(NodeId::new("#N3"))));

    // Second synthetic @tap on #N3 -> #S0 = 20
    runtime
        .dispatch_event(&NodeId::new("#N3"), "tap")
        .await
        .unwrap();
    assert_eq!(
        runtime.signals.get(&NodeId::new("#S0")),
        Some(&SignalValue::I32(20))
    );
    assert_eq!(runtime.signals.resolve_text("Puntos: {#S0}"), "Puntos: 20");
}

#[tokio::test]
async fn http_401_and_network_errors_trigger_err_toast_handler() {
    // 1. Missing Authorization header -> #MW0 returns 401 -> @err->TOAST("Error")
    let mut runtime = setup_in_memory_runtime();
    let outcome = runtime
        .dispatch_event(&NodeId::new("#N3"), "tap")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        DispatchOutcome::ErrorHandled {
            status: Some(401),
            message: "HTTP 401".to_string()
        }
    );
    assert_eq!(runtime.toasts, vec!["Error".to_string()]);
    assert_eq!(
        runtime.signals.get(&NodeId::new("#S0")),
        Some(&SignalValue::I32(0))
    );

    // 2. Transport-level network error -> @err->TOAST("Error")
    let program = parse_program(FULLSTACK_EXAMPLE).unwrap();
    let mut offline_rt = UiRuntime::new(program, Arc::new(FailingTransport)).unwrap();
    let net_outcome = offline_rt
        .dispatch_event(&NodeId::new("#N3"), "tap")
        .await
        .unwrap();
    assert_eq!(
        net_outcome,
        DispatchOutcome::ErrorHandled {
            status: None,
            message: "connection refused".to_string()
        }
    );
    assert_eq!(offline_rt.toasts, vec!["Error".to_string()]);
}
