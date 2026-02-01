use std::collections::HashMap;

use ratatui::style::Style;

#[derive(Debug, Clone, Copy)]
pub struct WorldBounds {
    pub min: (f32, f32),
    pub max: (f32, f32),
}

impl Default for WorldBounds {
    fn default() -> Self {
        Self {
            min: (0.0, 0.0),
            max: (0.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub world_bounds: WorldBounds,
    pub objects: Vec<Object>,
    pub overlays: Vec<Overlay>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Node,
    Edge,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Object {
    pub id: String,
    pub kind: ObjectKind,
    pub pos: (f32, f32),
    pub radius: f32,
    pub style: Style,
    pub label: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl Default for Object {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: ObjectKind::Unknown,
            pos: (0.0, 0.0),
            radius: 0.0,
            style: Style::default(),
            label: None,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    Label,
    Grid,
    Cursor,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Overlay {
    pub kind: OverlayKind,
    pub label: Option<String>,
    pub metadata: HashMap<String, String>,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            kind: OverlayKind::Unknown,
            label: None,
            metadata: HashMap::new(),
        }
    }
}
