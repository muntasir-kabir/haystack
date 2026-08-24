use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "HTTP",
    title: "HTTP structure",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_http() {
        assert_eq!(super::PRESENTATION.badge, "HTTP");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
