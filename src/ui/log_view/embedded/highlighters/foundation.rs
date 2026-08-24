use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "APPLE",
    title: "Foundation description",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_apple() {
        assert_eq!(super::PRESENTATION.badge, "APPLE");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
