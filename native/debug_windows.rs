use crate::runtime::inspection::{DebugWindow, DebugWindows};

#[derive(Default)]
pub struct DebugWindowBrowser {
    pub catalog: Option<DebugWindows>,
    pub selected: Option<String>,
}

impl DebugWindowBrowser {
    pub fn update(&mut self, catalog: Option<DebugWindows>) {
        if !catalog.as_ref().is_some_and(|catalog| {
            catalog
                .windows
                .iter()
                .any(|window| Some(&window.id) == self.selected.as_ref())
        }) {
            self.selected = catalog
                .as_ref()
                .and_then(|catalog| catalog.windows.first())
                .map(|window| window.id.clone());
        }
        self.catalog = catalog;
    }

    pub fn selected_window(&self) -> Option<&DebugWindow> {
        self.catalog
            .as_ref()?
            .windows
            .iter()
            .find(|window| Some(&window.id) == self.selected.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog(ids: &[&str]) -> Option<DebugWindows> {
        Some(DebugWindows {
            revision: 1,
            errors: vec![],
            windows: ids
                .iter()
                .map(|id| DebugWindow {
                    id: (*id).into(),
                    title: (*id).into(),
                    elements: vec![],
                })
                .collect(),
        })
    }
    #[test]
    fn selection_survives_refresh_and_clears_on_disconnect() {
        let mut browser = DebugWindowBrowser::default();
        browser.update(catalog(&["a", "b"]));
        assert_eq!(browser.selected.as_deref(), Some("a"));
        browser.selected = Some("b".into());
        browser.update(catalog(&["a", "b", "c"]));
        assert_eq!(browser.selected_window().unwrap().id, "b");
        browser.update(catalog(&["a"]));
        assert_eq!(browser.selected.as_deref(), Some("a"));
        browser.update(catalog(&[]));
        assert!(browser.selected.is_none());
        browser.update(catalog(&["a"]));
        browser.update(None);
        assert!(browser.selected_window().is_none());
    }
}
