//! Micro-profile of the analysis pipeline phases on a log file.
//! Times each stage cumulatively: line slice → utf8 → ts extract → ts strip → mask → drain.
//!
//!   cargo run --release --example profile_pipeline -- [logfile]
//!
//! With no argument, generates a synthetic ~64MB log in the temp dir and
//! profiles that. Pass a path (e.g. an iOS log from `gen_ios_logs`) to
//! profile a real file instead.

use std::borrow::Cow;
use std::io::Write;
use std::time::Instant;

use logotomy::core::drain::Drain;
use logotomy::core::masking::{LogMasker, MaskCache};
use logotomy::core::time::TimeDetector;

fn main() {
    let mut args = std::env::args().skip(1);
    let (path, generated) = match args.next() {
        Some(p) => (std::path::PathBuf::from(p), false),
        None => {
            let p = std::env::temp_dir().join("logotomy_profile.log");
            generate(&p, 64 * 1024 * 1024);
            (p, true)
        }
    };
    let data = std::fs::read(&path).unwrap();
    let mut offsets: Vec<usize> = vec![0];
    for (i, &b) in data.iter().enumerate() {
        if b == b'\n' && i + 1 < data.len() {
            offsets.push(i + 1);
        }
    }
    let n = offsets.len();
    let mb = data.len() as f64 / (1024.0 * 1024.0);
    println!("{} lines, {:.1} MB", n, mb);

    // Phase 1: line slicing (zero-copy)
    let t = Instant::now();
    let mut total_len = 0usize;
    for w in offsets.windows(2) {
        total_len += w[1] - w[0];
    }
    println!(
        "slice           : {:>8.1?}  ({:.0} MB/s) [{}]",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64(),
        total_len
    );

    // Phase 2: + utf8 lossy conversion
    let t = Instant::now();
    let mut lines: Vec<Cow<str>> = Vec::with_capacity(n);
    for w in offsets.windows(2) {
        let mut end = w[1];
        while end > w[0] && (data[end - 1] == b'\n' || data[end - 1] == b'\r') {
            end -= 1;
        }
        lines.push(String::from_utf8_lossy(&data[w[0]..end]));
    }
    println!(
        "+utf8 lossy     : {:>8.1?}  ({:.0} MB/s)",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64()
    );

    // Phase 3: + timestamp extraction
    let extractor = TimeDetector::detect(lines.iter().take(1000).map(|l| l.clone()));
    let t = Instant::now();
    let mut spans = Vec::with_capacity(n);
    for line in &lines {
        spans.push(extractor.as_ref().and_then(|e| e.extract(line)));
    }
    println!(
        "+ts extract     : {:>8.1?}  ({:.0} MB/s) [{} ts]",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64(),
        spans.iter().flatten().count()
    );

    // Phase 4: + ts strip (owned copy)
    let t = Instant::now();
    let mut stripped: Vec<Cow<str>> = Vec::with_capacity(n);
    for (line, sp) in lines.iter().zip(spans.iter()) {
        match sp {
            Some((_, range)) => {
                let mut owned = line.clone().into_owned();
                owned.replace_range(range.clone(), "");
                stripped.push(Cow::Owned(owned));
            }
            None => stripped.push(line.clone()),
        }
    }
    println!(
        "+ts strip       : {:>8.1?}  ({:.0} MB/s)",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64()
    );

    // Phase 5: + masking
    let masker = LogMasker::default();
    let t = Instant::now();
    let mut masked: Vec<Cow<str>> = Vec::with_capacity(n);
    let mut cache = MaskCache::default();
    for line in &stripped {
        masked.push(masker.mask_with_header(line, &[], &mut cache));
    }
    println!(
        "+mask           : {:>8.1?}  ({:.0} MB/s)",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64()
    );

    // Phase 6: + drain
    let t = Instant::now();
    let mut drain = Drain::default();
    let mut ids = Vec::with_capacity(n);
    for (i, line) in masked.iter().enumerate() {
        ids.push(drain.add_line(line, i));
    }
    println!(
        "+drain          : {:>8.1?}  ({:.0} MB/s) [{} clusters]",
        t.elapsed(),
        mb / t.elapsed().as_secs_f64(),
        drain.clusters.len()
    );

    if generated {
        std::fs::remove_file(&path).ok();
    }
}

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
    let base_ms = 1_752_000_000_000i64;
    while written < target_bytes {
        let ts = base_ms + (i * 37) as i64;
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
}
