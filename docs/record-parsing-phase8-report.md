# Record parsing Phase 8 — local verification and performance

Date: 2026-09-12. Source: Phase 7 commit `9a36ab9` plus the Phase 8 benchmark/documentation commit. Host: MacBookAir9,1 (Intel x86_64, 16 GiB RAM), macOS 15.7.7, Rust 1.97.1. No push, tag, or release was made.

## Method

Release executables are built with `--no-default-features` for production-path parsing measurements. File generation and compilation are outside the timed stages. Each input is deterministic (`gen_record_logs`, seed 20260911), and the benchmark asserts that resolved overview/lane totals equal indexed record starts/filter hits. One process runs at a time after the release build completes; the first run warms the OS cache. Load, search, timeline build, and 1,000-column/three-lane resolution are separate wall-clock stages. `/usr/bin/time -l` supplies peak process RSS, which includes mmap residency and the entire benchmark process, not just retained indexes. These are cache-warm local results, not disk-cold or cross-platform claims.

| Input | Bytes | Physical lines | Expected record starts | SHA-256 |
| --- | ---: | ---: | ---: | --- |
| Single-line ISO text | 9,840,096 | 100,000 | 100,000 | `4829b7e411746cb515da74510ebd4450ef01417009cac4e357bc1540f0798748` |
| Default ISO template | 8,011,818 | 100,000 | 9,091 | `e4ed8cad301f229925f0a364272c65a326d4faed677f6ed00a4807e0eba2c366` |
| Sparse ISO text, 100 continuations/header | 7,848,918 | 100,000 | 991 | `3145aa51aed2c1692328d397040ec3d9f67983210c3dad4ae85e338cb78304c8` |
| Six-field detailed ISO template | 8,404,774 | 100,000 | 9,091 | `a469ddd148c580c5d6b2b9be9042ac9e54f96407027e79b9f791d2dd6b5e1432` |
| Default ISO template, 1M lines | 81,023,754 | 1,000,000 | 90,910 | `1ba8dc248f40cc851e2c645d041fa263669425b23425d1d86411fa30db71902b` |
| Reversed-clock ISO text | 8,011,818 | 100,000 | 9,091 | `d1595010f44319d8177046b6dd6a8612de0e645f8748f5ef74d13fd82bbe0612` |
| CI-style iOS-100K, generator seed 42 | 12,917,930 | 100,000 | Audited by run | `2e1513d9f59c85f68847098228d9f85861b024cce4d6dd9979052d202b4bbe39` |
| Existing iOS-1M corpus | 129,158,002 | 1,000,000 | Audited by run | `db6fb2e7144e99ae3e44f3119843710015ffa5baebe4e6f902799a9f8d20f713` |

