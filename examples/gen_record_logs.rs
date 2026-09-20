//! Generate deterministic record-oriented logs for parser and timeline benchmarks.
//!
//! Usage:
//!   cargo run --release --example gen_record_logs -- [options]
//!
//! Options:
//!   --output PATH          Output path (default: record-benchmark.log)
//!   --lines N              Exact physical-line count (default: 100000)
//!   --continuations N      Continuations after each header (default: 10)
//!   --layout NAME          default | detailed | prefixed | json (default: default)
//!   --date-family NAME     iso | slash | epoch (default: iso)
//!   --reverse-every N      Move the clock backwards every N records (0 disables)
//!   --long-delay-every N   Add a 10m..7d gap every N records (default: 50, 0 disables)
//!   --payload-bytes N      Minimum payload bytes per line (default: 48)
//!   --seed N               Deterministic seed (default: 20260911)

use chrono::{DateTime, Utc};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const MIN_LONG_DELAY_MS: i64 = 10 * 60 * 1_000;
const MAX_LONG_DELAY_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const DEFAULT_LONG_DELAY_EVERY: usize = 50;

#[derive(Clone, Copy)]
enum Layout {
    Default,
    Detailed,
    Prefixed,
    Json,
}

#[derive(Clone, Copy)]
enum DateFamily {
    Iso,
    Slash,
    Epoch,
}

struct Config {
    output: PathBuf,
    lines: usize,
    continuations: usize,
    layout: Layout,
    date_family: DateFamily,
    reverse_every: usize,
    long_delay_every: usize,
    payload_bytes: usize,
    seed: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: PathBuf::from("record-benchmark.log"),
            lines: 100_000,
            continuations: 10,
            layout: Layout::Default,
            date_family: DateFamily::Iso,
            reverse_every: 0,
            long_delay_every: DEFAULT_LONG_DELAY_EVERY,
            payload_bytes: 48,
            seed: 20_260_911,
        }
    }
}

