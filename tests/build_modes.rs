//! Opt-in benchmark (`#[ignore]`d): build a compute- and allocation-heavy snippet in each of
//! `pro_compile`'s three modes and confirm they behave — **tiny** is the smallest binary,
//! **debug** compiles fastest, **release** runs fastest. It shells out to `cargo` (three full
//! builds), so it's slow and environment-sensitive; run it deliberately:
//!
//! ```text
//! cargo test --test build_modes -- --ignored --nocapture
//! ```
//!
//! The mode flags mirror `src/categories/project.rs`; the `tiny` build here omits the
//! `cargo bloat` *report* (it only prints sizes and isn't needed to measure the binary).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Recursion + heap allocation (memoized fib), a few monomorphized hot loops (optimizer work
// at both compile and run time), and a bulk allocation. `black_box` on the loop counts keeps
// the compiler from const-folding the loops away, so the work really happens at runtime.
const MAIN_RS: &str = r#"
use std::collections::HashMap;
use std::hint::black_box;

fn fib(n: u64, memo: &mut HashMap<u64, u64>) -> u64 {
    if n < 2 { return n; }
    if let Some(&v) = memo.get(&n) { return v; }
    let v = fib(n - 1, memo).wrapping_add(fib(n - 2, memo));
    memo.insert(n, v);
    v
}

fn crunch<const K: u64>(rounds: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..rounds {
        acc = acc.wrapping_add(i.wrapping_mul(K).rotate_left(13) ^ (i >> 3));
    }
    acc
}

fn main() {
    let mut memo = HashMap::new();
    let mut total = fib(90, &mut memo);
    total = total.wrapping_add(crunch::<1>(black_box(50_000_000)));
    total = total.wrapping_add(crunch::<3>(black_box(50_000_000)));
    total = total.wrapping_add(crunch::<7>(black_box(50_000_000)));
    total = total.wrapping_add(crunch::<11>(black_box(50_000_000)));
    let v: Vec<u64> = (0..3_000_000u64).map(|x| x.wrapping_mul(2654435761) >> 3).collect();
    total = total.wrapping_add(v.iter().copied().fold(0u64, u64::wrapping_add));
    println!("{total}");
}
"#;

const CARGO_TOML: &str = "[package]\nname = \"bench_snippet\"\nversion = \"0.0.0\"\nedition = \"2021\"\n";

struct Measured {
    compile: Duration,
    size: u64,
    runtime: Duration,
}

fn rustflags(opt_level: char) -> String {
    format!("-C opt-level={opt_level} -C target-cpu=native -C strip=symbols -C panic=abort -C codegen-units=1")
}

/// Clean, then time a full build; measure the binary size and its fastest of three runs.
fn build(project: &Path, release: bool, opt_level: Option<char>) -> Measured {
    // Clean first so every mode is timed as a full build (comparable compile times).
    assert!(Command::new("cargo").current_dir(project).arg("clean").status().unwrap().success());

    let mut cmd = Command::new("cargo");
    cmd.current_dir(project).arg("build");
    if release {
        cmd.arg("--release");
    }
    if let Some(opt) = opt_level {
        cmd.env("RUSTFLAGS", rustflags(opt));
    }
    let start = Instant::now();
    assert!(cmd.status().unwrap().success(), "snippet build failed");
    let compile = start.elapsed();

    let bin = project.join("target").join(if release { "release" } else { "debug" }).join("bench_snippet");
    let size = fs::metadata(&bin).unwrap().len();

    let runtime = (0..3)
        .map(|_| {
            let t = Instant::now();
            assert!(Command::new(&bin).stdout(Stdio::null()).status().unwrap().success());
            t.elapsed()
        })
        .min()
        .unwrap();

    Measured { compile, size, runtime }
}

#[test]
#[ignore = "slow benchmark; run with `--ignored --nocapture`"]
fn build_modes_behave_as_intended() {
    // A scratch crate under the system temp dir (no external crates involved).
    let project: PathBuf = std::env::temp_dir().join(format!("bashrs_build_bench_{}", std::process::id()));
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("Cargo.toml"), CARGO_TOML).unwrap();
    fs::write(project.join("src").join("main.rs"), MAIN_RS).unwrap();

    let debug = build(&project, false, None); // `cargo build`
    let release = build(&project, true, Some('3')); // opt-level=3 (fastest runtime)
    let tiny = build(&project, true, Some('z')); // opt-level=z (smallest)

    println!("\nmode     compile      size        runtime");
    let row = |n: &str, m: &Measured| {
        println!("{n:<8} {:>7.2}s {:>9} B {:>10.3}s", m.compile.as_secs_f64(), m.size, m.runtime.as_secs_f64());
    };
    row("debug", &debug);
    row("release", &release);
    row("tiny", &tiny);

    fs::remove_dir_all(&project).unwrap();

    // tiny → smallest: opt-level=z is <= opt-level=3, and both stripped builds are far
    // smaller than the unstripped, unoptimized debug binary.
    assert!(tiny.size <= release.size, "tiny ({}) should be <= release ({})", tiny.size, release.size);
    assert!(release.size < debug.size, "release ({}) should be smaller than debug ({})", release.size, debug.size);

    // release → fastest at runtime: opt-level=3 beats unoptimized debug decisively, and is
    // no slower than size-first tiny (small tolerance for scheduler noise).
    assert!(release.runtime < debug.runtime, "release should run faster than debug");
    assert!(release.runtime <= tiny.runtime.mul_f64(1.10), "release should be ~<= tiny at runtime");

    // debug → fastest to compile: no optimization + parallel codegen units.
    assert!(debug.compile < release.compile, "debug should compile faster than release");
    assert!(debug.compile < tiny.compile, "debug should compile faster than tiny");
}
