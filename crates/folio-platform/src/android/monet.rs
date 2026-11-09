//! Material You palette selection.
//!
//! Android 12 is API 31. Older APIs, and a missing palette, keep the existing theme.

/// A system palette. Empty primary means the palette is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPalette {
    primary: String,
}

impl SystemPalette {
    /// Store `primary` as given.
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
        }
    }

    /// Primary colour token.
    pub fn primary(&self) -> &str {
        &self.primary
    }
}

/// Theme source after the API and palette check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedTheme<'a> {
    /// Use the system primary on Android 12 or newer.
    Material {
        /// Palette primary.
        primary: &'a str,
    },
    /// Keep the theme Phase 10 already applied.
    Existing,
}

/// Pick Material You only when `api` is at least 31 and `palette` has a primary.
pub fn resolve_theme(api: u32, palette: Option<&SystemPalette>) -> ResolvedTheme<'_> {
    if api >= 31 {
        if let Some(palette) = palette {
            if !palette.primary.is_empty() {
                return ResolvedTheme::Material {
                    primary: palette.primary(),
                };
            }
        }
    }
    ResolvedTheme::Existing
}

#[cfg(test)]
mod tests {
    use super::{resolve_theme, ResolvedTheme, SystemPalette};

    #[test]
    fn material_you_needs_android_12_and_a_palette() {
        let palette = SystemPalette::new("primary");
        assert_eq!(resolve_theme(30, Some(&palette)), ResolvedTheme::Existing);
        assert_eq!(resolve_theme(31, None), ResolvedTheme::Existing);
        assert_eq!(
            resolve_theme(31, Some(&SystemPalette::new(""))),
            ResolvedTheme::Existing
        );
        assert_eq!(
            resolve_theme(31, Some(&palette)),
            ResolvedTheme::Material { primary: "primary" }
        );
    }
}
