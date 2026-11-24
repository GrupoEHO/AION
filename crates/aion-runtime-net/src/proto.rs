use std::collections::{BTreeMap, HashMap};

use aion_ast::{AionType, ApiOutput, FieldDef, NodeId, NodeKind, Program};
use aion_ext::ExtValue;
use prost::Message as ProstMessage;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MapKey, MessageDescriptor, ReflectMessage,
    Value as ReflectValue,
};
use prost_types::{
    field_descriptor_proto, DescriptorProto, FieldDescriptorProto, FileDescriptorProto,
    FileDescriptorSet, MessageOptions,
};

use crate::RuntimeNetError;

const PKG_NAME: &str = "aion.generated";

/// Compiled Protobuf schema (`FileDescriptorSet` + `DescriptorPool` + `.proto` text)
/// derived from `#M` models and `#API` `IN`/`OUT` blocks.
#[derive(Debug, Clone)]
pub struct ProtoSchema {
    pub proto_text: String,
    pub pool: DescriptorPool,
    pub api_in_msg: HashMap<NodeId, String>,
    pub api_out_msg: HashMap<NodeId, String>,
}

impl ProtoSchema {
    pub fn from_program(program: &Program) -> Result<Self, RuntimeNetError> {
        let mut model_names: HashMap<NodeId, String> = HashMap::new();
        for node in &program.nodes {
            if let NodeKind::Model(model) = &node.kind {
                model_names.insert(node.id.clone(), model.name.clone());
            }
        }

        let mut descriptors = Vec::new();
        let mut proto_text = format!("syntax = \"proto3\";\npackage {PKG_NAME};\n\n");

        // Standard error response descriptor.
        descriptors.push(build_descriptor(
            "ErrorResponse",
            &[
                ("status", &AionType::I32),
                ("error", &AionType::Str),
            ],
            &model_names,
        ));
        proto_text.push_str(
            "message ErrorResponse {\n  int32 status = 1;\n  string error = 2;\n}\n\n",
        );

        // `#M` models.
        for node in &program.nodes {
            if let NodeKind::Model(model) = &node.kind {
                let pairs: Vec<(&str, &AionType)> = model
                    .fields
                    .iter()
                    .map(|f| (f.name.as_str(), &f.ty))
                    .collect();
                descriptors.push(build_descriptor(&model.name, &pairs, &model_names));
                proto_text.push_str(&fields_to_proto_text(&model.name, &pairs, &model_names));
            }
        }

        // `#API` `IN` and `OUT` messages.
        let mut api_in_msg = HashMap::new();
        let mut api_out_msg = HashMap::new();

        for node in &program.nodes {
            if let NodeKind::Api(api) = &node.kind {
                let base = sanitize_pascal(node.id.as_str().trim_start_matches('#'));

                if !api.input_fields.is_empty() {
                    let msg_name = format!("{base}In");
                    let pairs: Vec<(&str, &AionType)> = api
                        .input_fields
                        .iter()
                        .map(|f| (f.name.as_str(), &f.ty))
                        .collect();
                    descriptors.push(build_descriptor(&msg_name, &pairs, &model_names));
                    proto_text.push_str(&fields_to_proto_text(&msg_name, &pairs, &model_names));
                    api_in_msg.insert(node.id.clone(), format!("{PKG_NAME}.{msg_name}"));
                }

                match &api.output {
                    Some(ApiOutput::ModelRef(model_id)) => {
                        if let Some(m_name) = model_names.get(model_id) {
                            api_out_msg.insert(node.id.clone(), format!("{PKG_NAME}.{m_name}"));
                        }
                    }
                    Some(ApiOutput::Fields(fields)) => {
                        let msg_name = format!("{base}Out");
                        let pairs: Vec<(&str, &AionType)> = fields
                            .iter()
                            .map(|f| (f.name.as_str(), &f.ty))
                            .collect();
                        descriptors.push(build_descriptor(&msg_name, &pairs, &model_names));
                        proto_text
                            .push_str(&fields_to_proto_text(&msg_name, &pairs, &model_names));
                        api_out_msg.insert(node.id.clone(), format!("{PKG_NAME}.{msg_name}"));
                    }
                    Some(ApiOutput::Type(AionType::ModelRef(model_id))) => {
                        if let Some(m_name) = model_names.get(model_id) {
                            api_out_msg.insert(node.id.clone(), format!("{PKG_NAME}.{m_name}"));
                        }
                    }
                    Some(ApiOutput::Type(ty)) if *ty != AionType::Void => {
                        let msg_name = format!("{base}Out");
                        let pairs = vec![("value", ty)];
                        descriptors.push(build_descriptor(&msg_name, &pairs, &model_names));
                        proto_text
                            .push_str(&fields_to_proto_text(&msg_name, &pairs, &model_names));
                        api_out_msg.insert(node.id.clone(), format!("{PKG_NAME}.{msg_name}"));
                    }
                    _ => {}
                }
            }
        }

        let fds = FileDescriptorSet {
            file: vec![FileDescriptorProto {
                name: Some("aion_schema.proto".to_string()),
                package: Some(PKG_NAME.to_string()),
                syntax: Some("proto3".to_string()),
                message_type: descriptors,
                ..Default::default()
            }],
        };

        let pool = DescriptorPool::from_file_descriptor_set(fds)
            .map_err(|e| RuntimeNetError::Proto(format!("invalid generated proto descriptor: {e}")))?;

        Ok(Self {
            proto_text,
            pool,
            api_in_msg,
            api_out_msg,
        })
    }

