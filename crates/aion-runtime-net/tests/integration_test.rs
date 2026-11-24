use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aion_ast::NodeId;
use aion_ext::{ExtManifest, ExtPlugin, ExtRegistry, ExtRuntimeError, ExtValue};
use aion_parser::parse_program;
use aion_runtime_net::{
    build_router, dynamic_msg_to_ext_value, ProtoSchema, RuntimeConfig, JSON_MIME, PROTOBUF_MIME,
};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use prost::Message as _;
use prost_reflect::{DynamicMessage, Value as ReflectValue};
use tower::ServiceExt;

const BACKEND_EXAMPLE: &str = r#"
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
                let token = match args.first() {
                    Some(ExtValue::Str(s)) => s.as_str(),
                    _ => "",
                };
                if token == "Bearer valid-jwt" {
                    let mut user = BTreeMap::new();
                    user.insert("uid".to_string(), ExtValue::Str("user-123".to_string()));
                    user.insert(
                        "email".to_string(),
                        ExtValue::Str("dev@aion.lang".to_string()),
                    );
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
            other => Err(ExtRuntimeError::InvocationFailed {
                module_id: self.manifest.id.clone(),
                fn_name: other.to_string(),
                message: "unknown function".to_string(),
            }),
        }
    }
}

struct MockMongoPlugin {
    manifest: ExtManifest,
    pts_store: Mutex<BTreeMap<String, i32>>,
}

impl ExtPlugin for MockMongoPlugin {
    fn manifest(&self) -> &ExtManifest {
        &self.manifest
    }

    fn invoke(&self, fn_name: &str, args: Vec<ExtValue>) -> Result<ExtValue, ExtRuntimeError> {
        match fn_name {
            "connect" => Ok(ExtValue::Void),
            "upd" => {
                // args: [collection, filter {uid}, update {$inc: {pts}}]
                let uid = match args.get(1) {
                    Some(ExtValue::Map(filter)) => match filter.get("uid") {
                        Some(ExtValue::Str(u)) => u.clone(),
                        _ => "unknown".to_string(),
                    },
                    _ => "unknown".to_string(),
                };
                let delta = match args.get(2) {
                    Some(ExtValue::Map(upd)) => match upd.get("$inc") {
                        Some(ExtValue::Map(inc)) => match inc.get("pts") {
                            Some(ExtValue::I32(d)) => *d,
                            _ => 0,
                        },
                        _ => 0,
                    },
                    _ => 0,
                };

                let mut store = self.pts_store.lock().unwrap();
                let entry = store.entry(uid.clone()).or_insert(0);
                *entry += delta;
                let total_pts = *entry;

                let mut user = BTreeMap::new();
                user.insert("id".to_string(), ExtValue::Str(format!("doc_{uid}")));
                user.insert("uid".to_string(), ExtValue::Str(uid));
                user.insert("pts".to_string(), ExtValue::I32(total_pts));
                Ok(ExtValue::Map(user))
            }
            other => Err(ExtRuntimeError::InvocationFailed {
                module_id: self.manifest.id.clone(),
                fn_name: other.to_string(),
                message: "not mocked".to_string(),
            }),
        }
    }
}

fn setup_test_router() -> (axum::Router, ProtoSchema) {
    let manifests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mongo_manifest =
        ExtManifest::from_file(&manifests_dir.join("aion-mongo-1.2.0.json")).unwrap();
    let auth_manifest =
        ExtManifest::from_file(&manifests_dir.join("aion-firebase-auth-2.0.0.json")).unwrap();

    let mut reg = ExtRegistry::new();
    reg.register_plugin(Arc::new(MockAuthPlugin {
        manifest: auth_manifest,
    }))
    .unwrap();
    reg.register_plugin(Arc::new(MockMongoPlugin {
        manifest: mongo_manifest,
        pts_store: Mutex::new(BTreeMap::new()),
    }))
    .unwrap();

    let program = parse_program(BACKEND_EXAMPLE).expect("example program should parse");
    let schema = ProtoSchema::from_program(&program).unwrap();
    let config = RuntimeConfig::new()
        .with_env("MONGO_URI", "mongodb://localhost:27017/aion")
        .with_env("FB_PROJ_ID", "aion-demo-proj");

    let router = build_router(program, Arc::new(reg), config).unwrap();
    (router, schema)
}

