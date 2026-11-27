use std::collections::{BTreeMap, HashMap, HashSet};

use aion_ast::{NodeId, NodeKind, Program, UiArg, UiBody, UiNode};
use serde::{Deserialize, Serialize};
use taffy::prelude::*;

use crate::signals::SignalStore;
use crate::UiError;

/// Standard target viewport dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Viewport {
    pub name: &'static str,
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    pub const MOBILE: Self = Self {
        name: "mobile",
        width: 390.0,
        height: 844.0,
    };

    pub const DESKTOP: Self = Self {
        name: "desktop",
        width: 1440.0,
        height: 900.0,
    };

    pub const fn custom(name: &'static str, width: f32, height: f32) -> Self {
        Self {
            name,
            width,
            height,
        }
    }
}

/// Computed absolute layout rectangle for a `#N` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutBox {
    pub node_id: NodeId,
    pub widget: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub parent: Option<NodeId>,
    pub scrollable: bool,
}

/// Overflow violation detected when a child exceeds its non-scrollable container or viewport.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverflowIssue {
    pub child_id: NodeId,
    pub parent_id: Option<NodeId>,
    pub viewport_name: String,
    pub container_width: f32,
    pub container_height: f32,
    pub child_width: f32,
    pub child_height: f32,
    pub axis: String,
}

/// Computed layout tree for an AION program at a given [`Viewport`].
#[derive(Debug, Clone, Serialize)]
pub struct LayoutTree {
    pub viewport: Viewport,
    pub roots: Vec<NodeId>,
    pub boxes: BTreeMap<NodeId, LayoutBox>,
    pub paint_order: Vec<NodeId>,
}

impl LayoutTree {
    pub fn get(&self, id: &NodeId) -> Option<&LayoutBox> {
        self.boxes.get(id)
    }

    /// Detects horizontal or vertical overflows of children exceeding non-scrollable parents
    /// or exceeding the viewport bounds.
    pub fn detect_overflows(&self) -> Vec<OverflowIssue> {
        const EPS: f32 = 0.5;
        let mut issues = Vec::new();

        for b in self.boxes.values() {
            if let Some(parent_id) = &b.parent {
                if let Some(parent) = self.boxes.get(parent_id) {
                    if parent.scrollable {
                        continue;
                    }
                    let horiz_overflow = b.width > parent.width + EPS
                        || b.x < parent.x - EPS
                        || (b.x + b.width) > (parent.x + parent.width + EPS);
                    let vert_overflow = b.height > parent.height + EPS
                        || b.y < parent.y - EPS
                        || (b.y + b.height) > (parent.y + parent.height + EPS);

                    if horiz_overflow || vert_overflow {
                        let axis = match (horiz_overflow, vert_overflow) {
                            (true, true) => "both",
                            (true, false) => "horizontal",
                            (false, true) => "vertical",
                            (false, false) => unreachable!(),
                        };
                        issues.push(OverflowIssue {
                            child_id: b.node_id.clone(),
                            parent_id: Some(parent_id.clone()),
                            viewport_name: self.viewport.name.to_string(),
                            container_width: parent.width,
                            container_height: parent.height,
                            child_width: b.width,
                            child_height: b.height,
                            axis: axis.to_string(),
                        });
                        continue;
                    }
                }
            }

            // Root-level node exceeding viewport bounds.
            let horiz_vp = b.width > self.viewport.width + EPS
                || (b.x + b.width) > self.viewport.width + EPS;
            let vert_vp = b.height > self.viewport.height + EPS
                || (b.y + b.height) > self.viewport.height + EPS;
            if horiz_vp || vert_vp {
                let axis = match (horiz_vp, vert_vp) {
                    (true, true) => "both",
                    (true, false) => "horizontal",
                    (false, true) => "vertical",
                    (false, false) => unreachable!(),
                };
                issues.push(OverflowIssue {
                    child_id: b.node_id.clone(),
                    parent_id: None,
                    viewport_name: self.viewport.name.to_string(),
                    container_width: self.viewport.width,
                    container_height: self.viewport.height,
                    child_width: b.width,
                    child_height: b.height,
                    axis: axis.to_string(),
                });
            }
        }

        issues
    }
}

