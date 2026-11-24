use std::collections::BTreeMap;
use std::sync::RwLock;

use aion_ext::{ExtManifest, ExtPlugin, ExtRuntimeError, ExtValue};
use mongodb::bson::spec::BinarySubtype;
use mongodb::bson::{Binary, Bson, DateTime, Document};
use mongodb::options::{ClientOptions, FindOneAndUpdateOptions, ReturnDocument};
use mongodb::{Client, Database};

const MANIFEST_JSON: &str = include_str!("../../manifests/aion-mongo-1.2.0.json");

struct ConnectedMongo {
    client: Client,
    db_name: String,
}

/// Real `ExtPlugin` implementation for `aion/mongo@1.2.0` backed by the official `mongodb` driver.
pub struct MongoPlugin {
    manifest: ExtManifest,
    rt: tokio::runtime::Runtime,
    conn: RwLock<Option<ConnectedMongo>>,
}

impl MongoPlugin {
    pub fn new() -> Self {
        let manifest =
            ExtManifest::from_json_str(MANIFEST_JSON).expect("embedded mongo manifest is valid");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("create mongo tokio runtime");
        Self {
            manifest,
            rt,
            conn: RwLock::new(None),
        }
    }

    fn run_async<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send,
        T: Send,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|s| {
                s.spawn(|| self.rt.block_on(fut))
                    .join()
                    .expect("mongo async worker panicked")
            })
        } else {
            self.rt.block_on(fut)
        }
    }

    fn database(&self) -> Result<Database, String> {
        let guard = self.conn.read().expect("mongo conn lock poisoned");
        let connected = guard
            .as_ref()
            .ok_or_else(|| "connection_failed".to_string())?;
        Ok(connected.client.database(&connected.db_name))
    }

    fn op_connect(&self, uri: &str) -> Result<ExtValue, String> {
        let uri = uri.to_string();
        let connected = self.run_async(async move {
            let options = ClientOptions::parse(&uri)
                .await
                .map_err(|_| "connection_failed".to_string())?;
            let db_name = options
                .default_database
                .clone()
                .unwrap_or_else(|| "aion".to_string());
            let client =
                Client::with_options(options).map_err(|_| "connection_failed".to_string())?;
            Ok::<ConnectedMongo, String>(ConnectedMongo { client, db_name })
        })?;

        *self.conn.write().expect("mongo conn lock poisoned") = Some(connected);
        Ok(ExtValue::Void)
    }

    fn op_find_one(&self, collection: &str, filter: Document) -> Result<ExtValue, String> {
        let db = self.database()?;
        let col_name = collection.to_string();
        self.run_async(async move {
            let col = db.collection::<Document>(&col_name);
            let doc_opt = col
                .find_one(filter)
                .await
                .map_err(map_mongo_err)?;
            match doc_opt {
                Some(doc) => Ok(bson_doc_to_ext_value(&doc)),
                None => Err("not_found".to_string()),
            }
        })
    }

    fn op_ins(&self, collection: &str, doc: Document) -> Result<ExtValue, String> {
        let db = self.database()?;
        let col_name = collection.to_string();
        self.run_async(async move {
            let col = db.collection::<Document>(&col_name);
            let res = col.insert_one(doc).await.map_err(map_mongo_err)?;
            let id_str = match res.inserted_id {
                Bson::ObjectId(oid) => oid.to_hex(),
                Bson::String(s) => s,
                other => other.to_string(),
            };
            let mut out = BTreeMap::new();
            out.insert("id".to_string(), ExtValue::Str(id_str));
            Ok(ExtValue::Map(out))
        })
    }

    fn op_upd(
        &self,
        collection: &str,
        filter: Document,
        update: Document,
    ) -> Result<ExtValue, String> {
        let db = self.database()?;
        let col_name = collection.to_string();
        self.run_async(async move {
            let col = db.collection::<Document>(&col_name);
            let opts = FindOneAndUpdateOptions::builder()
                .return_document(ReturnDocument::After)
                .build();
            let doc_opt = col
                .find_one_and_update(filter, update)
                .with_options(opts)
                .await
                .map_err(map_mongo_err)?;
            match doc_opt {
                Some(doc) => Ok(bson_doc_to_ext_value(&doc)),
                None => Err("not_found".to_string()),
            }
        })
    }

    fn op_del(&self, collection: &str, filter: Document) -> Result<ExtValue, String> {
        let db = self.database()?;
        let col_name = collection.to_string();
        self.run_async(async move {
            let col = db.collection::<Document>(&col_name);
            let res = col.delete_many(filter).await.map_err(map_mongo_err)?;
            Ok(ExtValue::I32(res.deleted_count as i32))
        })
    }
}

