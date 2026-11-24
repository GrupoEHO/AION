use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aion_ext::{ExtManifest, ExtPlugin, ExtRuntimeError, ExtValue};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};

pub const GOOGLE_FIREBASE_JWKS_URL: &str =
    "https://www.googleapis.com/service_accounts/v1/jwk/securetoken@system.gserviceaccount.com";

const MANIFEST_JSON: &str = include_str!("../../../manifests/aion-firebase-auth-2.0.0.json");
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Deserialize)]
struct JwksResponse {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwkEntry {
    pub kid: String,
    pub n: String,
    pub e: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FirebaseClaims {
    sub: String,
    aud: String,
    iss: String,
    exp: usize,
    #[serde(default)]
    iat: usize,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
}

#[derive(Debug, Clone)]
enum JwksSource {
    Remote {
        url: String,
    },
    Static,
}

#[derive(Debug, Clone)]
struct JwksCache {
    keys: HashMap<String, (String, String)>,
    expires_at: Instant,
}

/// Real `ExtPlugin` implementation for `aion/firebase-auth@2.0.0`.
///
/// Validates Firebase ID tokens (RS256) against Google's JWKS endpoint with an in-memory cache,
/// and supports an offline static-key mode for deterministic unit/integration tests.
pub struct FirebaseAuthPlugin {
    manifest: ExtManifest,
    source: JwksSource,
    cache: RwLock<JwksCache>,
    revoked_uids: RwLock<HashSet<String>>,
    http: reqwest::Client,
}

impl FirebaseAuthPlugin {
    /// Creates a plugin configured against Google's official Firebase JWKS endpoint.
    pub fn new() -> Self {
        Self::with_jwks_url(GOOGLE_FIREBASE_JWKS_URL)
    }

    /// Creates a plugin configured against a custom JWKS endpoint URL.
    pub fn with_jwks_url(url: impl Into<String>) -> Self {
        let manifest =
            ExtManifest::from_json_str(MANIFEST_JSON).expect("embedded firebase manifest is valid");
        Self {
            manifest,
            source: JwksSource::Remote { url: url.into() },
            cache: RwLock::new(JwksCache {
                keys: HashMap::new(),
                expires_at: Instant::now(),
            }),
            revoked_uids: RwLock::new(HashSet::new()),
            http: reqwest::Client::new(),
        }
    }

    /// Creates an offline plugin backed by static RSA `(n, e)` base64url JWK components.
    pub fn with_static_jwks(keys: HashMap<String, (String, String)>) -> Self {
        let manifest =
            ExtManifest::from_json_str(MANIFEST_JSON).expect("embedded firebase manifest is valid");
        Self {
            manifest,
            source: JwksSource::Static,
            cache: RwLock::new(JwksCache {
                keys,
                expires_at: Instant::now() + Duration::from_secs(86400 * 365),
            }),
            revoked_uids: RwLock::new(HashSet::new()),
            http: reqwest::Client::new(),
        }
    }

    fn get_rsa_components(&self, kid: &str) -> Result<(String, String), String> {
        {
            let guard = self.cache.read().expect("jwks cache poisoned");
            let fresh = matches!(self.source, JwksSource::Static)
                || Instant::now() < guard.expires_at;
            if fresh {
                if let Some(pair) = guard.keys.get(kid) {
                    return Ok(pair.clone());
                }
                if matches!(self.source, JwksSource::Static) {
                    return Err("invalid_token".to_string());
                }
            }
        }

        match &self.source {
            JwksSource::Static => Err("invalid_token".to_string()),
            JwksSource::Remote { url } => {
                let (keys, ttl) = self.fetch_jwks(url)?;
                let mut guard = self.cache.write().expect("jwks cache poisoned");
                guard.keys = keys;
                guard.expires_at = Instant::now() + ttl;
                guard
                    .keys
                    .get(kid)
                    .cloned()
                    .ok_or_else(|| "invalid_token".to_string())
            }
        }
    }

    fn fetch_jwks(&self, url: &str) -> Result<(HashMap<String, (String, String)>, Duration), String> {
        let client = self.http.clone();
        let url = url.to_string();
        run_async(async move {
            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|_| "network_error".to_string())?;
            if !resp.status().is_success() {
                return Err("network_error".to_string());
            }
            let ttl = resp
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_max_age)
                .unwrap_or(DEFAULT_CACHE_TTL);

            let body: JwksResponse = resp
                .json()
                .await
                .map_err(|_| "network_error".to_string())?;
            let map = body
                .keys
                .into_iter()
                .map(|k| (k.kid, (k.n, k.e)))
                .collect();
            Ok((map, ttl))
        })
    }

