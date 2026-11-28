use std::collections::BTreeMap;

use aion_ast::{NodeKind, Program};

use crate::ExtRegistry;

/// Curated general-purpose `aion/std` vector icon set (24x24 viewBox, stroke-based SVG paths).
/// Uses only `M`, `L`, `H`, `V`, `C`, `Q`, `Z` (and lowercase relative equivalents) so Web,
/// WASM, Android `Path`, and SwiftUI `Path` render identically with zero external dependencies.
pub fn std_icons() -> BTreeMap<&'static str, &'static str> {
    let mut m = BTreeMap::new();

    // Navigation & Arrows
    m.insert("home", "M3 10.5L12 3l9 7.5V21h-6v-6H9v6H3V10.5z");
    m.insert("menu", "M4 6h16M4 12h16M4 18h16");
    m.insert("back", "M15 18l-6-6 6-6");
    m.insert("forward", "M9 18l6-6-6-6");
    m.insert("arrow_left", "M19 12H5M12 19l-7-7 7-7");
    m.insert("arrow_right", "M5 12h14M12 5l7 7-7 7");
    m.insert("arrow_up", "M12 19V5M5 12l7-7 7 7");
    m.insert("arrow_down", "M12 5v14M19 12l-7 7-7-7");
    m.insert("chevron_up", "M18 15l-6-6-6 6");
    m.insert("chevron_down", "M6 9l6 6 6-6");
    m.insert("chevron_left", "M15 18l-6-6 6-6");
    m.insert("chevron_right", "M9 18l6-6-6-6");
    m.insert("external_link", "M15 3h6v6M10 14L21 3M18 13v6H5V6h6");

    // Actions & CRUD
    m.insert("add", "M12 5v14M5 12h14");
    m.insert("plus", "M12 5v14M5 12h14");
    m.insert("minus", "M5 12h14");
    m.insert("close", "M18 6L6 18M6 6l12 12");
    m.insert("check", "M20 6L9 17l-5-5");
    m.insert("edit", "M17 3l4 4L8 20H4v-4L17 3z");
    m.insert("trash", "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 11v6M14 11v6");
    m.insert("delete", "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 11v6M14 11v6");
    m.insert("search", "M11 3C6.58 3 3 6.58 3 11C3 15.42 6.58 19 11 19C15.42 19 19 15.42 19 11C19 6.58 15.42 3 11 3ZM21 21l-4.35-4.35");
    m.insert("filter", "M3 4h18l-7 8v6l-4 2v-8L3 4z");
    m.insert("refresh", "M21 12C21 16.97 16.97 21 12 21C7.03 21 3 16.97 3 12C3 7.03 7.03 3 12 3C15.3 3 18.2 4.8 19.7 7.5M21 3v5h-5");
    m.insert("copy", "M9 9h11v11H9zM5 15H4V4h11v1");
    m.insert("save", "M5 3h11l5 5v13H5zM8 3v5h7V3M8 21v-7h8v7");
    m.insert("share", "M12 3v12M7 8l5-5 5 5M5 15v5h14v-5");
    m.insert("download", "M12 3v12M7 10l5 5 5-5M5 19h14");
    m.insert("upload", "M12 15V3M7 8l5-5 5 5M5 19h14");
    m.insert("more", "M5 12h.01M12 12h.01M19 12h.01");
    m.insert("settings", "M12 8C9.79 8 8 9.79 8 12C8 14.21 9.79 16 12 16C14.21 16 16 14.21 16 12C16 9.79 14.21 8 12 8ZM12 2v3M12 19v3M4.93 4.93l2.12 2.12M16.95 16.95l2.12 2.12M2 12h3M19 12h3M4.93 19.07l2.12-2.12M16.95 7.05l2.12-2.12");

    // Media & Timer
    m.insert("play", "M6 4l14 8-14 8V4z");
    m.insert("pause", "M6 4h4v16H6zM14 4h4v16h-4z");
    m.insert("stop", "M5 5h14v14H5z");
    m.insert("volume", "M11 5L6 9H2v6h4l5 4V5zM15.5 8.5C16.8 9.8 16.8 14.2 15.5 15.5M18.5 5.5C21.2 8.2 21.2 15.8 18.5 18.5");
    m.insert("mute", "M11 5L6 9H2v6h4l5 4V5zM22 9l-6 6M16 9l6 6");
    m.insert("clock", "M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM12 6v6l4 2");
    m.insert("timer", "M10 2h4M12 14l3-3M12 6C7.58 6 4 9.58 4 14C4 18.42 7.58 22 12 22C16.42 22 20 18.42 20 14C20 9.58 16.42 6 12 6Z");
    m.insert("calendar", "M4 6h16v14H4zM16 3v4M8 3v4M4 11h16");
    m.insert("camera", "M4 7h4l2-3h4l2 3h4v12H4zM12 10C10.34 10 9 11.34 9 13C9 14.66 10.34 16 12 16C13.66 16 15 14.66 15 13C15 11.34 13.66 10 12 10Z");
    m.insert("image", "M4 4h16v16H4zM8.5 9.5C9.33 9.5 10 8.83 10 8C10 7.17 9.33 6.5 8.5 6.5C7.67 6.5 7 7.17 7 8C7 8.83 7.67 9.5 8.5 9.5ZM20 15l-5-5L5 20");

    // Communication & User
    m.insert("user", "M12 3C9.79 3 8 4.79 8 7C8 9.21 9.79 11 12 11C14.21 11 16 9.21 16 7C16 4.79 14.21 3 12 3ZM4 21C4 17.13 7.58 14 12 14C16.42 14 20 17.13 20 21");
    m.insert("users", "M9 4C7.34 4 6 5.34 6 7C6 8.66 7.34 10 9 10C10.66 10 12 8.66 12 7C12 5.34 10.66 4 9 4ZM3 20C3 16.69 5.69 14 9 14C12.31 14 15 16.69 15 20M16 5C17.66 5 19 6.34 19 8C19 9.66 17.66 11 16 11M21 20C21 17.5 19.2 15.4 17 14.8");
    m.insert("mail", "M3 5h18v14H3zM3 7l9 6 9-6");
    m.insert("message", "M4 5h16v11H8l-4 4V5z");
    m.insert("phone", "M5 4h4l2 5-2.5 1.5C9.5 13 11 14.5 13.5 15.5L15 13l5 2v4C20 20.1 19.1 21 18 21C9.72 21 3 14.28 3 6C3 4.9 3.9 4 5 4Z");
    m.insert("bell", "M18 8C18 4.69 15.31 2 12 2C8.69 2 6 4.69 6 8C6 14 3 16 3 16h18C21 16 18 14 18 8ZM10 20C10.5 21 11.2 21.5 12 21.5C12.8 21.5 13.5 21 14 20");
    m.insert("send", "M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z");

    // Status & Feedback
    m.insert("info", "M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM12 16v-4M12 8h.01");
    m.insert("warning", "M12 3L2 20h20L12 3zM12 9v5M12 17h.01");
    m.insert("error", "M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM15 9l-6 6M9 9l6 6");
    m.insert("check_circle", "M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM9 12l2 2 4-4");
    m.insert("star", "M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2z");
    m.insert("heart", "M12 21C12 21 3 14.5 3 8.5C3 5.42 5.42 3 8.5 3C10.24 3 11.91 3.81 12 5C12.09 3.81 13.76 3 15.5 3C18.58 3 21 5.42 21 8.5C21 14.5 12 21 12 21Z");
    m.insert("bookmark", "M6 3h12v18l-6-4-6 4V3z");

    // Security & Visibility
    m.insert("lock", "M5 11h14v10H5zM8 11V7C8 4.79 9.79 3 12 3C14.21 3 16 4.79 16 7v4");
    m.insert("unlock", "M5 11h14v10H5zM8 11V7C8 4.79 9.79 3 12 3C14.21 3 16 4.79 16 7");
    m.insert("eye", "M2 12C4.5 7 8 5 12 5C16 5 19.5 7 22 12C19.5 17 16 19 12 19C8 19 4.5 17 2 12ZM12 9C10.34 9 9 10.34 9 12C9 13.66 10.34 15 12 15C13.66 15 15 13.66 15 12C15 10.34 13.66 9 12 9Z");
    m.insert("eye_off", "M3 3l18 18M10.5 5.2C11 5.07 11.5 5 12 5C16 5 19.5 7 22 12C21.1 13.8 19.9 15.3 18.5 16.4M6.5 6.5C4.7 7.8 3.2 9.7 2 12C4.5 17 8 19 12 19C13.6 19 15.1 18.6 16.5 17.9");
    m.insert("shield", "M12 2l8 4v6C20 16.97 16.58 20.55 12 22C7.42 20.55 4 16.97 4 12V6l8-4z");

    // Finance, Files & Misc
    m.insert("wallet", "M3 6h18v14H3zM3 10h18M16 14h2");
    m.insert("card", "M2 6h20v12H2zM2 10h20M6 15h4");
    m.insert("dollar", "M12 2v20M17 6H9.5C7.57 6 6 7.34 6 9C6 10.66 7.57 12 9.5 12h5C16.43 12 18 13.34 18 15C18 16.66 16.43 18 14.5 18H6");
    m.insert("cart", "M3 4h2l2.5 11h10L20 7H6M9 20C9.55 20 10 19.55 10 19C10 18.45 9.55 18 9 18C8.45 18 8 18.45 8 19C8 19.55 8.45 20 9 20ZM17 20C17.55 20 18 19.55 18 19C18 18.45 17.55 18 17 18C16.45 18 16 18.45 16 19C16 19.55 16.45 20 17 20Z");
    m.insert("chart", "M4 20h16M7 16v-6M12 16V6M17 16v-9");
    m.insert("folder", "M3 6h6l2 2h10v11H3z");
    m.insert("file", "M6 3h8l4 4v14H6zM14 3v4h4");
    m.insert("list", "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01");
    m.insert("grid", "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z");
    m.insert("map_pin", "M12 21C12 21 19 15.2 19 10C19 6.13 15.87 3 12 3C8.13 3 5 6.13 5 10C5 15.2 12 21 12 21ZM12 8C10.9 8 10 8.9 10 10C10 11.1 10.9 12 12 12C13.1 12 14 11.1 14 10C14 8.9 13.1 8 12 8Z");
    m.insert("globe", "M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM2 12h20M12 2C9.5 5 8 8.4 8 12C8 15.6 9.5 19 12 22C14.5 19 16 15.6 16 12C16 8.4 14.5 5 12 2Z");
    m.insert("sun", "M12 8C9.79 8 8 9.79 8 12C8 14.21 9.79 16 12 16C14.21 16 16 14.21 16 12C16 9.79 14.21 8 12 8ZM12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41");
    m.insert("moon", "M21 12.79C20.1 17.46 15.99 21 11 21C5.48 21 1 16.52 1 11C1 6.01 4.54 1.9 9.21 1C8.44 2.4 8 4.02 8 5.75C8 11.41 12.59 16 18.25 16C19.24 16 20.17 15.86 21 15.6Z");
    m.insert("wifi", "M5 12.5C8.87 8.63 15.13 8.63 19 12.5M8.5 16C10.43 14.07 13.57 14.07 15.5 16M2 9C7.52 3.48 16.48 3.48 22 9M12 20h.01");
    m.insert("logout", "M9 21H5V3h4M16 17l5-5-5-5M21 12H9");
    m.insert("login", "M15 3h4v18h-4M10 17l5-5-5-5M15 12H3");

    m
}

/// Returns true if `name` is in the built-in `aion/std` icon catalog.
pub fn is_std_icon(name: &str) -> bool {
    std_icons().contains_key(name)
}

/// Builds the complete icon SVG path dictionary for a compiled bundle:
/// - All built-in `aion/std` icons under `"name"`
/// - All icons from `#EXT` modules imported by `program` under `"alias:icon_name"`
pub fn collect_program_icons(program: &Program, registry: &ExtRegistry) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = std_icons()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    for node in &program.nodes {
        if let NodeKind::Ext(ext) = &node.kind {
            if let Some(manifest) = registry.resolve(&ext.module_id, &ext.version_req) {
                for (icon_name, svg_path) in &manifest.icons {
                    out.insert(format!("{}:{icon_name}", ext.alias), svg_path.clone());
                }
            }
        }
    }

    out
}