/// Translates `#N` nodes (`ROOT`, `FLEX`, `TXT`, `BTN`) into a `taffy` layout tree
/// and computes absolute `(x, y, width, height)` boxes for `viewport`.
pub fn compute_layout(
    program: &Program,
    viewport: Viewport,
    signals: Option<&SignalStore>,
) -> Result<LayoutTree, UiError> {
    let mut ui_nodes: HashMap<NodeId, &UiNode> = HashMap::new();
    let mut ui_order: Vec<NodeId> = Vec::new();
    let mut referenced_children: HashSet<NodeId> = HashSet::new();

    for node in &program.nodes {
        if let NodeKind::Ui(ui) = &node.kind {
            ui_nodes.insert(node.id.clone(), ui);
            ui_order.push(node.id.clone());
            if let UiBody::Children(children) = &ui.body {
                for ch in children {
                    referenced_children.insert(ch.clone());
                }
            }
        }
    }

    let roots: Vec<NodeId> = ui_order
        .iter()
        .filter(|id| !referenced_children.contains(*id))
        .cloned()
        .collect();

    let mut taffy: TaffyTree<()> = TaffyTree::new();
    let mut id_to_taffy: HashMap<NodeId, taffy::NodeId> = HashMap::new();
    let mut parent_map: HashMap<NodeId, NodeId> = HashMap::new();
    let mut scrollable_map: HashMap<NodeId, bool> = HashMap::new();
    let mut visiting: HashSet<NodeId> = HashSet::new();

    #[allow(clippy::too_many_arguments)]
    fn build_node(
        node_id: &NodeId,
        ui_nodes: &HashMap<NodeId, &UiNode>,
        signals: Option<&SignalStore>,
        taffy: &mut TaffyTree<()>,
        id_to_taffy: &mut HashMap<NodeId, taffy::NodeId>,
        parent_map: &mut HashMap<NodeId, NodeId>,
        scrollable_map: &mut HashMap<NodeId, bool>,
        visiting: &mut HashSet<NodeId>,
    ) -> Result<Option<taffy::NodeId>, UiError> {
        if let Some(&existing) = id_to_taffy.get(node_id) {
            return Ok(Some(existing));
        }
        if !visiting.insert(node_id.clone()) {
            return Err(UiError::CircularLayout(node_id.clone()));
        }

        let Some(ui) = ui_nodes.get(node_id) else {
            visiting.remove(node_id);
            return Ok(None);
        };

        let mut child_taffy_ids = Vec::new();
        if let UiBody::Children(children) = &ui.body {
            for ch in children {
                parent_map.insert(ch.clone(), node_id.clone());
                if let Some(ch_tid) = build_node(
                    ch,
                    ui_nodes,
                    signals,
                    taffy,
                    id_to_taffy,
                    parent_map,
                    scrollable_map,
                    visiting,
                )? {
                    child_taffy_ids.push(ch_tid);
                }
            }
        }

        let (mut style, scrollable) = ui_node_to_style(ui, signals);
        // Direct children of a ROUTER are overlaid full-size screens so all screens
        // are statically checked for overflow without stacking vertically.
        if let Some(pid) = parent_map.get(node_id) {
            if let Some(parent_ui) = ui_nodes.get(pid) {
                if parent_ui.widget == "ROUTER" {
                    style.position = Position::Absolute;
                    style.inset = Rect {
                        left: length(0.0_f32),
                        right: length(0.0_f32),
                        top: length(0.0_f32),
                        bottom: length(0.0_f32),
                    };
                }
            }
        }
        scrollable_map.insert(node_id.clone(), scrollable);

        let tid = taffy
            .new_with_children(style, &child_taffy_ids)
            .map_err(|e| UiError::Layout(format!("taffy build failed for `{node_id}`: {e}")))?;
        id_to_taffy.insert(node_id.clone(), tid);
        visiting.remove(node_id);
        Ok(Some(tid))
    }

    let mut root_taffy_ids = Vec::new();
    for root in &roots {
        if let Some(tid) = build_node(
            root,
            &ui_nodes,
            signals,
            &mut taffy,
            &mut id_to_taffy,
            &mut parent_map,
            &mut scrollable_map,
            &mut visiting,
        )? {
            root_taffy_ids.push((root.clone(), tid));
        }
    }

    let available = Size {
        width: AvailableSpace::Definite(viewport.width),
        height: AvailableSpace::Definite(viewport.height),
    };

    let mut boxes = BTreeMap::new();
    let mut paint_order = Vec::new();

    #[allow(clippy::too_many_arguments)]
    fn collect_boxes(
        node_id: &NodeId,
        offset_x: f32,
        offset_y: f32,
        ui_nodes: &HashMap<NodeId, &UiNode>,
        taffy: &TaffyTree<()>,
        id_to_taffy: &HashMap<NodeId, taffy::NodeId>,
        parent_map: &HashMap<NodeId, NodeId>,
        scrollable_map: &HashMap<NodeId, bool>,
        boxes: &mut BTreeMap<NodeId, LayoutBox>,
        paint_order: &mut Vec<NodeId>,
    ) -> Result<(), UiError> {
        let Some(&tid) = id_to_taffy.get(node_id) else {
            return Ok(());
        };
        let layout = taffy
            .layout(tid)
            .map_err(|e| UiError::Layout(format!("taffy layout read failed: {e}")))?;
        let abs_x = offset_x + layout.location.x;
        let abs_y = offset_y + layout.location.y;
        let widget = ui_nodes
            .get(node_id)
            .map(|u| u.widget.clone())
            .unwrap_or_default();

        paint_order.push(node_id.clone());
        boxes.insert(
            node_id.clone(),
            LayoutBox {
                node_id: node_id.clone(),
                widget,
                x: abs_x,
                y: abs_y,
                width: layout.size.width,
                height: layout.size.height,
                parent: parent_map.get(node_id).cloned(),
                scrollable: scrollable_map.get(node_id).copied().unwrap_or(false),
            },
        );

        if let Some(ui) = ui_nodes.get(node_id) {
            if let UiBody::Children(children) = &ui.body {
                for ch in children {
                    collect_boxes(
                        ch,
                        abs_x,
                        abs_y,
                        ui_nodes,
                        taffy,
                        id_to_taffy,
                        parent_map,
                        scrollable_map,
                        boxes,
                        paint_order,
                    )?;
                }
            }
        }
        Ok(())
    }

    for (root_id, tid) in root_taffy_ids {
        taffy
            .compute_layout(tid, available)
            .map_err(|e| UiError::Layout(format!("taffy compute_layout failed: {e}")))?;
        collect_boxes(
            &root_id,
            0.0,
            0.0,
            &ui_nodes,
            &taffy,
            &id_to_taffy,
            &parent_map,
            &scrollable_map,
            &mut boxes,
            &mut paint_order,
        )?;
    }

    Ok(LayoutTree {
        viewport,
        roots,
        boxes,
        paint_order,
    })
}

