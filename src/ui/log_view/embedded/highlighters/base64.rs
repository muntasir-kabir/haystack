use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "B64",
    title: "Base64 payload",
    primary_tab: "Summary",
    explicit_decode: true,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_base64() {
        assert_eq!(super::PRESENTATION.badge, "B64");
        assert!(super::PRESENTATION.explicit_decode);
    }
}
