use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "JVM",
    title: "JVM debug value",
    primary_tab: "Tree",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_jvm() {
        assert_eq!(super::PRESENTATION.badge, "JVM");
        assert_eq!(super::PRESENTATION.primary_tab, "Tree");
    }
}
