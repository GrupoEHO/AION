use std::collections::HashMap;
use std::sync::Arc;

use aion_ast::{ApiNode, HttpMethod, MwNode, NodeId, NodeKind, Program, PropValue};
use aion_ext::{ExtRegistry, ExtValue};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::Router;

use crate::eval::{eval_api, eval_middleware, EvalOutcome, RequestContext};
use crate::proto::{
    ext_value_to_json, json_bytes_to_input_map, ProtoSchema,
};
use crate::{RuntimeConfig, RuntimeNetError};

pub const PROTOBUF_MIME: &str = "application/x-protobuf";
pub const JSON_MIME: &str = "application/json";

#[derive(Clone)]
struct AppState {
    program: Arc<Program>,
    registry: Arc<ExtRegistry>,
    config: Arc<RuntimeConfig>,
    schema: Arc<ProtoSchema>,
    middlewares: Arc<HashMap<NodeId, MwNode>>,
    apis: Arc<HashMap<NodeId, ApiNode>>,
}

/// Builds a dynamic `axum::Router` from the `#SRV`, `#MW`, `#M`, and `#API` nodes of `program`.
pub fn build_router(
    program: Program,
    registry: Arc<ExtRegistry>,
    config: RuntimeConfig,
) -> Result<Router, RuntimeNetError> {
    let schema = Arc::new(ProtoSchema::from_program(&program)?);

    let mut middlewares = HashMap::new();
    let mut apis = HashMap::new();
    let mut routes: Vec<(NodeId, HttpMethod, String)> = Vec::new();

    for node in &program.nodes {
        match &node.kind {
            NodeKind::Mw(mw) => {
                middlewares.insert(node.id.clone(), mw.clone());
            }
            NodeKind::Api(api) => {
                routes.push((node.id.clone(), api.method, api.path.clone()));
                apis.insert(node.id.clone(), api.clone());
            }
            _ => {}
        }
    }

    let state = AppState {
        program: Arc::new(program),
        registry,
        config: Arc::new(config),
        schema,
        middlewares: Arc::new(middlewares),
        apis: Arc::new(apis),
    };

    let mut router = Router::new().route("/__aion/schema.proto", get(serve_proto_schema));

    for (api_id, method, path) in routes {
        let handler = move |state: State<AppState>, headers: HeaderMap, body: Bytes| {
            let api_id = api_id.clone();
            async move { handle_api_request(state, api_id, headers, body).await }
        };

        let method_router = match method {
            HttpMethod::Get => get(handler),
            HttpMethod::Post => post(handler),
            HttpMethod::Put => put(handler),
            HttpMethod::Patch => patch(handler),
            HttpMethod::Delete => delete(handler),
        };
        router = router.route(&path, method_router);
    }

    Ok(router.with_state(state))
}

/// Extracts the configured port from the first `#SRV` node (defaulting to `8080`)
/// and runs the `axum` HTTP server with `tokio`.
pub async fn serve(
    program: Program,
    registry: Arc<ExtRegistry>,
    config: RuntimeConfig,
) -> Result<(), RuntimeNetError> {
    let port = extract_server_port(&program).unwrap_or(8080);
    let router = build_router(program, registry, config)?;
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| RuntimeNetError::Io(format!("failed to bind {addr}: {e}")))?;
    axum::serve(listener, router)
        .await
        .map_err(|e| RuntimeNetError::Io(format!("server error: {e}")))
}

fn extract_server_port(program: &Program) -> Option<u16> {
    for node in &program.nodes {
        if let NodeKind::Srv(srv) = &node.kind {
            for prop in &srv.props {
                if prop.key == "port" {
                    if let PropValue::Int(p) = prop.value {
                        return u16::try_from(p).ok();
                    }
                }
            }
        }
    }
    None
}

async fn serve_proto_schema(State(state): State<AppState>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        state.schema.proto_text.clone(),
    )
        .into_response()
}

