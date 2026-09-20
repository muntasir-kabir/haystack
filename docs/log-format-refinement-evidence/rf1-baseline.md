# RF1 baseline evidence

Captured on 2026-09-13 before RF2–RF8 implementation. This is a comparison baseline, not a product performance claim.

## Environment

- Revision: `7713423` plus the RF1 fixture/compatibility changes in this checkpoint.
- Machine: Intel MacBook Air, macOS 15.7.7 (24G720), Darwin 24.6.0 x86_64.
- Build: Rust release profile; benchmark parser built with `--no-default-features`.
- Generator: `examples/gen_record_logs.rs`, seed `20260911`, ISO dates, 10 continuations per header, 48-byte minimum payload.

## Correctness baseline

| Command | Result |
|---|---|
| `cargo test --test record_inline_contract` | 5 passed |
| `cargo test` | Passed: 362 library tests, GUI/main tests, 11 integration test binaries, and doc tests. |
| `cargo test --no-default-features` | Passed: 362 library tests, main tests, 11 integration test binaries, and doc tests. |

The RF1 test adds the human-authored schema-3 acceptance fixture and proves that a schema-2 custom field literally named `ignore` still serializes, compiles, and appears in captures. Schema-3 fixture cases remain intentionally unexecuted until RF5 implements that schema.

## Deterministic parser baseline

Generated inputs were stored under `/private/tmp` and are reproducible with:

```text
target/release/examples/gen_record_logs --output /private/tmp/haystack-rf1-<layout>-<size>.log --lines <100000|1000000> --continuations 10 --layout <default|detailed> --seed 20260911
target/release/examples/bench_record /private/tmp/haystack-rf1-<layout>-<size>.log <default|detailed>
```

`index_payload_bytes` is the document's retained index payload, not process RSS. Values are one run each and therefore establish a comparison point rather than a statistical latency result.

| Layout / lines | Input bytes | Record starts | Index payload bytes | Compile ms | Load ms | Scan ms | Timeline ms | Resolve ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Default / 100K | 8,011,818 | 9,091 | 2,438,300 | 0.212 | 507.994 | 32.116 | 3.559 | 1.308 |
| Detailed / 100K | 8,404,774 | 9,091 | 2,438,300 | 0.338 | 567.269 | 35.480 | 3.306 | 1.906 |
| Default / 1M | 81,023,754 | 90,910 | 24,382,828 | 0.112 | 4,503.974 | 268.822 | 4.124 | 1.455 |
| Detailed / 1M | 84,954,224 | 90,910 | 24,382,828 | 0.376 | 3,805.411 | 243.452 | 2.845 | 1.106 |

The benchmark's default/detailed profiles are schema-1 compatibility profiles. RF5 must repeat this exact workload and also add a schema-3 ignored-field workload, comparing retained capture bytes to an equivalent retained field profile.

## Pending evidence

RF1 does not alter UI. The user-provided current-window screenshots remain the visual baseline; RF2 will add native light/dark and constrained-viewport evidence. Windows/Linux and DPI/IME verification are RF3/RF8 work, not validated by this macOS run.