The pre-implementation reference is commit `8e9d239` in [the phased plan](record-parsing-phased-implementation.md#current-baseline-measurements). It measured three warmed runs with the same `bench` executable and `--no-default-features` on this architecture, but did not capture hardware model. Comparisons to that reference are directional: record semantics and the GUI timeline changed, and a 5% regression cannot be established from those noisy three-run figures.

## Results

One warm-up followed by five sequential measured runs per input. Values are medians with min–max in parentheses; stage times are milliseconds and RSS is MiB. The synthetic inputs use explicit `default`/`detailed` profiles and an ISO timestamp parser through `bench_record`; iOS inputs use Auto through the existing `bench`. These two harnesses should not be compared as if they had identical profile selection work. All synthetic record counts matched the generator's independent count, and benchmark overview/lane assertions passed.

| Workload | Record starts | Load ms | Timeline build ms | 1,000-column + 3-lane resolve ms | Peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Single-line 100K | 100,000 | 244.9 (238.9–257.6) | 2.62 (2.33–2.73) | 0.86 (0.81–0.97) | 28.3 (28.2–28.3) |
| Default 100K | 9,091 | 255.3 (244.2–269.6) | 1.61 (1.53–1.84) | 0.79 (0.75–0.90) | 23.6 (23.6–23.8) |
| Detailed six-field 100K | 9,091 | 333.6 (280.3–1700.6) | 1.93 (1.64–2.01) | 0.95 (0.76–1.02) | 24.3 (24.2–24.3) |
| Sparse-header 100K | 991 | 694.9 (321.1–4611.1) | 2.34 (1.86–40.85) | 1.28 (0.90–17.18) | 23.4 (23.3–23.6) |
| Reversed-clock 100K | 9,091 | 570.6 (328.8–4218.8) | 1.68 (1.40–32.54) | 0.79 (0.70–20.68) | 23.8 (23.7–23.9) |
| Default 1M | 90,910 | 2911.4 (2686.0–18199.5) | 2.74 (2.19–47.97) | 1.06 (0.83–26.42) | 124.3 (124.3–124.5) |
| CI-style iOS-100K | 97,458 | 915.1 (556.3–4400.0) | 1.80 (1.40–2.10) | 0.87 (0.71–1.50) | 25.6 (25.4–25.7) |
| Existing iOS-1M | 973,287 | 9900 (5300–18600) | 2.30 (2.20–44.20) | 1.20 (0.94–15.50) | 171.2 (171.1–171.2) |

The explicit-layout harness's three filter counts (`ERROR` / `request` / `payload_date`) were 0/9,091/9,091 for default 100K, 1,515/9,091/9,091 for detailed 100K, and 0/90,910/90,910 for default 1M. The iOS harness used `ERROR` / `user_id`: 4,960/4,245 at 100K and 48,547/42,195 at 1M. Logical retained main-index payload, excluding capacity/mmap/Drain, was 2,438,300 bytes at 100K and 24,382,828 bytes at 1M, or about 24.38 bytes per physical line as planned.

The detailed layout's observed median load is about 31% above the matched default generator, but its sample includes a 1.7 s outlier and the layouts have different header normalization work. A follow-up alternating default/detailed run gave medians 326/419 ms (within-pair median ratio 1.29); the first two pairs were multi-second system-delay outliers. This is **not** a measurement against an equivalent specialized six-field parser, so the plan's 15% budget cannot be judged from it. Sparse/reversed/iOS load ranges are also too wide to establish a stable percentage. The iOS-1M historical median was 6.2 s and 163.0 MiB; this run's 9.9 s and 171.2 MiB warrant an isolated remeasurement before claiming either a regression or acceptance. Its fastest 5.3 s run matches the historical minimum, while record semantics and process conditions differ. Resolution remained below 4 ms in the median three-lane test, but 20-lane p95 and full UI frame cost were not measured.

Reproduce after a release build, replacing `FILE` with one of the fingerprinted inputs:

```sh
cargo test --release
cargo test --release --example gen_ios_logs
cargo build --release --no-default-features --example bench --example bench_record
/usr/bin/time -l target/release/examples/bench_record FILE default
/usr/bin/time -l target/release/examples/bench_record FILE detailed
/usr/bin/time -l target/release/examples/bench FILE ERROR user_id
```

## Verification and remaining limits

The final full debug suite passed: 355 core/library tests, 164 desktop tests, and all integration targets, including the empty-file/first-append regression. MCP-only `cargo check --no-default-features --all-targets` and 354 MCP-only library tests passed before that test-only addition. The full `cargo test --release` suite passed 354 library, 164 desktop, and integration targets before the test-only addition; production code did not change after that run. `cargo test --release --example gen_ios_logs` passed seven tests; `cargo test --example bench_record --no-default-features` passed its layout-compilation test. The seeded generator produced the 100K iOS fixture outside the repo. `cargo check --no-default-features --target x86_64-unknown-linux-gnu --lib` and the corresponding `x86_64-pc-windows-msvc` check both passed; no cross-platform binaries were executed here.

Automated tests cover byte-split and repeated live append, record ownership/time/span/Drain equivalence, sparse detection, detailed templates, backward clocks, source-order queries, line-domain overview, legacy MCP histogram behavior, sidecar profile selection, and GUI reparse state. A full interactive GUI walkthrough, UI frame-time p95, 20 filter lanes, 10M-line/append latency, and true Windows/Linux/Apple Silicon runtime tests require dedicated runners and are not inferred from this Intel Mac. The GUI currently shows source-line labels without the planned in-axis event-time annotations or sparse clock markers; clock diagnostics remain available through time-query/MCP paths. During live append, document publication precedes completion of the asynchronous filter-lane refresh, so the old lanes can be briefly visible on the new document. Those are known follow-up items, not validated release gates.
