use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "PROTO",
    title: "Protobuf text",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_proto() {
        assert_eq!(super::PRESENTATION.badge, "PROTO");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
