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
//!   --payload-bytes N      Minimum payload bytes per line (default: 48)
//!   --seed N               Deterministic seed (default: 20260911)

use std::io::{BufWriter, Write};
use std::path::PathBuf;

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

    for physical_line in 0..config.lines {
        let stride = config.continuations + 1;
        let is_header = physical_line % stride == 0;
        let record = physical_line / stride;
        let text = if is_header {
            records += 1;
            header_line(&config, record, &mut rng)
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

fn header_line(config: &Config, record: usize, rng: &mut Lcg) -> String {
    let mut millis = 1_789_120_000_000i64 + record as i64 * 37;
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
    let seconds = millis.div_euclid(1_000);
    let sub_ms = millis.rem_euclid(1_000);
    let second_of_day = seconds.rem_euclid(86_400);
    let hour = second_of_day / 3_600;
    let minute = second_of_day / 60 % 60;
    let second = second_of_day % 60;
    match family {
        DateFamily::Iso => format!("2026-09-11T{hour:02}:{minute:02}:{second:02}.{sub_ms:03}Z"),
        DateFamily::Slash => format!("2026/09/11 {hour:02}:{minute:02}:{second:02}.{sub_ms:03}"),
        DateFamily::Epoch => millis.to_string(),
    }
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
        assert_eq!(
            header_line(&config, 0, &mut left),
            header_line(&config, 0, &mut right)
        );
        assert!(continuation_line(&config, 1, &mut left).starts_with("    "));
    }

    #[test]
    fn detailed_layout_keeps_windows_path_and_source_line() {
        let config = Config {
            layout: Layout::Detailed,
            ..Config::default()
        };
        let line = header_line(&config, 0, &mut Lcg(config.seed));
        assert!(line.contains("C:\\src\\Handler.rs:1"));
        assert!(line.contains(" - [TRACE] - worker-0 "));
    }
}