#[tokio::test]
async fn middleware_rejects_missing_authorization_with_401() {
    let (router, _) = setup_test_router();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/pts")
        .header(header::CONTENT_TYPE, JSON_MIME)
        .body(Body::from(r#"{"delta": 10}"#))
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 401);
    assert_eq!(json["error"], "invalid_token");
}

#[tokio::test]
async fn api_succeeds_with_json_and_binary_protobuf() {
    let (router, schema) = setup_test_router();

    // 1. JSON request/response
    let json_req = Request::builder()
        .method("POST")
        .uri("/v1/pts")
        .header(header::AUTHORIZATION, "Bearer valid-jwt")
        .header(header::CONTENT_TYPE, JSON_MIME)
        .header(header::ACCEPT, JSON_MIME)
        .body(Body::from(r#"{"delta": 10}"#))
        .unwrap();

    let json_resp = router.clone().oneshot(json_req).await.unwrap();
    assert_eq!(json_resp.status(), StatusCode::OK);
    assert_eq!(
        json_resp.headers().get(header::CONTENT_TYPE).unwrap(),
        JSON_MIME
    );
    let json_bytes = json_resp.into_body().collect().await.unwrap().to_bytes();
    let json_val: serde_json::Value = serde_json::from_slice(&json_bytes).unwrap();
    assert_eq!(json_val["id"], "doc_user-123");
    assert_eq!(json_val["uid"], "user-123");
    assert_eq!(json_val["pts"], 10);

    // 2. Binary Protobuf request/response (increments pts by 15 -> 25)
    let in_desc_name = schema.api_in_msg.get(&NodeId::new("#API0")).unwrap();
    let in_desc = schema.message_descriptor(in_desc_name).unwrap();
    let mut in_msg = DynamicMessage::new(in_desc);
    in_msg.set_field_by_name("delta", ReflectValue::I32(15));
    let proto_req_bytes = in_msg.encode_to_vec();

    let proto_req = Request::builder()
        .method("POST")
        .uri("/v1/pts")
        .header(header::AUTHORIZATION, "Bearer valid-jwt")
        .header(header::CONTENT_TYPE, PROTOBUF_MIME)
        .header(header::ACCEPT, PROTOBUF_MIME)
        .body(Body::from(proto_req_bytes))
        .unwrap();

    let proto_resp = router.clone().oneshot(proto_req).await.unwrap();
    assert_eq!(proto_resp.status(), StatusCode::OK);
    assert_eq!(
        proto_resp.headers().get(header::CONTENT_TYPE).unwrap(),
        PROTOBUF_MIME
    );
    let proto_resp_bytes = proto_resp.into_body().collect().await.unwrap().to_bytes();

    let out_desc_name = schema.api_out_msg.get(&NodeId::new("#API0")).unwrap();
    let out_desc = schema.message_descriptor(out_desc_name).unwrap();
    let decoded_msg = DynamicMessage::decode(out_desc, proto_resp_bytes).unwrap();
    let ExtValue::Map(decoded_map) = dynamic_msg_to_ext_value(&decoded_msg) else {
        panic!("expected decoded map");
    };
    assert_eq!(
        decoded_map.get("id"),
        Some(&ExtValue::Str("doc_user-123".to_string()))
    );
    assert_eq!(
        decoded_map.get("uid"),
        Some(&ExtValue::Str("user-123".to_string()))
    );
    assert_eq!(decoded_map.get("pts"), Some(&ExtValue::I32(25)));

    // 3. Schema inspection endpoint `GET /__aion/schema.proto`
    let schema_req = Request::builder()
        .method("GET")
        .uri("/__aion/schema.proto")
        .body(Body::empty())
        .unwrap();
    let schema_resp = router.oneshot(schema_req).await.unwrap();
    assert_eq!(schema_resp.status(), StatusCode::OK);
    let schema_text = String::from_utf8(
        schema_resp
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(schema_text.contains("message User {"));
    assert!(schema_text.contains("message Api0In {"));
}

#[tokio::test]
async fn supports_route_params_and_query_strings() {
    let src = r#"
#SRV0:HTTP[port:8080, proto:on]
#M0:FeedbackItem{id:str, status:str}
#API0:#SRV0.GET["/v1/feedback/:id"] OUT{#M0} {
  {id: ctx.param("id"), status: ctx.query("status")} -> RET
}
"#;
    let prog = parse_program(src).unwrap();
    let router = build_router(prog, Arc::new(ExtRegistry::new()), RuntimeConfig::new()).unwrap();

    let req = Request::builder()
        .method("GET")
        .uri("/v1/feedback/fb-42?status=open")
        .header(header::ACCEPT, JSON_MIME)
        .body(Body::empty())
        .unwrap();

    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["id"], "fb-42");
    assert_eq!(json["status"], "open");
}
