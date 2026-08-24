use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "FIELDS",
    title: "Embedded fields",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_fields() {
        assert_eq!(super::PRESENTATION.badge, "FIELDS");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
