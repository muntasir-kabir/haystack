use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "JSON",
    title: "Embedded JSON",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_json() {
        assert_eq!(super::PRESENTATION.badge, "JSON");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
