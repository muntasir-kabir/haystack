//! Production-path benchmark for an explicitly selected record layout.
//!
//! Usage: cargo run --release --no-default-features --example bench_record -- \
//!     FILE default|detailed [FILTER ...]
//! Generate matching inputs with `gen_record_logs --layout default|detailed`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use haystack::core::document::{LogDocument, ParsingConfig};
use haystack::core::record::{CompiledProfile, RecordProfile, TimestampSelection};
use haystack::core::search::scan_document;
use haystack::core::time::{Iso, TimeFormatKind};
use haystack::core::timeline::{Timeline, DEFAULT_BUCKETS};

fn profile_for(layout: &str) -> Result<RecordProfile, String> {
    let template = match layout {
        "default" => "{time} {log}",
        "detailed" => "{time} - [{log_level}] - {thread_id} {file}:{line} {log}",
        _ => {
            return Err(format!(
                "unknown layout {layout:?}; expected default or detailed"
            ))
        }
    };
    let mut profile = RecordProfile::text(format!("bench:{layout}"), layout, template);
    profile.timestamp = TimestampSelection::BuiltIn("ISO-8601".into());
    Ok(profile)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: bench_record FILE default|detailed [FILTER ...]");
    let layout = args.next().expect("missing layout: default or detailed");
    let filters: Vec<String> = args.collect();
    let filters = if filters.is_empty() {
        vec!["ERROR".into(), "request".into(), "payload_date".into()]
    } else {
        filters
    };

    let compile_started = Instant::now();
    let profile = CompiledProfile::compile(profile_for(&layout).expect("invalid layout"))
        .expect("profile compilation failed");
    let compile_time = compile_started.elapsed();

    let load_started = Instant::now();
    let doc = LogDocument::open_with_record_profile(
        Path::new(&path),
        ParsingConfig::default(),
        &[],
        profile,
        Some(TimeFormatKind::BuiltIn(&Iso)),
    )
    .expect("open failed");
    let load_time = load_started.elapsed();

    let scan_started = Instant::now();
    let matches = scan_document(&doc, &filters, &AtomicBool::new(false));
    let scan_time = scan_started.elapsed();

    let timeline_started = Instant::now();
    let timeline = Timeline::build(&doc, &matches, DEFAULT_BUCKETS);
    let timeline_time = timeline_started.elapsed();
    let resolve_started = Instant::now();
    let end = doc.total_lines().saturating_sub(1) as i64;
    let overview = timeline.resolve_density_bins(&doc, 0, end, 1_000);
    let lanes: Vec<_> = (0..matches.len())
        .map(|lane| timeline.resolve_filter_bins(&doc, lane, 0, end, 125))
        .collect();
    let resolve_time = resolve_started.elapsed();
    assert_eq!(
        overview.iter().map(|&count| count as u64).sum::<u64>(),
        doc.record_count() as u64
    );
    for (expected, bins) in matches.iter().zip(&lanes) {
        assert_eq!(
            bins.iter().map(|bin| bin.count as usize).sum::<usize>(),
            expected.len()
        );
    }

    println!("file={path}");
    println!("layout={layout}");
    println!("bytes={}", doc.file_size);
    println!("physical_lines={}", doc.total_lines());
    println!("record_starts={}", doc.record_count());
    println!("detected_profile={:?}", doc.detection.profile_id);
    println!("index_payload_bytes={}", doc.index_payload_bytes());
    for (filter, hits) in filters.iter().zip(&matches) {
        println!("filter_hits[{filter}]={}", hits.len());
    }
    println!("compile_ms={:.3}", compile_time.as_secs_f64() * 1_000.0);
    println!("load_ms={:.3}", load_time.as_secs_f64() * 1_000.0);
    println!("scan_ms={:.3}", scan_time.as_secs_f64() * 1_000.0);
    println!("timeline_ms={:.3}", timeline_time.as_secs_f64() * 1_000.0);
    println!("resolve_ms={:.3}", resolve_time.as_secs_f64() * 1_000.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_layouts_compile_to_the_intended_header_shapes() {
        for layout in ["default", "detailed"] {
            CompiledProfile::compile(profile_for(layout).unwrap()).unwrap();
        }
        assert!(profile_for("unknown").is_err());
    }
}
