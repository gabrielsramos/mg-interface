use crate::runtime::inspection::{InspectionState, SceneSnapshot};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct EntityBrowser {
    snapshot: Option<SceneSnapshot>,
    pub selected: Option<String>,
    collapsed: HashSet<String>,
    connected: bool,
}

pub struct EntityRow {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
}
impl EntityBrowser {
    pub fn update(&mut self, state: InspectionState) {
        self.connected = state.connected;
        let Some(snapshot) = state.snapshot.filter(|_| self.connected) else {
            self.snapshot = None;
            self.selected = None;
            self.collapsed.clear();
            return;
        };
        let ids: HashSet<_> = snapshot
            .entities
            .iter()
            .filter_map(|e| e["id"].as_str())
            .collect();
        if self.selected.as_deref().is_some_and(|id| !ids.contains(id)) {
            self.selected = None;
        }
        self.collapsed.retain(|id| ids.contains(id.as_str()));
        self.snapshot = Some(snapshot);
    }
    pub fn toggle(&mut self, id: &str) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_owned());
        }
    }
    pub fn rows(&self, ui: bool) -> Vec<EntityRow> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let nodes: HashMap<_, _> = snapshot
            .entities
            .iter()
            .filter_map(|e| Some((e["id"].as_str()?, e)))
            .collect();
        let roots = if ui {
            &snapshot.ui_roots
        } else {
            &snapshot.world_roots
        };
        let mut pending: Vec<_> = roots.iter().rev().map(|id| (id.as_str(), 0)).collect();
        let mut visited = HashSet::new();
        let mut rows = Vec::new();
        while let Some((id, depth)) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(entity) = nodes.get(id) else {
                continue;
            };
            let children: Vec<_> = entity["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .collect();
            let collapsed = self.collapsed.contains(id);
            rows.push(EntityRow {
                id: id.to_owned(),
                depth,
                has_children: !children.is_empty(),
                collapsed,
                label: entity["name"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!("Entity {}", id.chars().take(8).collect::<String>())
                    }),
            });
            if !collapsed {
                pending.extend(children.into_iter().rev().map(|id| (id, depth + 1)));
            }
        }
        rows
    }
    pub fn properties(&self) -> Vec<(String, String)> {
        let Some(entity) = self.snapshot.as_ref().and_then(|s| {
            s.entities
                .iter()
                .find(|e| e["id"].as_str() == self.selected.as_deref())
        }) else {
            return Vec::new();
        };
        let mut values = Vec::new();
        for (key, label) in [
            ("name", "Name"),
            ("id", "ID"),
            ("parentId", "Parent"),
            ("pass", "Render pass"),
            ("position", "Local position"),
            ("worldPosition", "World position"),
            ("color", "Color"),
            ("sourceRectangle", "Source rectangle"),
            ("origin", "Origin"),
            ("effects", "Sprite effects"),
            ("layerDepth", "Layer depth"),
            ("texture", "Texture"),
            ("tags", "Tags"),
        ] {
            let value = &entity[key];
            let text = if value.is_null() {
                "None".into()
            } else if let Some(s) = value.as_str() {
                s.to_owned()
            } else if let Some(o) = value.as_object() {
                if o.is_empty() {
                    "None".into()
                } else {
                    o.iter()
                        .map(|(k, v)| {
                            format!(
                                "{k}: {}",
                                v.as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| v.to_string())
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            } else {
                value.to_string()
            };
            values.push((label.to_owned(), text));
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(sequence: u64) -> InspectionState {
        InspectionState { connected: true, error: None, snapshot: Some(serde_json::from_value(serde_json::json!({
            "version":1, "type":"sceneSnapshot", "sequence":sequence,
            "worldRoots":["root"], "uiRoots":["ui"], "worldTransform":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],
            "entities":[{"id":"root", "name":"Grid", "children":["child"]},
                {"id":"child", "parentId":"root", "children":[], "position":{"x":sequence,"y":3}},
                {"id":"ui", "name":"HUD", "children":[]}]
        })).unwrap()) }
    }
    #[test]
    fn hierarchy_collapse_and_live_selection() {
        let mut browser = EntityBrowser::default();
        browser.update(state(1));
        assert_eq!(
            browser
                .rows(false)
                .iter()
                .map(|r| r.depth)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(browser.rows(true)[0].label, "HUD");
        browser.selected = Some("child".into());
        browser.toggle("root");
        browser.update(state(2));
        assert_eq!(browser.rows(false).len(), 1);
        assert_eq!(browser.selected.as_deref(), Some("child"));
        assert!(
            browser
                .properties()
                .iter()
                .any(|(k, v)| k == "Local position" && v.contains("x: 2"))
        );
        let mut removed = state(3);
        removed.snapshot.as_mut().unwrap().entities.remove(1);
        browser.update(removed);
        assert!(browser.selected.is_none());
        browser.update(InspectionState::default());
        assert!(browser.rows(false).is_empty());
    }
    #[test]
    fn malformed_cycles_do_not_loop_and_unnamed_nodes_have_labels() {
        let mut state = state(1);
        state.snapshot.as_mut().unwrap().entities[1]["children"] =
            serde_json::json!(["root", "missing"]);
        let mut browser = EntityBrowser::default();
        browser.update(state);
        assert_eq!(browser.rows(false).len(), 2);
        assert_eq!(browser.rows(false)[1].label, "Entity child");
    }
}