    pub fn message_descriptor(&self, full_name: &str) -> Option<MessageDescriptor> {
        self.pool.get_message_by_name(full_name)
    }

    pub fn decode_input(
        &self,
        api_id: &NodeId,
        bytes: &[u8],
    ) -> Result<BTreeMap<String, ExtValue>, RuntimeNetError> {
        let Some(full_name) = self.api_in_msg.get(api_id) else {
            return Ok(BTreeMap::new());
        };
        let desc = self.message_descriptor(full_name).ok_or_else(|| {
            RuntimeNetError::Proto(format!("missing descriptor for `{full_name}`"))
        })?;
        let msg = DynamicMessage::decode(desc, bytes)
            .map_err(|e| RuntimeNetError::Proto(format!("protobuf decode error: {e}")))?;
        match dynamic_msg_to_ext_value(&msg) {
            ExtValue::Map(m) => Ok(m),
            _ => Ok(BTreeMap::new()),
        }
    }

    pub fn encode_input(
        &self,
        api_id: &NodeId,
        val: &ExtValue,
    ) -> Result<Vec<u8>, RuntimeNetError> {
        let Some(full_name) = self.api_in_msg.get(api_id) else {
            return Ok(Vec::new());
        };
        let desc = self.message_descriptor(full_name).ok_or_else(|| {
            RuntimeNetError::Proto(format!("missing descriptor for `{full_name}`"))
        })?;
        let msg = ext_value_to_dynamic_msg(&desc, val)?;
        Ok(msg.encode_to_vec())
    }

    pub fn decode_output(
        &self,
        api_id: &NodeId,
        bytes: &[u8],
    ) -> Result<ExtValue, RuntimeNetError> {
        let Some(full_name) = self.api_out_msg.get(api_id) else {
            return Ok(ExtValue::Void);
        };
        let desc = self.message_descriptor(full_name).ok_or_else(|| {
            RuntimeNetError::Proto(format!("missing descriptor for `{full_name}`"))
        })?;
        let msg = DynamicMessage::decode(desc, bytes)
            .map_err(|e| RuntimeNetError::Proto(format!("protobuf decode error: {e}")))?;
        Ok(dynamic_msg_to_ext_value(&msg))
    }

