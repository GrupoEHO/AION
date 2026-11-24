pub mod eval;
pub mod proto;
pub mod server;

use std::collections::HashMap;
use thiserror::Error;

pub use eval::{eval_api, eval_middleware, EvalOutcome, RequestContext};
pub use proto::{
    dynamic_msg_to_ext_value, ext_value_to_json, generate_proto3, json_to_ext_value, ProtoSchema,
};
pub use server::{build_router, serve, JSON_MIME, PROTOBUF_MIME};

/// Runtime environment/config bindings injected for `#CFG:ENV{...}` lookups.
#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    pub env: HashMap<String, String>,
}

impl RuntimeConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.insert(key.into(), val.into());
        self
    }

    pub fn get_env(&self, key: &str) -> Option<String> {
        self.env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuntimeNetError {
    #[error("evaluation error: {0}")]
    Eval(String),
    #[error("external plugin error: {0}")]
    Ext(String),
    #[error("protobuf schema/codec error: {0}")]
    Proto(String),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("I/O error: {0}")]
    Io(String),
}
