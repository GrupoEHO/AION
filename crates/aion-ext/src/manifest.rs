use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use aion_ast::AionType;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Runtime capabilities declared by an external `#EXT` module (per `ext-abi-v1.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtCapability {
    Net,
    Env,
    FsRead,
    FsWrite,
    Crypto,
    Clock,
    Ui,
}

/// Positional function argument descriptor in an ABI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtArg {
    /// Argument name (`n`).
    pub n: String,
    /// Argument type (`t`).
    pub t: String,
    /// Whether this trailing argument is optional (`opt`).
    #[serde(default)]
    pub opt: bool,
}

/// Function signature declared in an `#EXT` ABI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtFnSig {
    pub args: Vec<ExtArg>,
    pub ret: String,
    #[serde(rename = "async")]
    pub is_async: bool,
    #[serde(default)]
    pub errs: Vec<String>,
}

impl ExtFnSig {
    pub fn format_signature(&self, fn_name: &str) -> String {
        let params: Vec<String> = self
            .args
            .iter()
            .map(|a| {
                if a.opt {
                    format!("{}?: {}", a.n, a.t)
                } else {
                    format!("{}: {}", a.n, a.t)
                }
            })
            .collect();
        let async_prefix = if self.is_async { "async " } else { "" };
        format!("{async_prefix}{fn_name}({}) -> {}", params.join(", "), self.ret)
    }
}

/// External native UI widget descriptor declared in an `#EXT` ABI manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtWidgetSpec {
    pub args: Vec<ExtArg>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub desc: Option<String>,
}

/// AION External Module ABI v1 Manifest (`https://aion.dev/schemas/ext-abi-v1.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtManifest {
    pub abi: u32,
    pub id: String,
    pub ver: String,
    pub caps: Vec<ExtCapability>,
    #[serde(default)]
    pub types: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub errs: Vec<String>,
    #[serde(default)]
    pub fns: BTreeMap<String, ExtFnSig>,
    #[serde(default)]
    pub widgets: BTreeMap<String, ExtWidgetSpec>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ManifestError {
    #[error("failed to read manifest file: {0}")]
    Io(String),
    #[error("invalid manifest JSON: {0}")]
    Json(String),
    #[error("unsupported ABI version {0}, expected 1")]
    UnsupportedAbi(u32),
    #[error("invalid module id `{0}`, expected `org/pkg` lowercase alphanumeric with hyphens")]
    InvalidModuleId(String),
    #[error("invalid semver `{0}`, expected `MAJOR.MINOR.PATCH`")]
    InvalidVersion(String),
    #[error("invalid type `{ty}` in `{context}`: {reason}")]
    InvalidType {
        context: String,
        ty: String,
        reason: String,
    },
    #[error("unknown `@Type` reference `{type_name}` in `{context}`")]
    UnknownExtType { context: String, type_name: String },
    #[error("function `{fn_name}` declares undeclared error code `{err_code}`")]
    UndeclaredErrorCode { fn_name: String, err_code: String },
    #[error("function `{fn_name}` has required argument `{arg_name}` after optional argument")]
    RequiredAfterOptional { fn_name: String, arg_name: String },
}

impl ExtManifest {
    pub fn from_json_str(raw: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_json::from_str(raw).map_err(|e| ManifestError::Json(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let raw = fs::read_to_string(path).map_err(|e| ManifestError::Io(e.to_string()))?;
        Self::from_json_str(&raw)
    }

    /// Validates all structural and semantic constraints from `ext-abi-v1.json`.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.abi != 1 {
            return Err(ManifestError::UnsupportedAbi(self.abi));
        }
        if !is_valid_module_id(&self.id) {
            return Err(ManifestError::InvalidModuleId(self.id.clone()));
        }
        if parse_semver(&self.ver).is_none() {
            return Err(ManifestError::InvalidVersion(self.ver.clone()));
        }

        // Validate custom types fields.
        for (type_name, fields) in &self.types {
            for (field_name, field_ty) in fields {
                self.validate_type_ref(field_ty, &format!("types.{type_name}.{field_name}"))?;
            }
        }

        let allowed_errs: BTreeSet<&str> = self.errs.iter().map(String::as_str).collect();

        for (fn_name, sig) in &self.fns {
            let mut seen_optional = false;
            for arg in &sig.args {
                if arg.opt {
                    seen_optional = true;
                } else if seen_optional {
                    return Err(ManifestError::RequiredAfterOptional {
                        fn_name: fn_name.clone(),
                        arg_name: arg.n.clone(),
                    });
                }
                self.validate_type_ref(&arg.t, &format!("fns.{fn_name}.args.{}", arg.n))?;
            }
            self.validate_type_ref(&sig.ret, &format!("fns.{fn_name}.ret"))?;

            if !allowed_errs.is_empty() {
                for err_code in &sig.errs {
                    if !allowed_errs.contains(err_code.as_str()) {
                        return Err(ManifestError::UndeclaredErrorCode {
                            fn_name: fn_name.clone(),
                            err_code: err_code.clone(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    fn validate_type_ref(&self, raw_ty: &str, context: &str) -> Result<(), ManifestError> {
        let parsed = AionType::parse(raw_ty).map_err(|reason| ManifestError::InvalidType {
            context: context.to_string(),
            ty: raw_ty.to_string(),
            reason,
        })?;
        self.check_ext_types_exist(&parsed, context)
    }

    fn check_ext_types_exist(&self, ty: &AionType, context: &str) -> Result<(), ManifestError> {
        match ty {
            AionType::ExtType { name, .. } => {
                if !self.types.contains_key(name) {
                    return Err(ManifestError::UnknownExtType {
                        context: context.to_string(),
                        type_name: name.clone(),
                    });
                }
                Ok(())
            }
            AionType::List(inner) => self.check_ext_types_exist(inner, context),
            AionType::Map(k, v) => {
                self.check_ext_types_exist(k, context)?;
                self.check_ext_types_exist(v, context)
            }
            _ => Ok(()),
        }
    }
}

pub(crate) fn is_valid_module_id(id: &str) -> bool {
    let Some((org, pkg)) = id.split_once('/') else {
        return false;
    };
    !org.is_empty()
        && !pkg.is_empty()
        && org
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && pkg
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub(crate) fn parse_semver(ver: &str) -> Option<(u64, u64, u64)> {
    let mut parts = ver.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub(crate) fn version_matches(version_req: &str, candidate_ver: &str) -> bool {
    let Some((cmaj, cmin, cpat)) = parse_semver(candidate_ver) else {
        return false;
    };
    let req = version_req.trim().trim_start_matches('^').trim_start_matches('~');
    let parts: Vec<&str> = req.split('.').collect();
    match parts.as_slice() {
        [maj] => maj.parse::<u64>().ok() == Some(cmaj),
        [maj, min] => {
            maj.parse::<u64>().ok() == Some(cmaj) && min.parse::<u64>().ok() == Some(cmin)
        }
        [maj, min, pat] => {
            maj.parse::<u64>().ok() == Some(cmaj)
                && min.parse::<u64>().ok() == Some(cmin)
                && pat.parse::<u64>().ok() == Some(cpat)
        }
        _ => false,
    }
}