    pub fn encode_output(
        &self,
        api_id: &NodeId,
        val: &ExtValue,
    ) -> Result<Vec<u8>, RuntimeNetError> {
        let Some(full_name) = self.api_out_msg.get(api_id) else {
            return Ok(Vec::new());
        };
        let desc = self.message_descriptor(full_name).ok_or_else(|| {
            RuntimeNetError::Proto(format!("missing descriptor for `{full_name}`"))
        })?;
        let msg = ext_value_to_dynamic_msg(&desc, val)?;
        Ok(msg.encode_to_vec())
    }

    pub fn encode_error(&self, status: u16, error: &str) -> Result<Vec<u8>, RuntimeNetError> {
        let full_name = format!("{PKG_NAME}.ErrorResponse");
        let desc = self.message_descriptor(&full_name).ok_or_else(|| {
            RuntimeNetError::Proto("missing ErrorResponse descriptor".to_string())
        })?;
        let mut map = BTreeMap::new();
        map.insert("status".to_string(), ExtValue::I32(status as i32));
        map.insert("error".to_string(), ExtValue::Str(error.to_string()));
        let msg = ext_value_to_dynamic_msg(&desc, &ExtValue::Map(map))?;
        Ok(msg.encode_to_vec())
    }
}

pub fn generate_proto3(program: &Program) -> String {
    match ProtoSchema::from_program(program) {
        Ok(schema) => schema.proto_text,
        Err(_) => {
            let mut out = format!("syntax = \"proto3\";\npackage {PKG_NAME};\n\n");
            let model_names: HashMap<NodeId, String> = program
                .nodes
                .iter()
                .filter_map(|n| match &n.kind {
                    NodeKind::Model(m) => Some((n.id.clone(), m.name.clone())),
                    _ => None,
                })
                .collect();
            for node in &program.nodes {
                if let NodeKind::Model(model) = &node.kind {
                    let pairs: Vec<(&str, &AionType)> = model
                        .fields
                        .iter()
                        .map(|f| (f.name.as_str(), &f.ty))
                        .collect();
                    out.push_str(&fields_to_proto_text(&model.name, &pairs, &model_names));
                }
            }
            out
        }
    }
}

fn fields_to_proto_text(
    msg_name: &str,
    fields: &[(&str, &AionType)],
    model_names: &HashMap<NodeId, String>,
) -> String {
    let mut out = format!("message {msg_name} {{\n");
    for (idx, (name, ty)) in fields.iter().enumerate() {
        let proto_ty = aion_type_to_proto_text(ty, model_names);
        out.push_str(&format!("  {proto_ty} {name} = {};\n", idx + 1));
    }
    out.push_str("}\n\n");
    out
}

fn aion_type_to_proto_text(ty: &AionType, model_names: &HashMap<NodeId, String>) -> String {
    match ty {
        AionType::I32 => "int32".to_string(),
        AionType::F64 => "double".to_string(),
        AionType::Str => "string".to_string(),
        AionType::Bool => "bool".to_string(),
        AionType::Bytes => "bytes".to_string(),
        AionType::Ts => "int64".to_string(),
        AionType::List(inner) => format!("repeated {}", aion_type_to_proto_text(inner, model_names)),
        AionType::Map(k, v) => format!(
            "map<{}, {}>",
            aion_type_to_proto_text(k, model_names),
            aion_type_to_proto_text(v, model_names)
        ),
        AionType::ModelRef(id) => model_names
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.as_str().trim_start_matches('#').to_string()),
        _ => "string".to_string(),
    }
}

