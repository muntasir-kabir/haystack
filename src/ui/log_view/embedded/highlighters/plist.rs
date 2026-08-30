use super::super::Presentation;

pub const PRESENTATION: Presentation = Presentation {
    badge: "PLIST",
    title: "Property list",
    primary_tab: "Tree",
    explicit_decode: false,
};

pub const BINARY_PRESENTATION: Presentation = Presentation {
    badge: "BPLIST",
    title: "Binary property list",
    primary_tab: "Summary",
    explicit_decode: true,
};

#[cfg(test)]
mod tests {
    #[test]
    fn plist_presentations_separate_structured_and_binary_values() {
        let plist = crate::ui::log_view::embedded::presentation("plist");
        let binary = crate::ui::log_view::embedded::presentation("binary-plist");
        assert_eq!(plist.badge, "PLIST");
        assert!(!plist.explicit_decode);
        assert_eq!(binary.badge, "BPLIST");
        assert_eq!(binary.primary_tab, "Summary");
        assert!(binary.explicit_decode);
    }
}
