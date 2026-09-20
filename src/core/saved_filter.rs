//! Data structure for a saved text filter: a named set of filter terms.
//!
//! The GUI's "Saved filters" feature stores each named filter set in this
//! format. This is distinct from Drain template mining (`document.rs`), which
//! is untouched.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFilter {
    pub filters: Vec<String>,
    #[serde(default)]
    pub field_queries: Vec<Option<crate::core::field_query::FieldQuery>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::field_query::FieldQuery;

    #[test]
    fn old_saved_filter_sets_default_to_text_and_field_sets_round_trip() {
        let old: SavedFilter = serde_json::from_str(r#"{"filters":["ERROR"]}"#).unwrap();
        assert!(old.field_queries.is_empty());
        let field = FieldQuery::parse("b >= 9", true).unwrap();
        let saved = SavedFilter {
            filters: vec!["b >= \"9\"".into()],
            field_queries: vec![Some(field.clone())],
        };
        let decoded: SavedFilter =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(decoded.field_queries[0], Some(field));
    }
}
