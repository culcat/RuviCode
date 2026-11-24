use std::sync::Arc;

use ratatui::style::Color;
use ruvi_core::{ExtensionRegistry, ThemePalette, UiTheme};

/// Default dark palette inspired by `OpenCode`'s terminal UI.
pub struct OpenCodeDarkTheme;

impl UiTheme for OpenCodeDarkTheme {
    fn id(&self) -> &'static str {
        "opencode-dark"
    }

    fn display_name(&self) -> &'static str {
        "OpenCode Dark"
    }

    fn palette(&self) -> ThemePalette {
        ThemePalette {
            background: (13, 17, 23),
            surface: (22, 27, 34),
            border: (48, 54, 61),
            text: (230, 237, 243),
            muted: (139, 148, 158),
            accent: (88, 166, 255),
            success: (63, 185, 80),
            warning: (210, 153, 34),
        }
    }
}

/// Alternative Catppuccin Mocha palette demonstrating OCP theme swapping.
pub struct CatppuccinMochaTheme;

impl UiTheme for CatppuccinMochaTheme {
    fn id(&self) -> &'static str {
        "catppuccin-mocha"
    }

    fn display_name(&self) -> &'static str {
        "Catppuccin Mocha"
    }

    fn palette(&self) -> ThemePalette {
        ThemePalette {
            background: (30, 30, 46),
            surface: (49, 50, 68),
            border: (88, 91, 112),
            text: (205, 214, 244),
            muted: (166, 173, 200),
            accent: (137, 180, 250),
            success: (166, 227, 161),
            warning: (249, 226, 175),
        }
    }
}

/// Converts an RGB tuple from [`ThemePalette`] into a [`ratatui::style::Color`].
#[must_use]
pub const fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// Registers built-in TUI themes into [`ExtensionRegistry`].
pub fn register_default_themes(registry: &mut ExtensionRegistry) {
    registry.register_theme(Arc::new(OpenCodeDarkTheme));
    registry.register_theme(Arc::new(CatppuccinMochaTheme));
}
