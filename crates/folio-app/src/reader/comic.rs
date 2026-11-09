//! Comic open options. Direction may come from storage; zoom does not.

use folio_reader::{resolve_direction, ReadDirection};

/// Direction for one book. A stored series override wins over ComicInfo.
pub fn opening_direction(comic_info_xml: &str, stored: Option<ReadDirection>) -> ReadDirection {
    resolve_direction(comic_info_xml, stored)
}

#[cfg(test)]
mod tests {
    use super::opening_direction;
    use folio_reader::ReadDirection;

    #[test]
    fn comic_opening_direction_uses_the_stored_override() {
        let xml = r#"<ComicInfo Manga="YesRTL"></ComicInfo>"#;
        assert_eq!(opening_direction(xml, None), ReadDirection::Rtl);
        assert_eq!(
            opening_direction(xml, Some(ReadDirection::Ltr)),
            ReadDirection::Ltr
        );
    }
}