    fn verify_token(&self, bearer: &str, project_id: &str) -> Result<ExtValue, String> {
        let token = bearer
            .trim()
            .strip_prefix("Bearer ")
            .or_else(|| bearer.trim().strip_prefix("bearer "))
            .unwrap_or(bearer.trim());

        if token.is_empty() {
            return Err("invalid_token".to_string());
        }

        let header = decode_header(token).map_err(|_| "invalid_token".to_string())?;
        if header.alg != Algorithm::RS256 {
            return Err("invalid_token".to_string());
        }
        let kid = header.kid.ok_or_else(|| "invalid_token".to_string())?;

        let (n, e) = self.get_rsa_components(&kid)?;
        let decoding_key =
            DecodingKey::from_rsa_components(&n, &e).map_err(|_| "invalid_token".to_string())?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[project_id]);
        validation.set_issuer(&[format!("https://securetoken.google.com/{project_id}")]);
        validation.validate_exp = true;
        validation.leeway = 0;

        let token_data = decode::<FirebaseClaims>(token, &decoding_key, &validation).map_err(
            |err| match err.kind() {
                ErrorKind::ExpiredSignature => "expired_token".to_string(),
                _ => "invalid_token".to_string(),
            },
        )?;

        let claims = token_data.claims;
        if claims.sub.is_empty() {
            return Err("invalid_token".to_string());
        }

        if self
            .revoked_uids
            .read()
            .expect("revoked_uids poisoned")
            .contains(&claims.sub)
        {
            return Err("unauthorized".to_string());
        }

        let mut user = BTreeMap::new();
        user.insert("uid".to_string(), ExtValue::Str(claims.sub));
        user.insert(
            "email".to_string(),
            ExtValue::Str(claims.email.unwrap_or_default()),
        );
        user.insert(
            "verified".to_string(),
            ExtValue::Bool(claims.email_verified.unwrap_or(false)),
        );
        Ok(ExtValue::Map(user))
    }

    fn revoke_uid(&self, uid: &str) -> Result<ExtValue, String> {
        if uid.trim().is_empty() {
            return Err("unauthorized".to_string());
        }
        self.revoked_uids
            .write()
            .expect("revoked_uids poisoned")
            .insert(uid.to_string());
        Ok(ExtValue::Void)
    }
}

impl Default for FirebaseAuthPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtPlugin for FirebaseAuthPlugin {
    fn manifest(&self) -> &ExtManifest {
        &self.manifest
    }

    fn invoke(&self, fn_name: &str, args: Vec<ExtValue>) -> Result<ExtValue, ExtRuntimeError> {
        let wrap_err = |msg: String| ExtRuntimeError::InvocationFailed {
            module_id: self.manifest.id.clone(),
            fn_name: fn_name.to_string(),
            message: msg,
        };

        match fn_name {
            "verify" => {
                if args.len() != 2 {
                    return Err(wrap_err(format!(
                        "expected 2 arguments (token, project_id), got {}",
                        args.len()
                    )));
                }
                let token = match &args[0] {
                    ExtValue::Str(s) => s.as_str(),
                    _ => return Err(wrap_err("invalid_token".to_string())),
                };
                let project_id = match &args[1] {
                    ExtValue::Str(s) => s.as_str(),
                    _ => return Err(wrap_err("invalid_token".to_string())),
                };
                self.verify_token(token, project_id).map_err(wrap_err)
            }
            "revoke" => {
                if args.len() != 1 {
                    return Err(wrap_err(format!(
                        "expected 1 argument (uid), got {}",
                        args.len()
                    )));
                }
                let uid = match &args[0] {
                    ExtValue::Str(s) => s.as_str(),
                    _ => return Err(wrap_err("unauthorized".to_string())),
                };
                self.revoke_uid(uid).map_err(wrap_err)
            }
            _ => Err(wrap_err(format!("unknown function `{fn_name}`"))),
        }
    }
}

fn parse_max_age(header_val: &str) -> Option<Duration> {
    for part in header_val.split(',') {
        let trimmed = part.trim();
        if let Some(secs) = trimmed.strip_prefix("max-age=") {
            if let Ok(n) = secs.trim().parse::<u64>() {
                return Some(Duration::from_secs(n));
            }
        }
    }
    None
}

fn run_async<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            tokio::runtime::RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| handle.block_on(fut))
            }
            _ => std::thread::scope(|s| {
                s.spawn(|| {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("tokio runtime");
                    rt.block_on(fut)
                })
                .join()
                .expect("async worker panicked")
            }),
        },
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(fut)
        }
    }
}

/// Offline RSA keypair helper for deterministic local JWT signing and testing.
pub struct LocalRsaKeyPair {
    pub kid: String,
    encoding_key: jsonwebtoken::EncodingKey,
    pub n_b64u: String,
    pub e_b64u: String,
}

impl LocalRsaKeyPair {
    /// Generates an in-memory 2048-bit RSA keypair and builds a paired offline [`FirebaseAuthPlugin`].
    pub fn generate(kid: impl Into<String>) -> (Self, FirebaseAuthPlugin) {
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::traits::PublicKeyParts;
        use rsa::{RsaPrivateKey, RsaPublicKey};

        let mut rng = rand::thread_rng();
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("generate 2048-bit RSA key");
        let pub_key = RsaPublicKey::from(&priv_key);

        let pem = priv_key
            .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
            .expect("encode pkcs1 pem");
        let encoding_key =
            jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).expect("rsa encoding key");

