//! EPUB stylesheet stack. Publisher CSS, then the theme, then the user.

use folio_reader::{stack_css, theme_css, CssError, Theme};

/// CSS for one chapter document.
///
/// # Errors
///
/// Returns an error when `user` contains a script element.
pub fn epub_document_css(
    publisher: &str,
    theme: Theme,
    custom_theme: &str,
    user: &str,
) -> Result<String, CssError> {
    stack_css(publisher, theme_css(theme, custom_theme), user)
}

#[cfg(test)]
mod tests {
    use super::epub_document_css;
    use folio_reader::Theme;

    #[test]
    fn epub_user_css_is_last_and_cannot_add_a_script() {
        let css = epub_document_css("publisher{}", Theme::Dark, "", "user{}").expect("css");
        assert!(css.starts_with("publisher{}"));
        assert!(css.ends_with("user{}"));
        assert!(css.find("publisher{}").expect("publisher") < css.find("user{}").expect("user"));
        assert!(epub_document_css("ok", Theme::Light, "", "<Script src='x'>").is_err());
    }

    #[test]
    fn epub_chrome_stylesheet_has_no_script() {
        let css = include_str!("../../../../assets/epub-chrome.css");
        let folded = css.to_ascii_lowercase();
        assert!(!folded.contains("<script"));
        assert!(!folded.contains("dioxus"));
        assert!(css.contains("line-height"));
    }
}
