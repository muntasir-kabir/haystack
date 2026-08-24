//! Performance proof: parse + filter-scan a log file, report timings.
//!
//!   cargo run --release --example bench -- <logfile> [filters...]
//!
//! Or generate a synthetic ~64MB log and bench that:
//!
//!   cargo run --release --example bench

use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use logotomy::core::document::LogDocument;
use logotomy::core::search::scan_document;
use logotomy::core::timeline::{Timeline, DEFAULT_BUCKETS};

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, generated) = match args.next() {
        Some(p) => (std::path::PathBuf::from(p), false),
        None => {
            let p = std::env::temp_dir().join("logotomy_bench.log");
            generate(&p, 64 * 1024 * 1024);
            (p, true)
        }
    };
    let filters: Vec<String> = args.collect();
    let filters = if filters.is_empty() {
        vec!["ERROR".into(), "timeout".into(), "user_id=42".into()]
    } else {
        filters
    };

    let t0 = Instant::now();
    let doc = LogDocument::open(&path).expect("open failed");
    let load_t = t0.elapsed();

    let t1 = Instant::now();
    let matches = scan_document(&doc, &filters, &AtomicBool::new(false));
    let scan_t = t1.elapsed();

    let t2 = Instant::now();
    let _tl = Timeline::build(&doc, &matches, DEFAULT_BUCKETS);
    let tl_t = t2.elapsed();

    let mb = doc.file_size as f64 / (1024.0 * 1024.0);

    // Template quality metrics: cluster count, wildcard degradation, top patterns.
    let degraded = doc
        .templates
        .iter()
        .filter(|t| {
            let toks: Vec<&str> = t.pattern.split_whitespace().collect();
            !toks.is_empty() && toks.iter().filter(|x| **x == "<*>").count() * 10 > toks.len() * 7
        })
        .count();
    let mut top: Vec<&logotomy::core::document::TemplateInfo> = doc.templates.iter().collect();
    top.sort_by_key(|t| std::cmp::Reverse(t.count));

    println!("┌─ logotomy bench ─────────────────────────────");
    println!("│ file          : {}", doc.path.display());
    println!("│ size          : {:.1} MB", mb);
    println!("│ lines         : {}", doc.total_lines());
    println!(
        "│ templates     : {} ({} >70% wildcards)",
        doc.templates.len(),
        degraded
    );
    for t in top.iter().take(10) {
        println!(
            "│   #{:<4} x{:<8} {}",
            t.id,
            t.count,
            t.pattern.chars().take(80).collect::<String>()
        );
    }
    println!(
        "│ time range    : {:?}",
        doc.time_range.map(|(a, b)| (
            logotomy::core::time::format_ms(a),
            logotomy::core::time::format_ms(b)
        ))
    );
    println!("│");
    println!(
        "│ load (index+mine+ts): {:>8.1?}  ({:.0} MB/s)",
        load_t,
        mb / load_t.as_secs_f64()
    );
    println!("│ scan {:?}: {:>8.1?}", filters, scan_t);
    for (k, m) in filters.iter().zip(matches.iter()) {
        println!("│   {k:<12} hits = {}", m.len());
    }
    println!("│ timeline build        : {:>8.1?}", tl_t);
    println!("└───────────────────────────────────────────────");

    if generated {
        std::fs::remove_file(&path).ok();
    }
}

/// Write a synthetic log of roughly `target_bytes` with realistic variety.
fn generate(path: &std::path::Path, target_bytes: u64) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let levels = ["INFO", "DEBUG", "WARN", "ERROR"];
    let events = [
        "request completed path=/api/users status=200",
        "db query took {n}ms sql=SELECT * FROM sessions",
        "cache miss for key user:{n}",
        "retry attempt {n} for job sync-photos",
        "connection timeout to backend-{n}:8443 after 3000ms",
        "ERROR unhandled exception in worker-{n}: NullPointerException",
        "payment authorized order_id=ORD-{n} amount=99.99",
        "user login user_id=42 session=sess-{n}",
    ];
    let mut written = 0u64;
    let mut i = 0u64;
    let base_ms = 1_752_000_000_000i64; // 2025-07-13T00:00:00Z
    while written < target_bytes {
        let ts = base_ms + (i * 37) as i64; // ~27 lines/sec
        let level = levels[(i % 97 / 24) as usize % 4];
        let event =
            events[(i % events.len() as u64) as usize].replace("{n}", &(i % 7919).to_string());
        let line = format!(
            "2025-07-13T{:02}:{:02}:{:02}.{:03}Z {} worker-{} {}\n",
            (ts / 3_600_000) % 24,
            (ts / 60_000) % 60,
            (ts / 1_000) % 60,
            ts % 1000,
            level,
            i % 8,
            event
        );
        written += line.len() as u64;
        f.write_all(line.as_bytes()).unwrap();
        i += 1;
    }
    println!(
        "generated {:.1} MB synthetic log: {}",
        written as f64 / 1e6,
        path.display()
    );
}
