use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "HEX",
    title: "Hex payload",
    primary_tab: "Summary",
    explicit_decode: true,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_hex() {
        assert_eq!(super::PRESENTATION.badge, "HEX");
        assert!(super::PRESENTATION.explicit_decode);
    }
}
