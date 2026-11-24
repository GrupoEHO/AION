use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use thiserror::Error;

use crate::manifest::{parse_semver, version_matches, ExtManifest, ManifestError};

/// Runtime value passed across the AION `#EXT` ABI boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtValue {
    I32(i32),
    F64(f64),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
    Ts(i64),
    List(Vec<ExtValue>),
    Map(BTreeMap<String, ExtValue>),
    Void,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExtRuntimeError {
    #[error("external module `{module_id}@{version}` is not registered")]
    ModuleNotFound { module_id: String, version: String },
    #[error("external module `{module_id}` is manifest-only and has no runtime backend loaded")]
    NoRuntimeBackend { module_id: String },
    #[error("function `{fn_name}` failed in `{module_id}`: {message}")]
    InvocationFailed {
        module_id: String,
        fn_name: String,
        message: String,
    },
}

/// Rust-native plugin trait for `#EXT` modules (FFI/trait bridge).
pub trait ExtPlugin: Send + Sync {
    fn manifest(&self) -> &ExtManifest;
    fn invoke(&self, fn_name: &str, args: Vec<ExtValue>) -> Result<ExtValue, ExtRuntimeError>;
}

/// Prepared descriptor for C-ABI shared-library plugins (`.dylib` / `.so` / `.dll`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiPluginDescriptor {
    pub symbol_prefix: String,
    pub library_path: String,
}

/// Prepared descriptor for WebAssembly Component Model / WIT plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmComponentDescriptor {
    pub wit_package: String,
    pub component_path: Option<String>,
}

/// Execution backend attached to an `#EXT` manifest.
pub enum ExtBackend {
    /// Compile-time contract verification only (JSON manifest loaded).
    ManifestOnly,
    /// In-process Rust crate plugin via `ExtPlugin` trait.
    NativeRust(Arc<dyn ExtPlugin>),
    /// C ABI FFI dynamic plugin descriptor (ready for `libloading` integration).
    FfiCAbi(FfiPluginDescriptor),
    /// WebAssembly Component / WIT descriptor (ready for `wasmtime` component integration).
    WasmComponent(WasmComponentDescriptor),
}

pub struct RegisteredExt {
    pub manifest: ExtManifest,
    pub backend: ExtBackend,
}

/// Registry of `#EXT` ABI manifests and optional runtime plugin backends.
#[derive(Default)]
pub struct ExtRegistry {
    /// Keyed by `(module_id, exact_semver)`.
    entries: BTreeMap<(String, String), RegisteredExt>,
}

impl ExtRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_manifest(&mut self, manifest: ExtManifest) -> Result<(), ManifestError> {
        manifest.validate()?;
        let key = (manifest.id.clone(), manifest.ver.clone());
        self.entries.insert(
            key,
            RegisteredExt {
                manifest,
                backend: ExtBackend::ManifestOnly,
            },
        );
        Ok(())
    }

    pub fn register_manifest_json(&mut self, raw_json: &str) -> Result<(), ManifestError> {
        let manifest = ExtManifest::from_json_str(raw_json)?;
        self.register_manifest(manifest)
    }

    pub fn register_manifest_file(&mut self, path: &Path) -> Result<(), ManifestError> {
        let manifest = ExtManifest::from_file(path)?;
        self.register_manifest(manifest)
    }

    pub fn register_plugin(&mut self, plugin: Arc<dyn ExtPlugin>) -> Result<(), ManifestError> {
        let manifest = plugin.manifest().clone();
        manifest.validate()?;
        let key = (manifest.id.clone(), manifest.ver.clone());
        self.entries.insert(
            key,
            RegisteredExt {
                manifest,
                backend: ExtBackend::NativeRust(plugin),
            },
        );
        Ok(())
    }

    pub fn register_wasm_descriptor(
        &mut self,
        manifest: ExtManifest,
        descriptor: WasmComponentDescriptor,
    ) -> Result<(), ManifestError> {
        manifest.validate()?;
        let key = (manifest.id.clone(), manifest.ver.clone());
        self.entries.insert(
            key,
            RegisteredExt {
                manifest,
                backend: ExtBackend::WasmComponent(descriptor),
            },
        );
        Ok(())
    }

    /// Resolves the highest compatible manifest matching `module_id` and `version_req` (e.g. `1.2` -> `1.2.0`).
    pub fn resolve(&self, module_id: &str, version_req: &str) -> Option<&ExtManifest> {
        self.resolve_entry(module_id, version_req)
            .map(|e| &e.manifest)
    }

    pub fn resolve_entry(&self, module_id: &str, version_req: &str) -> Option<&RegisteredExt> {
        self.entries
            .iter()
            .filter(|((id, ver), _)| id == module_id && version_matches(version_req, ver))
            .max_by_key(|((_, ver), _)| parse_semver(ver))
            .map(|(_, entry)| entry)
    }

    pub fn invoke(
        &self,
        module_id: &str,
        version_req: &str,
        fn_name: &str,
        args: Vec<ExtValue>,
    ) -> Result<ExtValue, ExtRuntimeError> {
        let entry = self
            .resolve_entry(module_id, version_req)
            .ok_or_else(|| ExtRuntimeError::ModuleNotFound {
                module_id: module_id.to_string(),
                version: version_req.to_string(),
            })?;

        match &entry.backend {
            ExtBackend::NativeRust(plugin) => plugin.invoke(fn_name, args),
            _ => Err(ExtRuntimeError::NoRuntimeBackend {
                module_id: module_id.to_string(),
            }),
        }
    }

    pub fn list_modules(&self) -> Vec<(&str, &str)> {
        self.entries
            .keys()
            .map(|(id, ver)| (id.as_str(), ver.as_str()))
            .collect()
    }
}
