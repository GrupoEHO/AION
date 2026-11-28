pub mod icons;
pub mod manifest;
pub mod registry;
pub mod wit;

pub use icons::*;
pub use manifest::*;
pub use registry::*;
pub use wit::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn loads_and_validates_mock_manifests() {
        let mongo_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../manifests/aion-mongo-1.2.0.json");
        let auth_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../manifests/aion-firebase-auth-2.0.0.json");

        let mut reg = ExtRegistry::new();
        reg.register_manifest_file(&mongo_path).unwrap();
        reg.register_manifest_file(&auth_path).unwrap();

        let mongo = reg.resolve("aion/mongo", "1.2").unwrap();
        assert_eq!(mongo.ver, "1.2.0");
        assert!(mongo.fns.contains_key("connect"));
        assert!(mongo.fns.contains_key("upd"));

        let auth = reg.resolve("aion/firebase-auth", "2.0").unwrap();
        assert_eq!(auth.ver, "2.0.0");
        assert!(auth.fns.contains_key("verify"));

        let wit = manifest_to_wit(auth);
        assert!(wit.contains("package aion:firebase-auth@2.0.0;"));
        assert!(wit.contains("record auth-user"));
        assert!(wit.contains("verify: func(token: string, project-id: string)"));
    }

    #[test]
    fn rejects_invalid_abi_and_unknown_fields() {
        let bad_abi = r#"{
            "abi": 2,
            "id": "aion/mongo",
            "ver": "1.2.0",
            "caps": ["net"],
            "fns": {}
        }"#;
        assert!(matches!(
            ExtManifest::from_json_str(bad_abi),
            Err(ManifestError::UnsupportedAbi(2))
        ));

        let extra_field = r#"{
            "abi": 1,
            "id": "aion/mongo",
            "ver": "1.2.0",
            "caps": ["net"],
            "unknown_prop": true,
            "fns": {}
        }"#;
        assert!(matches!(
            ExtManifest::from_json_str(extra_field),
            Err(ManifestError::Json(_))
        ));
    }
}
