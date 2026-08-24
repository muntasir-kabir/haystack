use super::super::Presentation;
pub const PRESENTATION: Presentation = Presentation {
    badge: "TRACE",
    title: "Stack trace",
    primary_tab: "Frames",
    explicit_decode: false,
};
#[cfg(test)]
mod tests {
    #[test]
    fn visualization_badge_is_trace() {
        assert_eq!(super::PRESENTATION.badge, "TRACE");
        assert_eq!(super::PRESENTATION.primary_tab, "Frames");
    }
}
