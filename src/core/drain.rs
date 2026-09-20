//! Native Rust implementation of the Drain log-template mining algorithm
//! (He et al., ICWS 2017) — no Python, no Drain3, no venv, no regrets.
//!
//! Drain organizes log lines into a fixed-depth parse tree:
//!   depth 1: token count of the line
//!   depth 2..n-1: the first few tokens
//!   leaves: candidate template clusters, matched by token similarity
//!
//! Lines that match a cluster with similarity >= threshold are merged into it
//! (diverging token positions become the `<*>` wildcard), otherwise a new
//! cluster is born. The result: every line gets a template ID, and the app
//! can show "template + data" (the abstract pattern + the concrete lines).

use rustc_hash::FxHashMap;

/// Wildcard marker used inside mined templates.
pub const WILDCARD: &str = "<*>";

/// Lines longer than this are truncated before mining. A template's shape is
/// decided by its head; mining a 5MB single-line JSON blob token-by-token
/// would be masochism.
const MAX_MINE_CHARS: usize = 4096;

/// Maximum clusters scanned per leaf node (perf guard for pathological logs).
const MAX_LEAF_SCAN: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCluster {
    pub id: u32,
    pub template: Vec<String>,
    pub size: usize,
    pub example_line: usize,
}

impl LogCluster {
    pub fn pattern(&self) -> String {
        self.template.join(" ")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Node {
    children: FxHashMap<String, Node>,
    cluster_ids: Vec<u32>,
}

#[derive(Clone)]
pub struct Drain {
    depth: usize,
    sim_threshold: f64,
    max_children: usize,
    max_clusters: usize,
    root: Node,
    pub clusters: Vec<LogCluster>,
}

/// Exact inverse of one `add_line_with_undo` call. Only the touched cluster
/// and newly created tree path are retained, not a clone of the full miner.
#[derive(Clone)]
pub struct DrainUndo {
    old_cluster_count: usize,
    old_cluster: Option<LogCluster>,
    path: Vec<String>,
    first_created_node: Option<usize>,
}

impl DrainUndo {
    fn new(old_cluster_count: usize) -> Self {
        Self {
            old_cluster_count,
            old_cluster: None,
            path: Vec::new(),
            first_created_node: None,
        }
    }
}

impl Default for Drain {
    fn default() -> Self {
        Self::new(4, 0.5, 100, 20_000)
    }
}

impl Drain {
    pub fn new(depth: usize, sim_threshold: f64, max_children: usize, max_clusters: usize) -> Self {
        let depth = depth.max(3);
        let mut d = Drain {
            depth,
            sim_threshold,
            max_children,
            max_clusters,
            root: Node::default(),
            clusters: Vec::with_capacity(256),
        };
        // Cluster 0 is reserved for blank/whitespace-only lines.
        d.clusters.push(LogCluster {
            id: 0,
            template: vec!["<EMPTY>".to_string()],
            size: 0,
            example_line: 0,
        });
        d
    }

    /// Restore a previously persisted set of clusters.  IDs must be dense and
    /// start at the reserved empty-line cluster (0), so restored IDs remain
    /// stable when new lines are mined.
    pub fn from_clusters(
        depth: usize,
        sim_threshold: f64,
        max_children: usize,
        max_clusters: usize,
        clusters: Vec<LogCluster>,
    ) -> Result<Self, String> {
        if clusters.is_empty() {
            return Err("template store must include the reserved template ID 0".to_string());
        }
        if clusters.len() > max_clusters {
            return Err(format!(
                "template store contains {} templates; maximum is {max_clusters}",
                clusters.len()
            ));
        }
        for (expected, cluster) in clusters.iter().enumerate() {
            if cluster.id != expected as u32 {
                return Err(format!(
                    "template store IDs must be dense; expected {expected}, found {}",
                    cluster.id
                ));
            }
            if cluster.template.is_empty() {
                return Err(format!("template {} has no tokens", cluster.id));
            }
        }
        if clusters[0].template != ["<EMPTY>".to_string()] {
            return Err("template ID 0 must be the reserved <EMPTY> template".to_string());
        }

        let mut drain = Self {
            depth: depth.max(3),
            sim_threshold,
            max_children,
            max_clusters,
            root: Node::default(),
            clusters,
        };
        for id in 1..drain.clusters.len() {
            drain.insert_restored_cluster(id as u32);
        }
        Ok(drain)
    }

    fn insert_restored_cluster(&mut self, id: u32) {
        let template = self.clusters[id as usize].template.clone();
        let mut len_buf = [0u8; 20];
        let len_key = render_usize(template.len(), &mut len_buf);
        let (mut node, _) = Self::child(&mut self.root, len_key, self.max_children);
        for i in 1..self.depth - 1 {
            let key = template.get(i - 1).map_or("*", String::as_str);
            let (next, _) = Self::child(node, key, self.max_children);
            node = next;
        }
        node.cluster_ids.push(id);
    }

    /// Feed one log line; returns the template cluster ID assigned to it.
    pub fn add_line(&mut self, line: &str, line_idx: usize) -> u32 {
        self.add_line_impl(line, line_idx, None)
    }

    /// Add a provisional final line and return the exact inverse operation.
    /// Replaying an amended physical line then produces the same tree and
    /// templates as parsing the final bytes once.
    pub fn add_line_with_undo(&mut self, line: &str, line_idx: usize) -> (u32, DrainUndo) {
        let mut undo = DrainUndo::new(self.clusters.len());
        let id = self.add_line_impl(line, line_idx, Some(&mut undo));
        (id, undo)
    }

    pub fn undo_line(&mut self, undo: DrainUndo) {
        if self.clusters.len() > undo.old_cluster_count {
            let added_id = undo.old_cluster_count as u32;
            if undo.first_created_node.is_none() {
                let mut node = &mut self.root;
                for key in &undo.path {
                    node = node.children.get_mut(key).expect("journal path exists");
                }
                assert_eq!(node.cluster_ids.pop(), Some(added_id));
            }
            self.clusters.truncate(undo.old_cluster_count);
        }
        if let Some(old_cluster) = undo.old_cluster {
            let id = old_cluster.id as usize;
            self.clusters[id] = old_cluster;
        }
        if let Some(first_created) = undo.first_created_node {
            let mut parent = &mut self.root;
            for key in &undo.path[..first_created] {
                parent = parent.children.get_mut(key).expect("journal parent exists");
            }
            parent.children.remove(&undo.path[first_created]);
        }
    }

    fn add_line_impl(
        &mut self,
        line: &str,
        line_idx: usize,
        mut undo: Option<&mut DrainUndo>,
    ) -> u32 {
        let head = if line.len() > MAX_MINE_CHARS {
            line.get(..MAX_MINE_CHARS).unwrap_or(line)
        } else {
            line
        };
        let tokens: Vec<&str> = head.split_whitespace().collect();
        if tokens.is_empty() {
            if let Some(undo) = undo.as_deref_mut() {
                undo.old_cluster = Some(self.clusters[0].clone());
            }
            self.clusters[0].size += 1;
            return 0;
        }

        // Walk the tree: length node, then first (depth-2) token nodes.
        // The length key is rendered into a stack buffer — no per-line alloc.
        let mut len_buf = [0u8; 20];
        let len_key = render_usize(tokens.len(), &mut len_buf);
        if let Some(undo) = undo.as_deref_mut() {
            let created = !self.root.children.contains_key(len_key)
                && !(len_key != "*" && self.root.children.len() >= self.max_children);
            let actual = if len_key != "*"
                && !self.root.children.contains_key(len_key)
                && self.root.children.len() >= self.max_children
            {
                "*"
            } else {
                len_key
            };
            if created || !self.root.children.contains_key(actual) {
                undo.first_created_node = Some(0);
            }
            undo.path.push(actual.to_string());
        }
        let (mut node, _) = Self::child(&mut self.root, len_key, self.max_children);
        let mut overflowed = false;
        for i in 1..self.depth - 1 {
            let key: &str = if i - 1 < tokens.len() {
                tokens[i - 1]
            } else {
                "*"
            };
            if let Some(undo) = undo.as_deref_mut() {
                let actual = if key != "*"
                    && !node.children.contains_key(key)
                    && node.children.len() >= self.max_children
                {
                    "*"
                } else {
                    key
                };
                if undo.first_created_node.is_none() && !node.children.contains_key(actual) {
                    undo.first_created_node = Some(undo.path.len());
                }
                undo.path.push(actual.to_string());
            }
            let (next, ovf) = Self::child(node, key, self.max_children);
            node = next;
            overflowed |= ovf;
        }

        // Find the most similar cluster at this leaf. Templates that are
        // already mostly wildcards stop attracting new lines unless the
        // match is strong — this keeps clusters from collapsing into
        // "<*> <*> <*> ..." mush over time.
        let scan_limit = node.cluster_ids.len().min(MAX_LEAF_SCAN);
        let mut best: Option<(u32, f64)> = None;
        for &cid in node.cluster_ids[..scan_limit].iter() {
            let template = &self.clusters[cid as usize].template;
            let wild = template.iter().filter(|t| t.as_str() == WILDCARD).count();
            let degraded = wild * 10 > template.len() * 7; // >70% wildcards
            let sim = seq_sim(&tokens, template);
            if degraded && sim < 0.8 {
                continue;
            }
            if best.map_or(true, |(_, s)| sim > s) {
                best = Some((cid, sim));
            }
        }

        // Overflow leaves are catch-alls: any similarity merges, so same-shape
        // overflow lines cluster together. The degraded-template guard above
        // keeps these clusters from absorbing ever-more-dissimilar lines.
        let threshold = if overflowed { 0.0 } else { self.sim_threshold };

        match best {
            Some((cid, sim)) if sim >= threshold => {
                let cluster = &mut self.clusters[cid as usize];
                if let Some(undo) = undo.as_deref_mut() {
                    undo.old_cluster = Some(cluster.clone());
                }
                merge(&mut cluster.template, &tokens);
                cluster.size += 1;
                cid
            }
            _ => {
                let id = self.clusters.len() as u32;
                if (id as usize) < self.max_clusters {
                    self.clusters.push(LogCluster {
                        id,
                        template: tokens.iter().map(|s| s.to_string()).collect(),
                        size: 1,
                        example_line: line_idx,
                    });
                    node.cluster_ids.push(id);
                    id
                } else if let Some((cid, _)) = best {
                    // Cluster budget exhausted: force-merge into the closest one.
                    let cluster = &mut self.clusters[cid as usize];
                    if let Some(undo) = undo.as_deref_mut() {
                        undo.old_cluster = Some(cluster.clone());
                    }
                    merge(&mut cluster.template, &tokens);
                    cluster.size += 1;
                    cid
                } else {
                    if let Some(undo) = undo.as_deref_mut() {
                        undo.old_cluster = Some(self.clusters[0].clone());
                    }
                    self.clusters[0].size += 1;
                    0
                }
            }
        }
    }

    fn child<'a>(node: &'a mut Node, key: &str, max_children: usize) -> (&'a mut Node, bool) {
        // Fast path: key already exists — zero allocation.
        if node.children.contains_key(key) {
            return (node.children.get_mut(key).unwrap(), false);
        }
        // Too many distinct branches: bucket the overflow under "*".
        // The bool is true only for this capacity-overflow case — not the
        // fast path, and not the short-line "*" key.
        if key != "*" && node.children.len() >= max_children {
            return (node.children.entry("*".to_string()).or_default(), true);
        }
        // Insert new entry — only here do we allocate an owned String.
        (node.children.entry(key.to_string()).or_default(), false)
    }
}

/// Render a usize as ASCII into `buf`, returning the &str slice.
fn render_usize(mut v: usize, buf: &mut [u8; 20]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    std::str::from_utf8(&buf[i..]).unwrap()
}

/// Token similarity: fraction of positions that match. An exact token match
/// scores full credit; a `<*>` wildcard scores half — wildcards are an
/// admission of ignorance, not evidence of similarity. This stops degraded
/// templates from attracting ever-more-dissimilar lines.
fn seq_sim(tokens: &[&str], template: &[String]) -> f64 {
    let n = tokens.len().max(template.len());
    if n == 0 {
        return 1.0;
    }
    // Tenths, to keep integer math: exact = 10, wildcard = 5.
    let mut score = 0usize;
    for i in 0..tokens.len().min(template.len()) {
        if tokens[i] == template[i] {
            score += 10;
        } else if template[i] == WILDCARD {
            score += 5;
        }
    }
    score as f64 / (10 * n) as f64
}

/// Merge a line into a template: differing positions become wildcards.
fn merge(template: &mut [String], tokens: &[&str]) {
    for i in 0..template.len().min(tokens.len()) {
        if template[i] != tokens[i] && template[i] != WILDCARD {
            template[i] = WILDCARD.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mines_common_template() {
        let mut d = Drain::default();
        let a = d.add_line("2026-07-19 INFO User alice logged in from 10.0.0.1", 0);
        let b = d.add_line("2026-07-19 INFO User bob logged in from 10.0.0.2", 1);
        let c = d.add_line("2026-07-19 ERROR Disk /dev/sda full", 2);
        assert_eq!(a, b, "same shape should share a template");
        assert_ne!(a, c, "different shape should not");
        let pattern = d.clusters[a as usize].pattern();
        assert!(
            pattern.contains("User"),
            "pattern keeps constant tokens: {pattern}"
        );
        assert!(
            pattern.contains("<*>"),
            "pattern abstracts variables: {pattern}"
        );
        assert_eq!(d.clusters[a as usize].size, 2);
    }

    #[test]
    fn blank_lines_get_cluster_zero() {
        let mut d = Drain::default();
        assert_eq!(d.add_line("", 0), 0);
        assert_eq!(d.add_line("   ", 1), 0);
        assert_eq!(d.clusters[0].size, 2);
    }

    #[test]
    fn different_lengths_diverge() {
        let mut d = Drain::default();
        let a = d.add_line("INFO short line", 0);
        let b = d.add_line("INFO a much longer line with more tokens", 1);
        assert_ne!(a, b);
    }

    #[test]
    fn handles_giant_line_without_dying() {
        let mut d = Drain::default();
        let giant = format!("START {}", "x ".repeat(200_000));
        let id = d.add_line(&giant, 0);
        assert!(id > 0);
    }

    #[test]
    fn wildcard_propagation() {
        let mut d = Drain::default();
        let a = d.add_line("INFO request from alice to bob", 0);
        let b = d.add_line("INFO request from charlie to dave", 1);
        assert_eq!(a, b, "same shape should share a template");
        let pattern = d.clusters[a as usize].pattern();
        assert_eq!(
            pattern, "INFO request from <*> to <*>",
            "pattern: {pattern}"
        );
        assert_eq!(d.clusters[a as usize].size, 2);
    }

    #[test]
    fn all_tokens_differ_wildcard() {
        // depth=3 so all lines with same first token route to the same leaf.
        let mut d = Drain::new(3, 0.0, 100, 100);
        let a = d.add_line("a b c", 0);
        let b = d.add_line("a d e", 1);
        assert_eq!(
            a, b,
            "same-first-token lines should share a template at depth=3"
        );
        let pattern = d.clusters[a as usize].pattern();
        assert_eq!(
            pattern, "a <*> <*>",
            "differing positions should be wildcarded: {pattern}"
        );
    }

    #[test]
    fn multiple_lines_same_template() {
        // depth=3 so tokens[0] ("EVENT") determines the leaf, not tokens[1].
        let mut d = Drain::new(3, 0.4, 100, 100);
        let mut ids = Vec::new();
        for i in 0..100 {
            ids.push(d.add_line(&format!("EVENT type={} status=ok", i % 5), i));
        }
        // All lines have the same shape: "EVENT type=<*> status=ok"
        let first = ids[0];
        for &id in &ids {
            assert_eq!(id, first, "all same-shape lines should share template");
        }
        assert_eq!(d.clusters[first as usize].size, 100);
    }

    #[test]
    fn restores_clusters_with_stable_ids_and_continues_mining() {
        let mut first = Drain::default();
        let original = first.add_line("INFO worker alice connected", 0);
        assert_eq!(first.add_line("INFO worker bob connected", 1), original);

        let mut restored = Drain::from_clusters(4, 0.5, 100, 20_000, first.clusters).unwrap();
        assert_eq!(
            restored.add_line("INFO worker carol connected", 0),
            original,
            "a restored template keeps its original ID"
        );
        assert_eq!(restored.clusters[original as usize].size, 3);
    }

    #[test]
    fn rejects_non_dense_restored_ids() {
        let clusters = vec![
            LogCluster {
                id: 0,
                template: vec!["<EMPTY>".to_string()],
                size: 0,
                example_line: 0,
            },
            LogCluster {
                id: 2,
                template: vec!["INFO".to_string()],
                size: 1,
                example_line: 0,
            },
        ];
        assert!(Drain::from_clusters(4, 0.5, 100, 20_000, clusters).is_err());
    }

    #[test]
    fn cluster_budget_exhaustion_force_merges() {
        // max_clusters=3 means only 2 real clusters (cluster 0 is reserved for blanks).
        // depth=3 so the first token determines the leaf.
        // All lines have 2 tokens and start with "INFO" → same leaf.
        let mut d = Drain::new(3, 0.4, 100, 3);
        // First distinct line → cluster 1
        let id1 = d.add_line("INFO alpha", 0);
        // Second distinct line → cluster 2
        let id2 = d.add_line("INFO beta", 1);
        // Third line → budget exhausted, force-merge into closest (id1 or id2)
        let id3 = d.add_line("INFO gamma", 2);
        assert!(
            id3 == id1 || id3 == id2,
            "force-merged line should match an existing cluster"
        );
        let cluster = d.clusters.iter().find(|c| c.id == id3).unwrap();
        assert!(cluster.size >= 2, "force-merged cluster should have grown");
    }

    #[test]
    fn below_threshold_creates_new_cluster() {
        let mut d = Drain::new(4, 1.0, 100, 100);
        let a = d.add_line("INFO foo", 0);
        let b = d.add_line("INFO bar", 1);
        assert_ne!(
            a, b,
            "different tokens below threshold 1.0 should get different clusters"
        );
        assert_eq!(d.clusters[a as usize].size, 1);
        assert_eq!(d.clusters[b as usize].size, 1);
    }

    #[test]
    fn tree_overflow_buckets_under_star() {
        // depth=3 so only tokens[0] determines the leaf (no tokens[1] routing).
        // max_children=2 at the length node: only 2 distinct first-token branches.
        // All lines have 3 tokens → same length node.
        let mut d = Drain::new(3, 0.4, 2, 100);
        let a = d.add_line("alpha one two", 0);
        let b = d.add_line("beta three four", 1);
        // "gamma" overflows to "*" branch at the length node.
        let c = d.add_line("gamma five six", 2);
        assert!(c > 0, "overflow line should still get a template ID");
        // "delta" also overflows to "*" branch — same leaf, same shape → same cluster.
        let d2 = d.add_line("delta seven eight", 3);
        assert_eq!(
            c, d2,
            "overflow lines with same shape should cluster together"
        );
        // "alpha" and "beta" are in their own clusters (different from overflow).
        assert_ne!(a, c, "different branches should have different clusters");
        assert_ne!(b, c, "different branches should have different clusters");
    }

    #[test]
    fn pattern_output_joins_with_spaces() {
        let mut d = Drain::default();
        let id = d.add_line("INFO User alice logged in", 0);
        let pattern = d.clusters[id as usize].pattern();
        // Should contain spaces between tokens, no leading/trailing spaces
        assert!(!pattern.starts_with(' '));
        assert!(!pattern.ends_with(' '));
        assert!(
            pattern.contains(' '),
            "pattern should have spaces: '{pattern}'"
        );
    }

    #[test]
    fn custom_depth_works() {
        // depth=5 means the tree uses: token count + first 3 tokens.
        let mut d = Drain::new(5, 0.4, 100, 100);
        let a = d.add_line("A B C D E", 0);
        let b = d.add_line("A B C X Y", 1); // first 3 tokens match → same leaf
        let c = d.add_line("A B X Y Z", 2); // only first 2 match → different leaf
        assert_eq!(a, b, "first 3 tokens match → same cluster");
        assert_ne!(a, c, "only first 2 tokens match → different cluster");
    }

    #[test]
    fn provisional_line_rollback_restores_exact_tree_and_cluster_state() {
        let mut base = Drain::new(3, 0.4, 2, 4);
        base.add_line("INFO alpha one", 0);
        base.add_line("WARN beta two", 1);
        for (partial, final_line) in [
            ("", "INFO final three"),
            ("INFO alpha", "INFO alpha one"),
            ("ERROR third path", "DEBUG final path"),
            ("WARN beta changed", "WARN beta two"),
        ] {
            let mut staged = base.clone();
            let (_, undo) = staged.add_line_with_undo(partial, 2);
            staged.undo_line(undo);
            assert_eq!(staged.clusters, base.clusters, "partial={partial:?}");
            assert_eq!(staged.root, base.root, "partial={partial:?}");

            let mut direct = base.clone();
            let expected = direct.add_line(final_line, 2);
            let actual = staged.add_line(final_line, 2);
            assert_eq!(actual, expected);
            assert_eq!(staged.clusters, direct.clusters);
            assert_eq!(staged.root, direct.root);
        }
    }
}