fn ui_node_to_style(ui: &UiNode, signals: Option<&SignalStore>) -> (Style, bool) {
    let mut scrollable = false;
    match ui.widget.as_str() {
        "ROOT" => {
            // ROOT[pad_x, pad_y, width, height, bg_hex]
            let pad_x = arg_as_f32(ui.args.first()).unwrap_or(0.0);
            let pad_y = arg_as_f32(ui.args.get(1)).unwrap_or(0.0);
            let width = arg_as_dim(ui.args.get(2)).unwrap_or_else(|| percent(1.0_f32));
            let height = arg_as_dim(ui.args.get(3)).unwrap_or_else(|| percent(1.0_f32));
            let style = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                size: Size { width, height },
                padding: Rect {
                    left: length(pad_x),
                    right: length(pad_x),
                    top: length(pad_y),
                    bottom: length(pad_y),
                },
                ..Default::default()
            };
            (style, false)
        }
        "FLEX" => {
            // FLEX[dir(v|h), justify(c|s|e|sb|sa), align(c|s|e|st), gap, optional scroll]
            let flex_direction = match arg_as_ident(ui.args.first()) {
                Some("h") | Some("row") => FlexDirection::Row,
                _ => FlexDirection::Column,
            };
            let justify_content = Some(match arg_as_ident(ui.args.get(1)) {
                Some("c") | Some("center") => JustifyContent::Center,
                Some("e") | Some("end") => JustifyContent::FlexEnd,
                Some("sb") => JustifyContent::SpaceBetween,
                Some("sa") => JustifyContent::SpaceAround,
                _ => JustifyContent::FlexStart,
            });
            let align_items = Some(match arg_as_ident(ui.args.get(2)) {
                Some("c") | Some("center") => AlignItems::Center,
                Some("e") | Some("end") => AlignItems::FlexEnd,
                Some("st") | Some("stretch") => AlignItems::Stretch,
                _ => AlignItems::FlexStart,
            });
            let gap_px = arg_as_f32(ui.args.get(3)).unwrap_or(0.0);
            if ui
                .args
                .iter()
                .any(|a| matches!(a, UiArg::Ident(s) if s == "scroll"))
            {
                scrollable = true;
            }
            let style = Style {
                display: Display::Flex,
                flex_direction,
                justify_content,
                align_items,
                gap: Size {
                    width: length(gap_px),
                    height: length(gap_px),
                },
                size: Size {
                    width: percent(1.0_f32),
                    height: percent(1.0_f32),
                },
                ..Default::default()
            };
            (style, scrollable)
        }
        "BTN" => {
            // BTN[width, height, radius, bg_hex]
            let width = arg_as_dim(ui.args.first()).unwrap_or_else(|| length(120.0_f32));
            let height = arg_as_dim(ui.args.get(1)).unwrap_or_else(|| length(44.0_f32));
            let style = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::Center),
                flex_shrink: 0.0,
                size: Size { width, height },
                ..Default::default()
            };
            (style, false)
        }
        "TXT" => {
            // TXT[font_size, weight, color_hex]
            let font_size = arg_as_f32(ui.args.first()).unwrap_or(16.0);
            let text = match &ui.body {
                UiBody::Text { template, .. } => match signals {
                    Some(store) => store.resolve_text(template),
                    None => template.clone(),
                },
                _ => String::new(),
            };
            let char_count = text.chars().count().max(1) as f32;
            let est_width = char_count * font_size * 0.55;
            let est_height = font_size * 1.25;
            let style = Style {
                flex_shrink: 0.0,
                size: Size {
                    width: length(est_width),
                    height: length(est_height),
                },
                ..Default::default()
            };
            (style, false)
        }
        "INP" => {
            // INP[width, height, radius, bg_hex, #S_bound?]{"placeholder"}
            let width = arg_as_dim(ui.args.first()).unwrap_or_else(|| length(240.0_f32));
            let height = arg_as_dim(ui.args.get(1)).unwrap_or_else(|| length(44.0_f32));
            let style = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                flex_shrink: 0.0,
                size: Size { width, height },
                ..Default::default()
            };
            (style, false)
        }
        "LIST" => {
            // LIST[dir(v|h), gap, noscroll?]{#N_items...}
            let flex_direction = match arg_as_ident(ui.args.first()) {
                Some("h") | Some("row") => FlexDirection::Row,
                _ => FlexDirection::Column,
            };
            let gap_px = arg_as_f32(ui.args.get(1)).unwrap_or(8.0);
            let is_scrollable = !ui
                .args
                .iter()
                .any(|a| matches!(a, UiArg::Ident(s) if s == "noscroll"));
            let style = Style {
                display: Display::Flex,
                flex_direction,
                gap: Size {
                    width: length(gap_px),
                    height: length(gap_px),
                },
                flex_grow: 1.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            };
            (style, is_scrollable)
        }
        "NAV" => {
            // NAV[width, height, bg_hex]{#N_tabs...}
            let width = arg_as_dim(ui.args.first()).unwrap_or_else(|| percent(1.0_f32));
            let height = arg_as_dim(ui.args.get(1)).unwrap_or_else(|| length(56.0_f32));
            let style = Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                justify_content: Some(JustifyContent::SpaceAround),
                align_items: Some(AlignItems::Center),
                flex_shrink: 0.0,
                size: Size { width, height },
                ..Default::default()
            };
            (style, false)
        }
        "ROUTER" => {
            // ROUTER[#N_initial_screen?]{#N_screen1, #N_screen2, ...}
            let style = Style {
                display: Display::Flex,
                flex_grow: 1.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: percent(1.0_f32),
                },
                ..Default::default()
            };
            (style, false)
        }
        _ => {
            let width = arg_as_dim(ui.args.first()).unwrap_or_else(auto);
            let height = arg_as_dim(ui.args.get(1)).unwrap_or_else(auto);
            let style = Style {
                display: Display::Flex,
                flex_shrink: 0.0,
                size: Size { width, height },
                ..Default::default()
            };
            (style, false)
        }
    }
}

fn arg_as_f32(arg: Option<&UiArg>) -> Option<f32> {
    match arg {
        Some(UiArg::Int(n)) => Some(*n as f32),
        Some(UiArg::Float(f)) => Some(*f as f32),
        _ => None,
    }
}

fn arg_as_dim(arg: Option<&UiArg>) -> Option<Dimension> {
    match arg {
        Some(UiArg::Int(n)) => Some(length(*n as f32)),
        Some(UiArg::Float(f)) => Some(length(*f as f32)),
        Some(UiArg::Percent(p)) => Some(percent((*p as f32) / 100.0)),
        _ => None,
    }
}

fn arg_as_ident(arg: Option<&UiArg>) -> Option<&str> {
    match arg {
        Some(UiArg::Ident(s)) => Some(s.as_str()),
        _ => None,
    }
}
