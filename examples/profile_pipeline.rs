//! Opt-in timings from the actual production document-loading pipeline.
//!
//!   cargo run --release --example profile_pipeline -- [logfile]
//!
//! With no argument, generation happens before the timed load and produces a
//! synthetic ~64 MiB input. Peak RSS is intentionally left to the platform's
//! process timing utility; retained index payload is reported here.

use std::io::Write;

use haystack::core::document::{LoadProfile, LogDocument, ParsingConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, generated) = match args.next() {
        Some(path) => (std::path::PathBuf::from(path), false),
        None => {
            let path = std::env::temp_dir().join("haystack_profile.log");
            generate(&path, 64 * 1024 * 1024);
            (path, true)
        }
    };

    let (document, profile) = LogDocument::open_profiled(&path, ParsingConfig::default(), &[])
        .unwrap_or_else(|error| panic!("cannot profile {}: {error}", path.display()));
    print_profile(&document, &profile);

    if generated {
        std::fs::remove_file(&path).ok();
    }
}

fn print_profile(document: &LogDocument, profile: &LoadProfile) {
    let mib = profile.bytes as f64 / (1024.0 * 1024.0);
    println!("path={}", document.path.display());
    println!("bytes={}", profile.bytes);
    println!("physical_lines={}", profile.physical_lines);
    println!("record_marks={}", document.record_count());
    println!("explicit_timestamps={}", profile.explicit_timestamps);
    println!("format={}", document.format_name());
    println!(
        "date_format={}",
        document
            .time_format_name()
            .as_deref()
            .unwrap_or("field-based/none")
    );
    println!("templates={}", document.templates.len());
    println!("index_payload_bytes={}", document.index_payload_bytes());
    println!("input_mib={mib:.2}");
    println!("index_build={:?}", profile.index_build);
    println!("profile_discovery={:?}", profile.profile_discovery);
    println!("header_learning={:?}", profile.header_learning);
    println!("analysis_wall={:?}", profile.analysis_wall);
    println!("  line_decode={:?}", profile.line_decode);
    println!("  timestamp_extract={:?}", profile.timestamp_extract);
    println!(
        "  normalization_and_masking={:?}",
        profile.normalization_and_masking
    );
    println!("  drain_mining={:?}", profile.drain_mining);
    println!("total_wall={:?}", profile.total_wall);
}

fn generate(path: &std::path::Path, target_bytes: u64) {
    let mut output = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
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
    let mut index = 0u64;
    let base_ms = 1_752_000_000_000i64;
    while written < target_bytes {
        let timestamp = base_ms + (index * 37) as i64;
        let level = levels[(index % 97 / 24) as usize % 4];
        let event = events[(index % events.len() as u64) as usize]
            .replace("{n}", &(index % 7919).to_string());
        let line = format!(
            "2025-07-13T{:02}:{:02}:{:02}.{:03}Z {} worker-{} {}\n",
            (timestamp / 3_600_000) % 24,
            (timestamp / 60_000) % 60,
            (timestamp / 1_000) % 60,
            timestamp % 1000,
            level,
            index % 8,
            event
        );
        written += line.len() as u64;
        output.write_all(line.as_bytes()).unwrap();
        index += 1;
    }
}