async fn handle_api_request(
    State(state): State<AppState>,
    api_id: NodeId,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let wants_proto = prefers_protobuf(&headers);
    let req_is_proto = is_protobuf_content_type(&headers);

    let Some(api) = state.apis.get(&api_id) else {
        return error_response(
            &state.schema,
            wants_proto,
            StatusCode::NOT_FOUND,
            "unknown API endpoint",
        );
    };

    let mut ctx = RequestContext::from_headers(&headers);

    // 1. Run `#MW` middleware pipeline.
    for mw_id in &api.middlewares {
        let Some(mw) = state.middlewares.get(mw_id) else {
            return error_response(
                &state.schema,
                wants_proto,
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("missing middleware `{mw_id}`"),
            );
        };
        match eval_middleware(
            &state.program,
            &state.registry,
            &state.config,
            &mw.ctx_param,
            &mw.body,
            &mut ctx,
        ) {
            Ok(EvalOutcome::Next) => {}
            Ok(EvalOutcome::Err { status, message }) => {
                let code = StatusCode::from_u16(status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                return error_response(&state.schema, wants_proto, code, &message);
            }
            Ok(EvalOutcome::Ret(_)) => break,
            Err(err) => {
                return error_response(
                    &state.schema,
                    wants_proto,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &err.to_string(),
                );
            }
        }
    }

    // 2. Decode request body (`IN{...}`).
    let input_map = if req_is_proto {
        match state.schema.decode_input(&api_id, &body) {
            Ok(m) => m,
            Err(err) => {
                return error_response(
                    &state.schema,
                    wants_proto,
                    StatusCode::BAD_REQUEST,
                    &err.to_string(),
                );
            }
        }
    } else {
        match json_bytes_to_input_map(&body, &api.input_fields) {
            Ok(m) => m,
            Err(err) => {
                return error_response(
                    &state.schema,
                    wants_proto,
                    StatusCode::BAD_REQUEST,
                    &err.to_string(),
                );
            }
        }
    };

    // 3. Evaluate `#API` body.
    match eval_api(
        &state.program,
        &state.registry,
        &state.config,
        &api.body,
        &mut ctx,
        &input_map,
    ) {
        Ok(EvalOutcome::Ret(val)) => {
            encode_success_response(&state.schema, &api_id, wants_proto, &val)
        }
        Ok(EvalOutcome::Next) => {
            encode_success_response(&state.schema, &api_id, wants_proto, &ExtValue::Void)
        }
        Ok(EvalOutcome::Err { status, message }) => {
            let code =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            error_response(&state.schema, wants_proto, code, &message)
        }
        Err(err) => error_response(
            &state.schema,
            wants_proto,
            StatusCode::INTERNAL_SERVER_ERROR,
            &err.to_string(),
        ),
    }
}

fn encode_success_response(
    schema: &ProtoSchema,
    api_id: &NodeId,
    wants_proto: bool,
    val: &ExtValue,
) -> Response {
    if wants_proto {
        match schema.encode_output(api_id, val) {
            Ok(bytes) => {
                let mut resp = (StatusCode::OK, bytes).into_response();
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(PROTOBUF_MIME),
                );
                resp
            }
            Err(err) => error_response(
                schema,
                false,
                StatusCode::INTERNAL_SERVER_ERROR,
                &err.to_string(),
            ),
        }
    } else {
        let json = ext_value_to_json(val);
        let body = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
        let mut resp = (StatusCode::OK, body).into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON_MIME));
        resp
    }
}

fn error_response(
    schema: &ProtoSchema,
    wants_proto: bool,
    status: StatusCode,
    message: &str,
) -> Response {
    if wants_proto {
        if let Ok(bytes) = schema.encode_error(status.as_u16(), message) {
            let mut resp = (status, bytes).into_response();
            resp.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(PROTOBUF_MIME),
            );
            return resp;
        }
    }
    let payload = serde_json::json!({
        "status": status.as_u16(),
        "error": message,
    });
    let body = serde_json::to_vec(&payload).unwrap_or_default();
    let mut resp = (status, body).into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON_MIME));
    resp
}

fn is_protobuf_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("x-protobuf") || ct.contains("application/protobuf"))
        .unwrap_or(false)
}

fn prefers_protobuf(headers: &HeaderMap) -> bool {
    if let Some(accept) = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()) {
        if accept.contains("x-protobuf") || accept.contains("application/protobuf") {
            return true;
        }
        if accept.contains("application/json") {
            return false;
        }
    }
    is_protobuf_content_type(headers)
}
