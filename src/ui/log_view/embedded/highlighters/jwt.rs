use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "JWT",
    title: "JSON Web Token",
    primary_tab: "Summary",
    explicit_decode: true,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_jwt() {
        assert_eq!(super::PRESENTATION.badge, "JWT");
        assert!(super::PRESENTATION.explicit_decode);
    }
}