fn build_descriptor(
    msg_name: &str,
    fields: &[(&str, &AionType)],
    model_names: &HashMap<NodeId, String>,
) -> DescriptorProto {
    let mut field_protos = Vec::with_capacity(fields.len());
    let mut nested_types = Vec::new();

    for (idx, (name, ty)) in fields.iter().enumerate() {
        let number = (idx + 1) as i32;
        if let AionType::Map(k, v) = ty {
            let entry_name = format!("{}Entry", sanitize_pascal(name));
            let (k_type, k_tname) = scalar_or_msg_type(k, model_names);
            let (v_type, v_tname) = scalar_or_msg_type(v, model_names);
            nested_types.push(DescriptorProto {
                name: Some(entry_name.clone()),
                field: vec![
                    FieldDescriptorProto {
                        name: Some("key".to_string()),
                        number: Some(1),
                        label: Some(field_descriptor_proto::Label::Optional as i32),
                        r#type: Some(k_type as i32),
                        type_name: k_tname,
                        json_name: Some("key".to_string()),
                        ..Default::default()
                    },
                    FieldDescriptorProto {
                        name: Some("value".to_string()),
                        number: Some(2),
                        label: Some(field_descriptor_proto::Label::Optional as i32),
                        r#type: Some(v_type as i32),
                        type_name: v_tname,
                        json_name: Some("value".to_string()),
                        ..Default::default()
                    },
                ],
                options: Some(MessageOptions {
                    map_entry: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            });

            field_protos.push(FieldDescriptorProto {
                name: Some((*name).to_string()),
                number: Some(number),
                label: Some(field_descriptor_proto::Label::Repeated as i32),
                r#type: Some(field_descriptor_proto::Type::Message as i32),
                type_name: Some(format!(".{PKG_NAME}.{msg_name}.{entry_name}")),
                json_name: Some((*name).to_string()),
                ..Default::default()
            });
        } else {
            let (label, ftype, tname) = match ty {
                AionType::List(inner) => {
                    let (it, tn) = scalar_or_msg_type(inner, model_names);
                    (field_descriptor_proto::Label::Repeated, it, tn)
                }
                other => {
                    let (ft, tn) = scalar_or_msg_type(other, model_names);
                    (field_descriptor_proto::Label::Optional, ft, tn)
                }
            };
            field_protos.push(FieldDescriptorProto {
                name: Some((*name).to_string()),
                number: Some(number),
                label: Some(label as i32),
                r#type: Some(ftype as i32),
                type_name: tname,
                json_name: Some((*name).to_string()),
                ..Default::default()
            });
        }
    }

    DescriptorProto {
        name: Some(msg_name.to_string()),
        field: field_protos,
        nested_type: nested_types,
        ..Default::default()
    }
}

fn scalar_or_msg_type(
    ty: &AionType,
    model_names: &HashMap<NodeId, String>,
) -> (field_descriptor_proto::Type, Option<String>) {
    match ty {
        AionType::I32 => (field_descriptor_proto::Type::Int32, None),
        AionType::F64 => (field_descriptor_proto::Type::Double, None),
        AionType::Str => (field_descriptor_proto::Type::String, None),
        AionType::Bool => (field_descriptor_proto::Type::Bool, None),
        AionType::Bytes => (field_descriptor_proto::Type::Bytes, None),
        AionType::Ts => (field_descriptor_proto::Type::Int64, None),
        AionType::ModelRef(id) => {
            let name = model_names
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.as_str().trim_start_matches('#').to_string());
            (
                field_descriptor_proto::Type::Message,
                Some(format!(".{PKG_NAME}.{name}")),
            )
        }
        _ => (field_descriptor_proto::Type::String, None),
    }
}

fn ext_value_to_dynamic_msg(
    desc: &MessageDescriptor,
    val: &ExtValue,
) -> Result<DynamicMessage, RuntimeNetError> {
    let mut msg = DynamicMessage::new(desc.clone());
    match val {
        ExtValue::Map(map) => {
            for field_desc in desc.fields() {
                if let Some(fv) = map.get(field_desc.name()) {
                    let rval = ext_value_to_reflect(&field_desc, fv)?;
                    msg.set_field(&field_desc, rval);
                }
            }
        }
        other => {
            if let Some(field_desc) = desc.get_field_by_name("value") {
                let rval = ext_value_to_reflect(&field_desc, other)?;
                msg.set_field(&field_desc, rval);
            }
        }
    }
    Ok(msg)
}

fn ext_value_to_reflect(
    field_desc: &prost_reflect::FieldDescriptor,
    val: &ExtValue,
) -> Result<ReflectValue, RuntimeNetError> {
    if field_desc.is_map() {
        let mut rmap = HashMap::new();
        if let ExtValue::Map(m) = val {
            if let prost_reflect::Kind::Message(entry_desc) = field_desc.kind() {
                let val_field = entry_desc.get_field_by_name("value").ok_or_else(|| {
                    RuntimeNetError::Proto("map entry missing `value` field".to_string())
                })?;
                for (k, v) in m {
                    rmap.insert(
                        MapKey::String(k.clone()),
                        ext_value_to_reflect(&val_field, v)?,
                    );
                }
            }
        }
        return Ok(ReflectValue::Map(rmap));
    }

    if field_desc.is_list() {
        let mut rlist = Vec::new();
        if let ExtValue::List(items) = val {
            for item in items {
                rlist.push(kind_to_reflect(&field_desc.kind(), item)?);
            }
        }
        return Ok(ReflectValue::List(rlist));
    }

    kind_to_reflect(&field_desc.kind(), val)
}

fn kind_to_reflect(
    kind: &prost_reflect::Kind,
    val: &ExtValue,
) -> Result<ReflectValue, RuntimeNetError> {
    use prost_reflect::Kind;
    match (kind, val) {
        (Kind::Int32 | Kind::Sint32 | Kind::Sfixed32, ExtValue::I32(n)) => {
            Ok(ReflectValue::I32(*n))
        }
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ExtValue::Ts(t)) => {
            Ok(ReflectValue::I64(*t))
        }
        (Kind::Int64 | Kind::Sint64 | Kind::Sfixed64, ExtValue::I32(n)) => {
            Ok(ReflectValue::I64(*n as i64))
        }
        (Kind::Double, ExtValue::F64(f)) => Ok(ReflectValue::F64(*f)),
        (Kind::Double, ExtValue::I32(n)) => Ok(ReflectValue::F64(*n as f64)),
        (Kind::Float, ExtValue::F64(f)) => Ok(ReflectValue::F32(*f as f32)),
        (Kind::Bool, ExtValue::Bool(b)) => Ok(ReflectValue::Bool(*b)),
        (Kind::String, ExtValue::Str(s)) => Ok(ReflectValue::String(s.clone())),
        (Kind::String, ExtValue::Void) => Ok(ReflectValue::String(String::new())),
        (Kind::String, other) => Ok(ReflectValue::String(ext_value_to_json(other).to_string())),
        (Kind::Bytes, ExtValue::Bytes(b)) => {
            Ok(ReflectValue::Bytes(bytes::Bytes::from(b.clone())))
        }
        (Kind::Message(sub_desc), _) => {
            Ok(ReflectValue::Message(ext_value_to_dynamic_msg(sub_desc, val)?))
        }
        _ => Err(RuntimeNetError::Proto(format!(
            "cannot convert {val:?} to protobuf kind {kind:?}"
        ))),
    }
}

