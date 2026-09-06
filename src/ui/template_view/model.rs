use std::collections::HashMap;
use std::sync::Arc;

use logotomy::core::document::LogDocument;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemplateSort {
    #[default]
    Count,
    FirstSeen,
    LastSeen,
    Rarity,
}

impl TemplateSort {
    pub const ALL: [Self; 4] = [Self::Count, Self::FirstSeen, Self::LastSeen, Self::Rarity];

    pub fn label(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::FirstSeen => "First seen",
            Self::LastSeen => "Last seen",
            Self::Rarity => "Rarity",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TemplateRow {
    pub template_index: usize,
    pub first_line: usize,
    pub last_line: usize,
    pub occurrences: Vec<usize>,
    pub rare: bool,
    pub late: bool,
    pub bursty: bool,
}

/// Per-tab, invalidation-driven state for the Templates view. Sorting and
/// occurrence discovery happen only when the document/trim changes, never in
/// the egui paint path.
#[derive(Default)]
pub struct TemplateBrowserState {
    pub query: String,
    pub sort: TemplateSort,
    pub descending: bool,
    pub selected_id: Option<u32>,
    signature: Option<(u64, usize, usize, usize, usize)>,
    pub rows: Vec<TemplateRow>,
    pub order: Vec<usize>,
    order_generation: u64,
    visible_cache: Option<(u64, String, Arc<Vec<usize>>)>,
}

impl TemplateBrowserState {
    pub fn refresh(&mut self, doc: &LogDocument) {
        let signature = (
            doc.file_size,
            doc.trim_start,
            doc.trim_end,
            doc.templates.len(),
            doc.templates.iter().map(|template| template.count).sum(),
        );
        if self.signature == Some(signature) {
            return;
        }
        self.signature = Some(signature);
        let by_id: HashMap<u32, usize> = doc
            .templates
            .iter()
            .enumerate()
            .map(|(index, template)| (template.id, index))
            .collect();
        self.rows = doc
            .templates
            .iter()
            .enumerate()
            .map(|(template_index, _)| TemplateRow {
                template_index,
                ..Default::default()
            })
            .collect();
        for line in 0..doc.total_lines() {
            let id = doc.template_at(line);
            if let Some(&index) = by_id.get(&id) {
                let row = &mut self.rows[index];
                if row.occurrences.is_empty() {
                    row.first_line = line;
                }
                row.last_line = line;
                row.occurrences.push(line);
            }
        }
        let total = doc.total_lines().max(1);
        let rare_max = (total / 1000).max(1);
        let late_from = total * 9 / 10;
        for row in &mut self.rows {
            row.rare = row.occurrences.len() <= rare_max && !row.occurrences.is_empty();
            row.late = !row.occurrences.is_empty() && row.first_line >= late_from;
            if row.occurrences.len() >= 10 {
                let mut buckets = [0usize; 50];
                for &line in &row.occurrences {
                    buckets[(line * 50 / total).min(49)] += 1;
                }
                row.bursty = buckets.into_iter().max().unwrap_or(0) * 2 >= row.occurrences.len();
            }
        }
        self.resort(doc);
    }

    pub fn resort(&mut self, doc: &LogDocument) {
        self.order = (0..self.rows.len()).collect();
        self.order.sort_by(|&left, &right| {
            let a = &self.rows[left];
            let b = &self.rows[right];
            let compare = match self.sort {
                TemplateSort::Count => doc.templates[b.template_index]
                    .count
                    .cmp(&doc.templates[a.template_index].count),
                TemplateSort::FirstSeen => a.first_line.cmp(&b.first_line),
                TemplateSort::LastSeen => b.last_line.cmp(&a.last_line),
                TemplateSort::Rarity => doc.templates[a.template_index]
                    .count
                    .cmp(&doc.templates[b.template_index].count),
            };
            let compare = compare.then_with(|| {
                doc.templates[a.template_index]
                    .id
                    .cmp(&doc.templates[b.template_index].id)
            });
            if self.descending {
                compare.reverse()
            } else {
                compare
            }
        });
        self.order_generation = self.order_generation.wrapping_add(1);
        self.visible_cache = None;
    }

    pub fn visible_order(&mut self, doc: &LogDocument) -> Arc<Vec<usize>> {
        let query = self.query.trim().to_ascii_lowercase();
        if let Some((generation, cached_query, cached)) = &self.visible_cache {
            if *generation == self.order_generation && *cached_query == query {
                return Arc::clone(cached);
            }
        }
        let visible = Arc::new(
            self.order
                .iter()
                .copied()
                .filter(|&row_index| {
                    if query.is_empty() {
                        return true;
                    }
                    let template = &doc.templates[self.rows[row_index].template_index];
                    template.id.to_string().contains(&query)
                        || format!("t{}", template.id).contains(&query)
                        || template.pattern.to_ascii_lowercase().contains(&query)
                })
                .collect(),
        );
        self.visible_cache = Some((self.order_generation, query, Arc::clone(&visible)));
        visible
    }

    pub fn row_for_id(&self, doc: &LogDocument, id: u32) -> Option<&TemplateRow> {
        self.rows
            .iter()
            .find(|row| doc.templates[row.template_index].id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn visible_order_reuses_the_cached_allocation_until_query_changes() {
        let path = std::env::temp_dir().join(format!(
            "logotomy-template-cache-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "2026-01-01T00:00:00Z INFO alpha").unwrap();
        writeln!(file, "2026-01-01T00:00:01Z ERROR beta").unwrap();
        drop(file);
        let doc = LogDocument::open(&path).unwrap();
        let mut browser = TemplateBrowserState::default();
        browser.refresh(&doc);
        let first = browser.visible_order(&doc);
        let second = browser.visible_order(&doc);
        assert!(Arc::ptr_eq(&first, &second));
        browser.query = "beta".to_string();
        let filtered = browser.visible_order(&doc);
        assert!(!Arc::ptr_eq(&first, &filtered));
        std::fs::remove_file(path).ok();
    }
}
