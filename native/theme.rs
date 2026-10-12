use serde::{Deserialize, Deserializer};
use std::path::PathBuf;
const DEFAULT: &str = include_str!("../themes/default.toml");

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u32);
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let hex = value
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| serde::de::Error::custom("Expected a color in #RRGGBB format"))?;
        u32::from_str_radix(hex, 16)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub colors: Colors,
    pub buttons: Buttons,
    pub panels: Panels,
    pub typography: Typography,
    pub spacing: Spacing,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    pub background: Color,
    pub panel: Color,
    pub viewport: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub secondary: Color,
    pub disabled: Color,
    pub error: Color,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Buttons {
    pub corner_radius: f32,
    pub padding_horizontal: f32,
    pub padding_vertical: f32,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Panels {
    pub corner_radius: f32,
    pub padding: f32,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Typography {
    pub font_family: String,
    pub font_size: f32,
    pub small_font_size: f32,
}
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Spacing {
    pub page_padding: f32,
    pub workspace_padding: f32,
    pub gap: f32,
    pub toolbar_gap: f32,
    pub launcher_toolbar_height: f32,
    pub workspace_toolbar_height: f32,
    pub status_height: f32,
    pub sidebar_width: f32,
}
impl Theme {
    fn parse(source: &str) -> Result<Self, String> {
        let theme: Self = toml::from_str(source).map_err(|e| e.to_string())?;
        let b = &theme.buttons;
        let p = &theme.panels;
        let t = &theme.typography;
        let s = &theme.spacing;
        let numbers = [
            b.corner_radius,
            b.padding_horizontal,
            b.padding_vertical,
            p.corner_radius,
            p.padding,
            t.font_size,
            t.small_font_size,
            s.page_padding,
            s.workspace_padding,
            s.gap,
            s.toolbar_gap,
            s.launcher_toolbar_height,
            s.workspace_toolbar_height,
            s.status_height,
            s.sidebar_width,
        ];
        if numbers
            .iter()
            .any(|n| !n.is_finite() || *n < 0. || *n > 4096.)
        {
            return Err("Sizes must be finite and between 0 and 4096".into());
        }
        if t.font_family.trim().is_empty()
            || t.font_size == 0.
            || t.small_font_size == 0.
            || s.sidebar_width == 0.
            || s.launcher_toolbar_height == 0.
            || s.workspace_toolbar_height == 0.
            || s.status_height == 0.
        {
            return Err(
                "Font family, font sizes and panel/toolbar dimensions must not be empty or zero"
                    .into(),
            );
        }
        Ok(theme)
    }
}
// A single store is shared by both views. Content comparison handles coarse file
// timestamps, atomic-save replacements, deletion and later recreation.
pub struct ThemeStore {
    pub theme: Theme,
    pub error: Option<String>,
    pub path: PathBuf,
    last_source: Option<Result<String, String>>,
}
impl ThemeStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            theme: Theme::parse(DEFAULT).expect("Invalid built-in theme"),
            error: None,
            path,
            last_source: None,
        }
    }
    pub fn apply(&mut self, source: Result<String, String>) -> bool {
        if self.last_source.as_ref() == Some(&source) {
            return false;
        }
        self.last_source = Some(source.clone());
        match source.and_then(|source| Theme::parse(&source)) {
            Ok(theme) => {
                self.theme = theme;
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("Theme error ({}): {error}", self.path.display()));
            }
        }
        true
    }
}
#[cfg(windows)]
pub mod styles {
    use super::Theme;
    use gpui::{Styled, px, rgb};
    pub trait ThemeStyled: Styled + Sized {
        fn app_style(self, theme: &Theme) -> Self {
            self.bg(rgb(theme.colors.background.0))
                .text_color(rgb(theme.colors.text.0))
                .font_family(theme.typography.font_family.clone())
                .text_size(px(theme.typography.font_size))
        }
        fn panel_style(self, theme: &Theme) -> Self {
            self.p(px(theme.panels.padding))
                .rounded(px(theme.panels.corner_radius))
                .border_4()
                .border_color(rgb(theme.colors.muted.0))
                .bg(rgb(theme.colors.panel.0))
        }
        fn button_style(self, theme: &Theme, primary: bool, enabled: bool) -> Self {
            let color = if !enabled {
                theme.colors.disabled
            } else if primary {
                theme.colors.accent
            } else {
                theme.colors.secondary
            };
            self.px(px(theme.buttons.padding_horizontal))
                .py(px(theme.buttons.padding_vertical))
                .rounded(px(theme.buttons.corner_radius))
                .bg(rgb(color.0))
        }
        fn small_style(self, theme: &Theme) -> Self {
            self.text_size(px(theme.typography.small_font_size))
                .text_color(rgb(theme.colors.muted.0))
        }
    }
    impl<T: Styled> ThemeStyled for T {}
}
#[cfg(windows)]
pub fn start(cx: &mut gpui::App) -> gpui::Entity<ThemeStore> {
    use gpui::AppContext;
    let path = std::env::var_os("MG_INTERFACE_THEME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes/default.toml"));
    println!("Live theme: {}", path.display());
    let mut initial = ThemeStore::new(path.clone());
    initial.apply(std::fs::read_to_string(&path).map_err(|e| e.to_string()));
    let theme = cx.new(|_| initial);
    let watched = theme.downgrade();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let path = path.clone();
            let source = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(path).map_err(|e| e.to_string()) })
                .await;
            if watched
                .update(cx, |store, cx| {
                    if store.apply(source) {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
    theme
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reload_retains_last_valid_theme_and_recovers() {
        let mut store = ThemeStore::new(PathBuf::from("test.toml"));
        assert!(store.apply(Ok(DEFAULT.into())));
        assert!(!store.apply(Ok(DEFAULT.into())));
        let changed = DEFAULT
            .replace("#2563eb", "#ff00ff")
            .replace("sidebar_width = 240", "sidebar_width = 300");
        assert!(store.apply(Ok(changed.clone())));
        assert_eq!(store.theme.colors.accent, Color(0xff00ff));
        assert_eq!(store.theme.spacing.sidebar_width, 300.);
        let good = store.theme.clone();
        for bad in [
            changed.replace("#ff00ff", "bad"),
            changed.replace(
                &format!("\nfont_size = {}", good.typography.font_size),
                "\nfont_size = -1",
            ),
            changed.replace("gap = 16", "gap = nan"),
            String::from("[colors"),
            changed.clone() + "\nunknown = 4",
        ] {
            assert!(store.apply(Ok(bad)));
            assert!(store.error.is_some());
            assert_eq!(store.theme, good);
        }
        assert!(store.apply(Err("File temporarily missing".into())));
        assert_eq!(store.theme, good);
        assert!(store.apply(Ok(changed)));
        assert!(store.error.is_none());
    }
}