        let n_b64u = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e_b64u = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());
        let kid = kid.into();

        let mut keys = HashMap::new();
        keys.insert(kid.clone(), (n_b64u.clone(), e_b64u.clone()));
        let plugin = FirebaseAuthPlugin::with_static_jwks(keys);

        (
            Self {
                kid,
                encoding_key,
                n_b64u,
                e_b64u,
            },
            plugin,
        )
    }

    /// Signs a Firebase-formatted RS256 JWT with `exp_offset_secs` relative to current time.
    pub fn sign_jwt(
        &self,
        uid: &str,
        email: &str,
        email_verified: bool,
        project_id: &str,
        exp_offset_secs: i64,
    ) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let exp = (now + exp_offset_secs).max(0) as usize;
        let claims = FirebaseClaims {
            sub: uid.to_string(),
            aud: project_id.to_string(),
            iss: format!("https://securetoken.google.com/{project_id}"),
            exp,
            iat: now as usize,
            email: Some(email.to_string()),
            email_verified: Some(email_verified),
        };
        let mut header = jsonwebtoken::Header::new(Algorithm::RS256);
        header.kid = Some(self.kid.clone());
        jsonwebtoken::encode(&header, &claims, &self.encoding_key).expect("sign RS256 jwt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_ext::ExtRegistry;
    use std::sync::Arc;

    #[test]
    fn conforms_strictly_to_abi_manifest() {
        let plugin = FirebaseAuthPlugin::new();
        let manifest = plugin.manifest();
        manifest.validate().expect("manifest must satisfy ABI v1");
        assert_eq!(manifest.id, "aion/firebase-auth");
        assert_eq!(manifest.ver, "2.0.0");
        assert!(manifest.fns.contains_key("verify"));
        assert!(manifest.fns.contains_key("revoke"));
        assert!(manifest.types.contains_key("AuthUser"));

        let mut reg = ExtRegistry::new();
        reg.register_plugin(Arc::new(plugin)).unwrap();
        assert!(reg.resolve("aion/firebase-auth", "2.0").is_some());
    }

    #[test]
    fn offline_rs256_verify_and_revoke_flow() {
        let (keys, plugin) = LocalRsaKeyPair::generate("test-kid-1");
        let mut reg = ExtRegistry::new();
        reg.register_plugin(Arc::new(plugin)).unwrap();

        let jwt = keys.sign_jwt("uid-42", "ai@aion.dev", true, "aion-proj", 300);

        // Valid Bearer token returns `@AuthUser` map matching manifest fields.
        let res = reg
            .invoke(
                "aion/firebase-auth",
                "2.0",
                "verify",
                vec![
                    ExtValue::Str(format!("Bearer {jwt}")),
                    ExtValue::Str("aion-proj".to_string()),
                ],
            )
            .expect("valid jwt should verify");

        let ExtValue::Map(user) = res else {
            panic!("expected AuthUser map");
        };
        assert_eq!(user.get("uid"), Some(&ExtValue::Str("uid-42".to_string())));
        assert_eq!(
            user.get("email"),
            Some(&ExtValue::Str("ai@aion.dev".to_string()))
        );
        assert_eq!(user.get("verified"), Some(&ExtValue::Bool(true)));

        // Wrong project_id -> invalid_token
        let wrong_aud = reg
            .invoke(
                "aion/firebase-auth",
                "2.0",
                "verify",
                vec![
                    ExtValue::Str(jwt.clone()),
                    ExtValue::Str("other-proj".to_string()),
                ],
            )
            .unwrap_err();
        assert!(wrong_aud.to_string().contains("invalid_token"));

        // Expired token -> expired_token
        let expired_jwt = keys.sign_jwt("uid-42", "ai@aion.dev", true, "aion-proj", -60);
        let expired_err = reg
            .invoke(
                "aion/firebase-auth",
                "2.0",
                "verify",
                vec![
                    ExtValue::Str(expired_jwt),
                    ExtValue::Str("aion-proj".to_string()),
                ],
            )
            .unwrap_err();
        assert!(expired_err.to_string().contains("expired_token"));

        // Revoke UID -> subsequent verify returns unauthorized
        let revoke_res = reg
            .invoke(
                "aion/firebase-auth",
                "2.0",
                "revoke",
                vec![ExtValue::Str("uid-42".to_string())],
            )
            .expect("revoke should succeed");
        assert_eq!(revoke_res, ExtValue::Void);

        let revoked_err = reg
            .invoke(
                "aion/firebase-auth",
                "2.0",
                "verify",
                vec![
                    ExtValue::Str(jwt),
                    ExtValue::Str("aion-proj".to_string()),
                ],
            )
            .unwrap_err();
        assert!(revoked_err.to_string().contains("unauthorized"));
    }
}
