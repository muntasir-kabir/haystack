//! Data structure for a saved text filter: a named set of filter terms.
//!
//! The GUI's "Saved filters" feature stores each named filter set in this
//! format. This is distinct from Drain template mining (`document.rs`), which
//! is untouched.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedFilter {
    pub filters: Vec<String>,
}