pub fn dynamic_msg_to_ext_value(msg: &DynamicMessage) -> ExtValue {
    let mut map = BTreeMap::new();
    for field_desc in msg.descriptor().fields() {
        let val = msg.get_field(&field_desc);
        map.insert(field_desc.name().to_string(), reflect_to_ext_value(&val));
    }
    ExtValue::Map(map)
}

fn reflect_to_ext_value(val: &ReflectValue) -> ExtValue {
    match val {
        ReflectValue::Bool(b) => ExtValue::Bool(*b),
        ReflectValue::I32(n) => ExtValue::I32(*n),
        ReflectValue::I64(n) => ExtValue::Ts(*n),
        ReflectValue::U32(n) => ExtValue::I32(*n as i32),
        ReflectValue::U64(n) => ExtValue::Ts(*n as i64),
        ReflectValue::F32(f) => ExtValue::F64(*f as f64),
        ReflectValue::F64(f) => ExtValue::F64(*f),
        ReflectValue::String(s) => ExtValue::Str(s.clone()),
        ReflectValue::Bytes(b) => ExtValue::Bytes(b.to_vec()),
        ReflectValue::EnumNumber(n) => ExtValue::I32(*n),
        ReflectValue::Message(m) => dynamic_msg_to_ext_value(m),
        ReflectValue::List(items) => {
            ExtValue::List(items.iter().map(reflect_to_ext_value).collect())
        }
        ReflectValue::Map(m) => {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                let key_str = match k {
                    MapKey::String(s) => s.clone(),
                    MapKey::Bool(b) => b.to_string(),
                    MapKey::I32(n) => n.to_string(),
                    MapKey::I64(n) => n.to_string(),
                    MapKey::U32(n) => n.to_string(),
                    MapKey::U64(n) => n.to_string(),
                };
                out.insert(key_str, reflect_to_ext_value(v));
            }
            ExtValue::Map(out)
        }
    }
}

