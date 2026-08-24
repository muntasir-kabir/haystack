use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "PEM",
    title: "PEM certificate or key",
    primary_tab: "Summary",
    explicit_decode: true,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_pem() {
        assert_eq!(super::PRESENTATION.badge, "PEM");
        assert!(super::PRESENTATION.explicit_decode);
    }
}