impl Default for MongoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtPlugin for MongoPlugin {
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
            "connect" => {
                if args.len() != 1 {
                    return Err(wrap_err(format!("expected 1 argument (uri), got {}", args.len())));
                }
                let uri = expect_str(&args[0], "uri").map_err(wrap_err)?;
                self.op_connect(uri).map_err(wrap_err)
            }
            "find_one" => {
                if args.len() != 2 {
                    return Err(wrap_err(format!(
                        "expected 2 arguments (collection, filter), got {}",
                        args.len()
                    )));
                }
                let col = expect_str(&args[0], "collection").map_err(wrap_err)?;
                let filter = ext_value_to_doc(&args[1]).map_err(wrap_err)?;
                self.op_find_one(col, filter).map_err(wrap_err)
            }
            "ins" => {
                if args.len() != 2 {
                    return Err(wrap_err(format!(
                        "expected 2 arguments (collection, doc), got {}",
                        args.len()
                    )));
                }
                let col = expect_str(&args[0], "collection").map_err(wrap_err)?;
                let doc = ext_value_to_doc(&args[1]).map_err(wrap_err)?;
                self.op_ins(col, doc).map_err(wrap_err)
            }
            "upd" => {
                if args.len() != 3 {
                    return Err(wrap_err(format!(
                        "expected 3 arguments (collection, filter, update), got {}",
                        args.len()
                    )));
                }
                let col = expect_str(&args[0], "collection").map_err(wrap_err)?;
                let filter = ext_value_to_doc(&args[1]).map_err(wrap_err)?;
                let update = ext_value_to_doc(&args[2]).map_err(wrap_err)?;
                self.op_upd(col, filter, update).map_err(wrap_err)
            }
            "del" => {
                if args.len() != 2 {
                    return Err(wrap_err(format!(
                        "expected 2 arguments (collection, filter), got {}",
                        args.len()
                    )));
                }
                let col = expect_str(&args[0], "collection").map_err(wrap_err)?;
                let filter = ext_value_to_doc(&args[1]).map_err(wrap_err)?;
                self.op_del(col, filter).map_err(wrap_err)
            }
            _ => Err(wrap_err(format!("unknown function `{fn_name}`"))),
        }
    }
}

fn expect_str<'a>(val: &'a ExtValue, name: &str) -> Result<&'a str, String> {
    match val {
        ExtValue::Str(s) => Ok(s.as_str()),
        _ => Err(format!("invalid_query: `{name}` must be str")),
    }
}

fn map_mongo_err(err: mongodb::error::Error) -> String {
    if err.is_network_timeout() {
        "timeout".to_string()
    } else if err.is_network_error() {
        "connection_failed".to_string()
    } else {
        "invalid_query".to_string()
    }
}

/// Translates an AION [`ExtValue::Map`] into a BSON [`Document`].
pub fn ext_value_to_doc(val: &ExtValue) -> Result<Document, String> {
    match val {
        ExtValue::Map(map) => {
            let mut doc = Document::new();
            for (k, v) in map {
                doc.insert(k.clone(), ext_value_to_bson(v));
            }
            Ok(doc)
        }
        _ => Err("invalid_query".to_string()),
    }
}

/// Translates any AION [`ExtValue`] dynamically into [`Bson`].
pub fn ext_value_to_bson(val: &ExtValue) -> Bson {
    match val {
        ExtValue::I32(n) => Bson::Int32(*n),
        ExtValue::F64(f) => Bson::Double(*f),
        ExtValue::Str(s) => Bson::String(s.clone()),
        ExtValue::Bool(b) => Bson::Boolean(*b),
        ExtValue::Bytes(bytes) => Bson::Binary(Binary {
            subtype: BinarySubtype::Generic,
            bytes: bytes.clone(),
        }),
        ExtValue::Ts(ms) => Bson::DateTime(DateTime::from_millis(*ms)),
        ExtValue::List(items) => Bson::Array(items.iter().map(ext_value_to_bson).collect()),
        ExtValue::Map(map) => {
            let mut doc = Document::new();
            for (k, v) in map {
                doc.insert(k.clone(), ext_value_to_bson(v));
            }
            Bson::Document(doc)
        }
        ExtValue::Void => Bson::Null,
    }
}

/// Translates a BSON [`Document`] into an AION [`ExtValue::Map`].
/// Maps `_id` to `id` when `id` is not explicitly present.
pub fn bson_doc_to_ext_value(doc: &Document) -> ExtValue {
    let mut map = BTreeMap::new();
    for (k, v) in doc {
        map.insert(k.clone(), bson_to_ext_value(v));
    }
    if !map.contains_key("id") {
        if let Some(oid_val) = map.get("_id").cloned() {
            map.insert("id".to_string(), oid_val);
        }
    }
    ExtValue::Map(map)
}