fn main() {
    let config = parse_args().unwrap_or_else(|error| {
        eprintln!("error: {error}");
        std::process::exit(2);
    });
    let file = std::fs::File::create(&config.output)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", config.output.display()));
    let mut output = BufWriter::new(file);
    let mut rng = Lcg(config.seed);
    let mut records = 0usize;
    let mut bytes = 0usize;
    let mut accumulated_delay_ms = 0i64;

    for physical_line in 0..config.lines {
        let stride = config.continuations + 1;
        let is_header = physical_line % stride == 0;
        let record = physical_line / stride;
        let text = if is_header {
            records += 1;
            header_line(&config, record, &mut rng, &mut accumulated_delay_ms)
        } else {
            continuation_line(&config, physical_line, &mut rng)
        };
        bytes += text.len() + 1;
        writeln!(output, "{text}").expect("write generated log");
    }
    output.flush().expect("flush generated log");

    println!("path={}", config.output.display());
    println!("bytes={bytes}");
    println!("physical_lines={}", config.lines);
    println!("records={records}");
    println!("continuations={}", config.lines.saturating_sub(records));
    println!("seed={}", config.seed);
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = std::env::args();
    let _program = args.next();
    while let Some(arg) = args.next() {
        let value = |args: &mut std::env::Args| {
            args.next().ok_or_else(|| format!("{arg} requires a value"))
        };
        match arg.as_str() {
            "--output" => config.output = PathBuf::from(value(&mut args)?),
            "--lines" => config.lines = parse_usize(&arg, value(&mut args)?)?,
            "--continuations" => config.continuations = parse_usize(&arg, value(&mut args)?)?,
            "--reverse-every" => config.reverse_every = parse_usize(&arg, value(&mut args)?)?,
            "--long-delay-every" => {
                config.long_delay_every = parse_usize(&arg, value(&mut args)?)?;
                if config.long_delay_every != 0 && config.long_delay_every < 50 {
                    return Err("--long-delay-every must be 0 or at least 50 (max 20 gaps per 1,000 records)".to_string());
                }
            }
            "--payload-bytes" => config.payload_bytes = parse_usize(&arg, value(&mut args)?)?,
            "--seed" => {
                config.seed = value(&mut args)?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer".to_string())?
            }
            "--layout" => {
                config.layout = match value(&mut args)?.as_str() {
                    "default" => Layout::Default,
                    "detailed" => Layout::Detailed,
                    "prefixed" => Layout::Prefixed,
                    "json" => Layout::Json,
                    other => return Err(format!("unknown layout {other:?}")),
                }
            }
            "--date-family" => {
                config.date_family = match value(&mut args)?.as_str() {
                    "iso" => DateFamily::Iso,
                    "slash" => DateFamily::Slash,
                    "epoch" => DateFamily::Epoch,
                    other => return Err(format!("unknown date family {other:?}")),
                }
            }
            "-h" | "--help" => {
                println!(
                    "{}",
                    include_str!("gen_record_logs.rs")
                        .lines()
                        .take(15)
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    Ok(config)
}

fn parse_usize(option: &str, value: String) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{option} must be an unsigned integer"))
}

fn header_line(
    config: &Config,
    record: usize,
    rng: &mut Lcg,
    accumulated_delay_ms: &mut i64,
) -> String {
    if let Some(delay_ms) = long_delay(config.long_delay_every, record, rng) {
        *accumulated_delay_ms += delay_ms;
    }
    let mut millis = 1_789_120_000_000i64 + record as i64 * 37 + *accumulated_delay_ms;
    if config.reverse_every > 0 && record > 0 && record % config.reverse_every == 0 {
        millis -= 5_000;
    }
    let timestamp = timestamp(config.date_family, millis);
    let level = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"][record % 6];
    let message = pad_payload(
        format!(
            "request={} status={} payload_date=2035-01-01T00:00:00Z",
            rng.next(),
            200 + record % 5
        ),
        config.payload_bytes,
    );
    match config.layout {
        Layout::Default => format!("{timestamp} {message}"),
        Layout::Detailed => format!(
            "{timestamp} - [{level}] - worker-{} C:\\src\\Handler.rs:{} {message}",
            record % 31,
            record % 997 + 1
        ),
        Layout::Prefixed => format!("[worker-{}] {level} {timestamp} {message}", record % 31),
        Layout::Json => {
            format!("{{\"msg\":{message:?},\"time\":{timestamp:?},\"record\":{record}}}")
        }
    }
}

fn continuation_line(config: &Config, physical_line: usize, rng: &mut Lcg) -> String {
    pad_payload(
        format!(
            "    payload row={physical_line} retry_at=2035-01-01T00:00:00Z token={}",
            rng.next()
        ),
        config.payload_bytes,
    )
}

fn pad_payload(mut value: String, minimum: usize) -> String {
    if value.len() < minimum {
        value.push(' ');
        value.extend(std::iter::repeat_n('x', minimum - value.len()));
    }
    value
}

fn timestamp(family: DateFamily, millis: i64) -> String {
    match family {
        DateFamily::Iso => DateTime::<Utc>::from_timestamp_millis(millis)
            .expect("generated timestamp in range")
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string(),
        DateFamily::Slash => DateTime::<Utc>::from_timestamp_millis(millis)
            .expect("generated timestamp in range")
            .format("%Y/%m/%d %H:%M:%S%.3f")
            .to_string(),
        DateFamily::Epoch => millis.to_string(),
    }
}

fn long_delay(every: usize, record: usize, rng: &mut Lcg) -> Option<i64> {
    if every == 0 || record == 0 || record % every != 0 {
        return None;
    }
    let range = (MAX_LONG_DELAY_MS - MIN_LONG_DELAY_MS + 1) as u64;
    Some(MIN_LONG_DELAY_MS + (rng.next() % range) as i64)
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_headers_and_continuations_are_deterministic() {
        let config = Config::default();
        let mut left = Lcg(config.seed);
        let mut right = Lcg(config.seed);
        let mut left_delay = 0;
        let mut right_delay = 0;
        assert_eq!(
            header_line(&config, 0, &mut left, &mut left_delay),
            header_line(&config, 0, &mut right, &mut right_delay)
        );
        assert!(continuation_line(&config, 1, &mut left).starts_with("    "));
    }

    #[test]
    fn detailed_layout_keeps_windows_path_and_source_line() {
        let config = Config {
            layout: Layout::Detailed,
            ..Config::default()
        };
        let mut delay = 0;
        let line = header_line(&config, 0, &mut Lcg(config.seed), &mut delay);
        assert!(line.contains("C:\\src\\Handler.rs:1"));
        assert!(line.contains(" - [TRACE] - worker-0 "));
    }

    #[test]
    fn long_delays_are_bounded_and_sparse() {
        let mut rng = Lcg(Config::default().seed);
        let delays: Vec<_> = (0..1_000)
            .filter_map(|record| long_delay(DEFAULT_LONG_DELAY_EVERY, record, &mut rng))
            .collect();
        assert_eq!(delays.len(), 19);
        assert!(delays
            .iter()
            .all(|delay| (MIN_LONG_DELAY_MS..=MAX_LONG_DELAY_MS).contains(delay)));
    }

    #[test]
    fn long_delay_timestamp_rolls_over_the_calendar() {
        let millis = 1_789_120_000_000i64 + 7 * 24 * 60 * 60 * 1_000;
        assert!(timestamp(DateFamily::Iso, millis).starts_with("2026-09-18T"));
        assert!(timestamp(DateFamily::Slash, millis).starts_with("2026/09/18 "));
    }
}
