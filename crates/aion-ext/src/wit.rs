use aion_ast::AionType;

use crate::manifest::ExtManifest;

/// Generates a WebAssembly Component Model (WIT) interface definition from an [`ExtManifest`].
pub fn manifest_to_wit(manifest: &ExtManifest) -> String {
    let (ns, pkg) = manifest.id.split_once('/').unwrap_or(("aion", &manifest.id));
    let iface_name = pkg.replace('_', "-");
    let mut out = String::new();
    out.push_str(&format!("package {ns}:{iface_name}@{};\n\n", manifest.ver));
    out.push_str(&format!("interface {iface_name} {{\n"));

    for (type_name, fields) in &manifest.types {
        let wit_record = to_kebab(type_name);
        out.push_str(&format!("  record {wit_record} {{\n"));
        for (field_name, ty_str) in fields {
            let wit_ty = aion_type_str_to_wit(ty_str);
            out.push_str(&format!("    {}: {wit_ty},\n", to_kebab(field_name)));
        }
        out.push_str("  }\n\n");
    }

    if !manifest.errs.is_empty() {
        out.push_str("  enum ext-error {\n");
        for err in &manifest.errs {
            out.push_str(&format!("    {},\n", to_kebab(err)));
        }
        out.push_str("  }\n\n");
    }

    for (fn_name, sig) in &manifest.fns {
        let params: Vec<String> = sig
            .args
            .iter()
            .map(|a| {
                let wit_ty = aion_type_str_to_wit(&a.t);
                if a.opt {
                    format!("{}: option<{wit_ty}>", to_kebab(&a.n))
                } else {
                    format!("{}: {wit_ty}", to_kebab(&a.n))
                }
            })
            .collect();
        let ret_wit = if sig.ret == "void" {
            if sig.errs.is_empty() {
                String::new()
            } else {
                " -> result<_, ext-error>".to_string()
            }
        } else {
            let ty = aion_type_str_to_wit(&sig.ret);
            if sig.errs.is_empty() {
                format!(" -> {ty}")
            } else {
                format!(" -> result<{ty}, ext-error>")
            }
        };
        out.push_str(&format!(
            "  {}: func({}){};\n",
            to_kebab(fn_name),
            params.join(", "),
            ret_wit
        ));
    }

    out.push_str("}\n");
    out
}

fn aion_type_str_to_wit(raw: &str) -> String {
    match AionType::parse(raw) {
        Ok(ty) => aion_type_to_wit(&ty),
        Err(_) => "string".to_string(),
    }
}

fn aion_type_to_wit(ty: &AionType) -> String {
    match ty {
        AionType::I32 => "s32".to_string(),
        AionType::F64 => "f64".to_string(),
        AionType::Str => "string".to_string(),
        AionType::Bool => "bool".to_string(),
        AionType::Bytes => "list<u8>".to_string(),
        AionType::Ts => "s64".to_string(),
        AionType::List(inner) => format!("list<{}>", aion_type_to_wit(inner)),
        AionType::Map(k, v) => {
            format!(
                "list<tuple<{}, {}>>",
                aion_type_to_wit(k),
                aion_type_to_wit(v)
            )
        }
        AionType::ExtType { name, .. } => to_kebab(name),
        AionType::Any => "string".to_string(),
        AionType::Void => "_".to_string(),
        AionType::ModelRef(id) => to_kebab(id.as_str().trim_start_matches('#')),
        AionType::Named(n) => to_kebab(n),
        AionType::Unknown => "string".to_string(),
    }
}

fn to_kebab(s: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in s.chars().enumerate() {
        if ch == '_' {
            out.push('-');
        } else if ch.is_ascii_uppercase() {
            if idx > 0 && !out.ends_with('-') {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