pub fn ext_value_to_json(val: &ExtValue) -> serde_json::Value {
    match val {
        ExtValue::I32(n) => serde_json::Value::Number((*n).into()),
        ExtValue::F64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        ExtValue::Str(s) => serde_json::Value::String(s.clone()),
        ExtValue::Bool(b) => serde_json::Value::Bool(*b),
        ExtValue::Bytes(b) => {
            serde_json::Value::Array(b.iter().map(|byte| (*byte).into()).collect())
        }
        ExtValue::Ts(t) => serde_json::Value::Number((*t).into()),
        ExtValue::List(items) => {
            serde_json::Value::Array(items.iter().map(ext_value_to_json).collect())
        }
        ExtValue::Map(m) => {
            let obj = m
                .iter()
                .map(|(k, v)| (k.clone(), ext_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        ExtValue::Void => serde_json::Value::Null,
    }
}

pub fn json_to_ext_value(val: &serde_json::Value) -> ExtValue {
    match val {
        serde_json::Value::Null => ExtValue::Void,
        serde_json::Value::Bool(b) => ExtValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(i32_v) = i32::try_from(i) {
                    ExtValue::I32(i32_v)
                } else {
                    ExtValue::Ts(i)
                }
            } else if let Some(f) = n.as_f64() {
                ExtValue::F64(f)
            } else {
                ExtValue::Void
            }
        }
        serde_json::Value::String(s) => ExtValue::Str(s.clone()),
        serde_json::Value::Array(arr) => {
            ExtValue::List(arr.iter().map(json_to_ext_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_ext_value(v)))
                .collect();
            ExtValue::Map(map)
        }
    }
}

pub fn json_bytes_to_input_map(
    bytes: &[u8],
    _fields: &[FieldDef],
) -> Result<BTreeMap<String, ExtValue>, RuntimeNetError> {
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    let val: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| RuntimeNetError::Codec(format!("invalid JSON request body: {e}")))?;
    match json_to_ext_value(&val) {
        ExtValue::Map(m) => Ok(m),
        _ => Err(RuntimeNetError::Codec(
            "JSON request body must be an object".to_string(),
        )),
    }
}

fn sanitize_pascal(raw: &str) -> String {
    let mut out = String::new();
    let mut cap_next = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if cap_next {
                out.push(ch.to_ascii_uppercase());
                cap_next = false;
            } else {
                out.push(ch.to_ascii_lowercase());
            }
        } else {
            cap_next = true;
        }
    }
    if out.is_empty() {
        "Msg".to_string()
    } else {
        out
    }
}
