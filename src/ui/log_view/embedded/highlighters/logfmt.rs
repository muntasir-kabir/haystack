use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "KV",
    title: "Embedded logfmt",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_kv() {
        assert_eq!(super::PRESENTATION.badge, "KV");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