/// Translates any [`Bson`] value dynamically into an AION [`ExtValue`].
pub fn bson_to_ext_value(val: &Bson) -> ExtValue {
    match val {
        Bson::Int32(n) => ExtValue::I32(*n),
        Bson::Int64(n) => ExtValue::Ts(*n),
        Bson::Double(f) => ExtValue::F64(*f),
        Bson::String(s) => ExtValue::Str(s.clone()),
        Bson::Boolean(b) => ExtValue::Bool(*b),
        Bson::Binary(bin) => ExtValue::Bytes(bin.bytes.clone()),
        Bson::DateTime(dt) => ExtValue::Ts(dt.timestamp_millis()),
        Bson::ObjectId(oid) => ExtValue::Str(oid.to_hex()),
        Bson::Array(items) => ExtValue::List(items.iter().map(bson_to_ext_value).collect()),
        Bson::Document(doc) => bson_doc_to_ext_value(doc),
        Bson::Null | Bson::Undefined => ExtValue::Void,
        other => ExtValue::Str(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aion_ext::ExtRegistry;
    use mongodb::bson::oid::ObjectId;
    use std::sync::Arc;

    #[test]
    fn conforms_strictly_to_abi_manifest() {
        let plugin = MongoPlugin::new();
        let manifest = plugin.manifest();
        manifest.validate().expect("manifest must satisfy ABI v1");
        assert_eq!(manifest.id, "aion/mongo");
        assert_eq!(manifest.ver, "1.2.0");

        for expected_fn in ["connect", "find_one", "ins", "upd", "del"] {
            assert!(
                manifest.fns.contains_key(expected_fn),
                "missing expected fn `{expected_fn}` in manifest"
            );
        }

        let mut reg = ExtRegistry::new();
        reg.register_plugin(Arc::new(plugin)).unwrap();
        assert!(reg.resolve("aion/mongo", "1.2").is_some());

        // Calling before connect returns `connection_failed` (declared in manifest errs).
        let err = reg
            .invoke(
                "aion/mongo",
                "1.2",
                "find_one",
                vec![
                    ExtValue::Str("users".to_string()),
                    ExtValue::Map(BTreeMap::new()),
                ],
            )
            .unwrap_err();
        assert!(err.to_string().contains("connection_failed"));

        // Invalid URI on connect returns `connection_failed`.
        let bad_uri_err = reg
            .invoke(
                "aion/mongo",
                "1.2",
                "connect",
                vec![ExtValue::Str("not-a-valid-uri".to_string())],
            )
            .unwrap_err();
        assert!(bad_uri_err.to_string().contains("connection_failed"));
    }

    #[test]
    fn dynamic_ext_value_bson_roundtrip_and_operators() {
        let mut inc = BTreeMap::new();
        inc.insert("pts".to_string(), ExtValue::I32(10));

        let mut root = BTreeMap::new();
        root.insert("$inc".to_string(), ExtValue::Map(inc));
        root.insert("active".to_string(), ExtValue::Bool(true));
        root.insert("score".to_string(), ExtValue::F64(98.5));
        root.insert("raw".to_string(), ExtValue::Bytes(vec![1, 2, 3]));
        root.insert("ts".to_string(), ExtValue::Ts(1_700_000_000_000));
        root.insert(
            "tags".to_string(),
            ExtValue::List(vec![
                ExtValue::Str("aion".to_string()),
                ExtValue::Str("mongo".to_string()),
            ]),
        );

        let mut doc = ext_value_to_doc(&ExtValue::Map(root.clone())).unwrap();
        let oid = ObjectId::new();
        doc.insert("_id", oid);

        assert_eq!(doc.get_document("$inc").unwrap().get_i32("pts").unwrap(), 10);

        let decoded = bson_doc_to_ext_value(&doc);
        let ExtValue::Map(decoded_map) = decoded else {
            panic!("expected ExtValue::Map");
        };
        assert_eq!(decoded_map.get("id"), Some(&ExtValue::Str(oid.to_hex())));
        assert_eq!(decoded_map.get("active"), Some(&ExtValue::Bool(true)));
        assert_eq!(decoded_map.get("score"), Some(&ExtValue::F64(98.5)));
        assert_eq!(
            decoded_map.get("raw"),
            Some(&ExtValue::Bytes(vec![1, 2, 3]))
        );
        assert_eq!(
            decoded_map.get("ts"),
            Some(&ExtValue::Ts(1_700_000_000_000))
        );
    }
}
