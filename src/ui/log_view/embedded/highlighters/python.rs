use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "PY",
    title: "Python literal",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_python() {
        assert_eq!(super::PRESENTATION.badge, "PY");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
