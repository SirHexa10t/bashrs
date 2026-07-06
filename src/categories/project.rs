//! Project commands (`pro_*`): detect the toolchain from the marker files in a directory and
//! run the right build / test / run / dependency-update — or, where a step isn't applicable,
//! print how to do it by hand.
//!
//! Adding a language (or package manager) is one row in [`TOOLCHAINS`]. The first row whose
//! marker is present wins, so more specific markers (lockfiles) come before general ones, and
//! a bare `Makefile` comes last. Each build has three modes: `-d`/debug (fastest to compile),
//! `-r`/release (fastest at runtime — the default), and `-t`/tiny (smallest binary).
//
//
//  ┌──────────────────────────────────────────┬───────────────────────────────────────┬───────────────────────────┬───────────────────────┬────────────────────────────────────┐
//  │            Toolchain (marker)            │                compile                │           test            │          run          │          update_packages           │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Rust (Cargo.toml)                        │ cargo build (--release/--tiny)        │ cargo test --all-features │ cargo run             │ cargo update                       │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Go (go.mod)                              │ go build ./...                        │ go test ./...             │ go run .              │ go get -u ./... && go mod tidy     │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Node (package.json, pm by lockfile)      │ <pm> run build                        │ <pm> test                 │ <pm> start            │ <pm> update                        │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ TypeScript (tsconfig.json)               │ tsc                                   │ <pm> test                 │ <pm> start            │ <pm> update                        │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Python·uv (uv.lock)                      │ —                                     │ uv run pytest             │ uv run <entry> ❓      │ uv lock --upgrade && uv sync       │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Python·poetry (pyproject.toml w/ poetry) │ —                                     │ poetry run pytest         │ poetry run <entry> ❓  │ poetry update                      │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Python·pip (requirements.txt)            │ —                                     │ pytest                    │ python <entry> ❓      │ pip install -U -r requirements.txt │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Java·Maven (pom.xml)                     │ mvn compile                           │ mvn test                  │ mvn exec:java         │ mvn versions:use-latest-releases   │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Java·Gradle (build.gradle)               │ gradle build                          │ gradle test               │ gradle run            │ ❓ (needs a plugin)                 │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ C/C++·CMake (CMakeLists.txt)             │ cmake -B build && cmake --build build │ ctest --test-dir build    │ ❓ (which binary)      │ —                                  │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ C/C++·Make (Makefile)                    │ make                                  │ make test                 │ ❓                     │ —                                  │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Ruby (Gemfile)                           │ —                                     │ bundle exec rake test     │ ruby <entry> ❓        │ bundle update                      │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Zig (build.zig)                          │ zig build                             │ zig build test            │ zig build run         │ ❓ (young pkg mgmt)                 │
//  ├──────────────────────────────────────────┼───────────────────────────────────────┼───────────────────────────┼───────────────────────┼────────────────────────────────────┤
//  │ Perl (cpanfile/Makefile.PL)              │ perl -c <main>                        │ prove -l t/               │ perl <main> ❓         │ cpanm --installdeps .              │
//  └──────────────────────────────────────────┴───────────────────────────────────────┴───────────────────────────┴───────────────────────┴────────────────────────────────────┘
//
// <pm> = detected package manager (npm/yarn/pnpm/bun by lockfile). ❓ = a genuinely ambiguous cell.


#[bashrs_macros::category(command = ProjectCommand, prefix = "pro_")]
mod commands {
    use crate::support::exec;
    use clap::Args;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    /// What to do for a given (toolchain, action): run a shell command, or — when the action
    /// isn't applicable — print guidance (usually the command to run by hand).
    #[derive(Clone, Copy)]
    enum Step {
        Run(&'static str),
        Note(&'static str),
    }

    /// A detectable toolchain and how to drive it. The three build modes are `debug` (fastest
    /// to compile), `release` (fastest at runtime — the default), and `tiny` (smallest binary).
    /// `test_needs_build` compiles (debug) before testing, for runners that expect an
    /// already-built tree (e.g. CMake's `ctest`).
    struct Toolchain {
        name: &'static str,
        markers: &'static [&'static str],
        debug: Step,
        release: Step,
        tiny: Step,
        test: Step,
        test_needs_build: bool,
        run: Step,
        update: Step,
    }

    // opt-level 3 = fastest runtime, z = smallest; strip-symbols can break WebAssembly.
    const RUST_RELEASE: &str = r#"RUSTFLAGS="-C opt-level=3 -C target-cpu=native -C strip=symbols -C panic=abort -C codegen-units=1" cargo build --release"#;
    const RUST_TINY: &str = r#"RUSTFLAGS="-C opt-level=z -C target-cpu=native -C strip=symbols -C panic=abort -C codegen-units=1" cargo build --release && cargo bloat --release --crates"#;

    /// Build step for interpreted languages — there's nothing to compile.
    const NO_BUILD: Step = Step::Note("interpreted — nothing to compile");
    /// Build step for Perl — the closest thing is a syntax check.
    const PERL_CHECK: Step = Step::Note("interpreted — syntax-check a file with:  perl -c <file>");

    const TOOLCHAINS: &[Toolchain] = &[
        Toolchain {
            name: "Rust",
            markers: &["Cargo.toml"],
            debug: Step::Run("cargo build"),
            release: Step::Run(RUST_RELEASE),
            tiny: Step::Run(RUST_TINY),
            test: Step::Run("cargo test --all-features"),
            test_needs_build: false,
            run: Step::Run("cargo run"),
            update: Step::Run("cargo update"),
        },
        Toolchain {
            name: "Go",
            markers: &["go.mod"],
            debug: Step::Run("go build -gcflags='all=-N -l'"), // disable optimization + inlining
            release: Step::Run("go build -trimpath -ldflags='-s -w'"), // strip debug info
            tiny: Step::Run("go build -trimpath -ldflags='-s -w'"), // Go's smallest native build
            test: Step::Run("go test ./..."),
            test_needs_build: false,
            run: Step::Run("go run ."),
            update: Step::Run("go get -u ./... && go mod tidy"),
        },
        // Node — build/test/run are the package manager's scripts; the build mode is up to the
        // project's own `build` script, so all three modes invoke it.
        Toolchain {
            name: "Node (pnpm)",
            markers: &["pnpm-lock.yaml"],
            debug: Step::Run("pnpm run build"),
            release: Step::Run("pnpm run build"),
            tiny: Step::Run("pnpm run build"),
            test: Step::Run("pnpm test"),
            test_needs_build: false,
            run: Step::Run("pnpm start"),
            update: Step::Run("pnpm update"),
        },
        Toolchain {
            name: "Node (yarn)",
            markers: &["yarn.lock"],
            debug: Step::Run("yarn run build"),
            release: Step::Run("yarn run build"),
            tiny: Step::Run("yarn run build"),
            test: Step::Run("yarn test"),
            test_needs_build: false,
            run: Step::Run("yarn start"),
            update: Step::Run("yarn upgrade"),
        },
        Toolchain {
            name: "Node (bun)",
            markers: &["bun.lockb"],
            debug: Step::Run("bun run build"),
            release: Step::Run("bun run build"),
            tiny: Step::Run("bun run build"),
            test: Step::Run("bun test"),
            test_needs_build: false,
            run: Step::Run("bun start"),
            update: Step::Run("bun update"),
        },
        Toolchain {
            name: "Node (npm)",
            markers: &["package.json"],
            debug: Step::Run("npm run build"),
            release: Step::Run("npm run build"),
            tiny: Step::Run("npm run build"),
            test: Step::Run("npm test"),
            test_needs_build: false,
            run: Step::Run("npm start"),
            update: Step::Run("npm update"),
        },
        Toolchain {
            name: "TypeScript",
            markers: &["tsconfig.json"],
            debug: Step::Run("tsc --sourceMap"),
            release: Step::Run("tsc"),
            tiny: Step::Run("tsc --removeComments"),
            test: Step::Note("standalone TS (no package.json) — add a test runner; inside a Node project pro_test uses the package manager"),
            test_needs_build: false,
            run: Step::Note("compile with tsc, then run the output:  node <output>.js"),
            update: Step::Note("standalone TS has no manifest — add a package.json to manage dependencies"),
        },
        Toolchain {
            name: "Python (uv)",
            markers: &["uv.lock"],
            debug: NO_BUILD,
            release: NO_BUILD,
            tiny: NO_BUILD,
            test: Step::Run("uv run pytest"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  uv run python <main-file>"),
            update: Step::Run("uv lock --upgrade && uv sync"),
        },
        Toolchain {
            name: "Python (poetry)",
            markers: &["poetry.lock"],
            debug: NO_BUILD,
            release: NO_BUILD,
            tiny: NO_BUILD,
            test: Step::Run("poetry run pytest"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  poetry run python <main-file>"),
            update: Step::Run("poetry update"),
        },
        Toolchain {
            name: "Python (pip)",
            markers: &["requirements.txt"],
            debug: NO_BUILD,
            release: NO_BUILD,
            tiny: NO_BUILD,
            test: Step::Run("pytest"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  python3 <main-file>"),
            update: Step::Run("pip install -U -r requirements.txt"),
        },
        Toolchain {
            name: "Python",
            markers: &["pyproject.toml", "setup.py"],
            debug: NO_BUILD,
            release: NO_BUILD,
            tiny: NO_BUILD,
            test: Step::Run("pytest"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  python3 <main-file>"),
            update: Step::Note("no lockfile found — install deps with your tool (pip / poetry / uv)"),
        },
        Toolchain {
            name: "Java (Maven)",
            markers: &["pom.xml"],
            debug: Step::Run("mvn compile"),
            release: Step::Run("mvn package"),
            tiny: Step::Note("no size-optimized build — use the Shade plugin's minimizeJar"),
            test: Step::Run("mvn test"),
            test_needs_build: false,
            run: Step::Run("mvn exec:java"),
            update: Step::Run("mvn versions:use-latest-releases"),
        },
        Toolchain {
            name: "Java (Gradle)",
            markers: &["build.gradle", "build.gradle.kts"],
            debug: Step::Run("gradle build -x test"),
            release: Step::Run("gradle build"),
            tiny: Step::Note("no size-optimized build — use a shadow/minimize plugin"),
            test: Step::Run("gradle test"),
            test_needs_build: false,
            run: Step::Run("gradle run"),
            update: Step::Note("no built-in upgrade — use the Versions plugin (com.github.ben-manes.versions) or edit build.gradle"),
        },
        Toolchain {
            name: "Ruby",
            markers: &["Gemfile"],
            debug: NO_BUILD,
            release: NO_BUILD,
            tiny: NO_BUILD,
            test: Step::Run("bundle exec rake test"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  ruby <main-file>  (or bundle exec ruby ...)"),
            update: Step::Run("bundle update"),
        },
        Toolchain {
            name: "Zig",
            markers: &["build.zig"],
            debug: Step::Run("zig build -Doptimize=Debug"),
            release: Step::Run("zig build -Doptimize=ReleaseFast"),
            tiny: Step::Run("zig build -Doptimize=ReleaseSmall"),
            test: Step::Run("zig build test"),
            test_needs_build: false,
            run: Step::Run("zig build run"),
            update: Step::Note("package management is young — add a dependency with:  zig fetch --save <url>"),
        },
        Toolchain {
            name: "Perl",
            markers: &["cpanfile", "Makefile.PL"],
            debug: PERL_CHECK,
            release: PERL_CHECK,
            tiny: PERL_CHECK,
            test: Step::Run("prove -l t/"),
            test_needs_build: false,
            run: Step::Note("interpreted — run it with:  perl <main-file>"),
            update: Step::Run("cpanm --installdeps ."),
        },
        Toolchain {
            name: "C/C++ (CMake)",
            markers: &["CMakeLists.txt"],
            debug: Step::Run("cmake -B build -DCMAKE_BUILD_TYPE=Debug && cmake --build build"),
            release: Step::Run("cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build"),
            tiny: Step::Run("cmake -B build -DCMAKE_BUILD_TYPE=MinSizeRel && cmake --build build"),
            test: Step::Run("ctest --test-dir build"),
            test_needs_build: true, // ctest runs an already-built tree
            run: Step::Note("no single entry point — run your built binary directly, e.g.  ./build/<target>"),
            update: Step::Note("C/C++ has no standard package manager"),
        },
        Toolchain {
            name: "C/C++ (Make)",
            markers: &["Makefile", "makefile"],
            debug: Step::Run("make"),
            release: Step::Run("make"),
            tiny: Step::Note("Make has no standard size mode — set optimization flags in your Makefile"),
            test: Step::Run("make test"),
            test_needs_build: false,
            run: Step::Note("no single entry point — run your built binary directly, e.g.  ./<target>"),
            update: Step::Note("C/C++ has no standard package manager"),
        },
    ];

    /// Build the project in DIR (default `.`); `-d` debug (fastest compile), `-r` release
    /// (fastest runtime, the default), `-t` tiny (smallest binary)
    pub fn compile(args: CompileArgs) {
        let dir = Path::new(&args.dir);
        let Some(tc) = _detect(dir) else { return };
        let step = if args.tiny {
            tc.tiny
        } else if args.debug {
            tc.debug
        } else {
            tc.release // default
        };
        println!("Detected {}", tc.name);
        _run(step, dir);
    }

    /// Run the test suite of the project in DIR (default `.`)
    pub fn test(args: DirArgs) {
        let dir = Path::new(&args.dir);
        let Some(tc) = _detect(dir) else { return };
        println!("Detected {}", tc.name);
        // Some test runners (e.g. CMake's ctest) expect an already-built tree — do the
        // fastest (debug) build first, and stop if it fails.
        if tc.test_needs_build && !_run(tc.debug, dir) {
            return;
        }
        _run(tc.test, dir);
    }

    /// Build and run the project in DIR (default `.`)
    pub fn run(args: DirArgs) {
        let dir = Path::new(&args.dir);
        if let Some(tc) = _detect(dir) {
            println!("Detected {}", tc.name);
            _run(tc.run, dir);
        }
    }

    /// Upgrade the dependencies of the project in DIR (default `.`)
    pub fn update_packages(args: DirArgs) {
        let dir = Path::new(&args.dir);
        if let Some(tc) = _detect(dir) {
            println!("Detected {}", tc.name);
            _run(tc.update, dir);
        }
    }

    /// Show the project's README, else its manifest description, else its main file's doc
    pub fn readme(args: DirArgs) {
        let dir = Path::new(&args.dir);
        if let Some(path) = _find_readme(dir) {
            match fs::read_to_string(&path) {
                Ok(text) => print!("{text}"),
                Err(err) => eprintln!("pro_readme: cannot read {}: {err}", path.display()),
            }
        } else if let Some(desc) = _manifest_description(dir) {
            println!("{desc}");
        } else if let Some(doc) = _main_doc(dir) {
            println!("{doc}");
        } else {
            eprintln!("pro_readme: found no README, manifest description, or top-of-file doc comment in '{}'", dir.display());
        }
    }

    /// The directory to act in (default: current directory).
    #[derive(Args)]
    pub struct DirArgs {
        #[arg(default_value = ".")]
        dir: String,
    }

    /// The directory to act in, plus the build mode (default: release). `--tiny` wins over
    /// `--debug`, which wins over `--release`.
    #[derive(Args)]
    pub struct CompileArgs {
        #[arg(default_value = ".")]
        dir: String,
        /// Debug build: fastest to compile.
        #[arg(short, long)]
        debug: bool,
        /// Release build: fastest at runtime (the default).
        #[arg(short, long)]
        release: bool,
        /// Tiny build: smallest binary.
        #[arg(short, long)]
        tiny: bool,
    }

    /// The first toolchain any of whose markers satisfies `present` — table order is priority.
    fn _detect_with(present: impl Fn(&str) -> bool) -> Option<&'static Toolchain> {
        TOOLCHAINS.iter().find(|tc| tc.markers.iter().any(|m| present(m)))
    }

    /// The first toolchain whose marker files exist in `dir`; prints a note if none match.
    fn _detect(dir: &Path) -> Option<&'static Toolchain> {
        let found = _detect_with(|m| dir.join(m).exists());
        if found.is_none() {
            eprintln!("pro: no recognized project in '{}' (looked for the markers of {} toolchains)", dir.display(), TOOLCHAINS.len());
        }
        found
    }

    /// Run a resolved step in `dir`, returning whether it succeeded (a Note always does). Runs
    /// via `bash -c` so the stored `&&`/env-prefixed snippets work.
    fn _run(step: Step, dir: &Path) -> bool {
        match step {
            Step::Run(cmd) => {
                if let Err(err) = std::env::set_current_dir(dir) {
                    eprintln!("pro: cannot enter '{}': {err}", dir.display());
                    return false;
                }
                println!("running:  {cmd}");
                let started = Instant::now();
                let ok = exec::run_reporting("bash", ["-c", cmd]);
                if ok {
                    println!("finished in {:.1}s", started.elapsed().as_secs_f32());
                }
                ok
            }
            Step::Note(msg) => {
                println!("{msg}");
                true
            }
        }
    }

    // ——— pro_readme support ————————————————————————————————————————————————

    /// A manifest format `pro_readme` can parse for a project description.
    #[derive(Clone, Copy)]
    enum Manifest {
        Toml,
        Json,
    }

    /// Where to read a one-line project description — `(manifest, format, key path)`.
    /// `pyproject.toml` appears twice: PEP 621 `[project]` and Poetry's `[tool.poetry]`.
    const DESCRIPTIONS: &[(&str, Manifest, &[&str])] = &[
        ("Cargo.toml", Manifest::Toml, &["package", "description"]),
        ("pyproject.toml", Manifest::Toml, &["project", "description"]),
        ("pyproject.toml", Manifest::Toml, &["tool", "poetry", "description"]),
        ("package.json", Manifest::Json, &["description"]),
    ];

    /// Entry files whose leading doc comment describes the project (best-effort fallback).
    const DOC_FILES: &[&str] = &[
        "src/lib.rs", "src/main.rs", "main.go", "main.py", "__init__.py", "src/index.ts",
        "index.ts", "src/index.js", "index.js", "main.c", "main.cpp", "main.rb",
    ];

    /// The first `README*` file in `dir` (preferring a `.md`), if any.
    fn _find_readme(dir: &Path) -> Option<PathBuf> {
        let mut readmes: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.to_uppercase().starts_with("README"))
            })
            .collect();
        readmes.sort();
        readmes
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "md"))
            .cloned()
            .or_else(|| readmes.into_iter().next())
    }

    /// A project description read from a manifest (see [`DESCRIPTIONS`]).
    fn _manifest_description(dir: &Path) -> Option<String> {
        for &(file, format, keys) in DESCRIPTIONS {
            let Ok(content) = fs::read_to_string(dir.join(file)) else { continue };
            let desc = match format {
                Manifest::Toml => toml::from_str::<toml::Value>(&content).ok().and_then(|v| _toml_str(&v, keys)),
                Manifest::Json => serde_json::from_str::<serde_json::Value>(&content).ok().and_then(|v| _json_str(&v, keys)),
            };
            if let Some(desc) = desc {
                return Some(desc);
            }
        }
        None
    }

    /// Follow a key path through a TOML value to a string.
    fn _toml_str(value: &toml::Value, path: &[&str]) -> Option<String> {
        let mut cur = value;
        for key in path {
            cur = cur.get(key)?;
        }
        cur.as_str().map(str::to_owned)
    }

    /// Follow a key path through a JSON value to a string.
    fn _json_str(value: &serde_json::Value, path: &[&str]) -> Option<String> {
        let mut cur = value;
        for key in path {
            cur = cur.get(key)?;
        }
        cur.as_str().map(str::to_owned)
    }

    /// The leading doc comment of the first present [`DOC_FILES`] entry in `dir`.
    fn _main_doc(dir: &Path) -> Option<String> {
        for &file in DOC_FILES {
            let Ok(content) = fs::read_to_string(dir.join(file)) else { continue };
            if let Some(doc) = _leading_doc(&content) {
                return Some(doc);
            }
        }
        None
    }

    /// Best-effort extraction of a file's leading doc comment: a `"""`/`'''`/`/* */` block, or
    /// a run of `//!`/`///`/`//`/`#` lines at the top, with the markers stripped.
    fn _leading_doc(source: &str) -> Option<String> {
        let lines: Vec<&str> = source.lines().collect();
        // Skip leading blanks and a shebang / inner attribute like `#![...]`.
        let start = lines.iter().position(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with("#!")
        })?;
        let body = &lines[start..];
        let first = body[0].trim_start();

        for open in ["\"\"\"", "'''", "/*"] {
            if let Some(after) = first.strip_prefix(open) {
                let close = if open == "/*" { "*/" } else { open };
                let mut text = String::new();
                if let Some(end) = after.find(close) {
                    text.push_str(&after[..end]);
                } else {
                    text.push_str(after);
                    for line in &body[1..] {
                        if let Some(end) = line.find(close) {
                            text.push('\n');
                            text.push_str(&line[..end]);
                            break;
                        }
                        text.push('\n');
                        text.push_str(line);
                    }
                }
                let cleaned =
                    text.lines().map(|l| l.trim().trim_start_matches('*').trim()).collect::<Vec<_>>().join("\n");
                return _nonempty(cleaned.trim());
            }
        }

        let mut out: Vec<&str> = Vec::new();
        for line in body {
            let t = line.trim_start();
            match t
                .strip_prefix("//!")
                .or_else(|| t.strip_prefix("///"))
                .or_else(|| t.strip_prefix("//"))
                .or_else(|| t.strip_prefix('#'))
            {
                Some(rest) => out.push(rest.trim()),
                None => break,
            }
        }
        _nonempty(out.join("\n").trim())
    }

    /// `Some(s)` unless `s` is empty.
    fn _nonempty(s: &str) -> Option<String> {
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn detect(present: &[&str]) -> Option<&'static str> {
            _detect_with(|m| present.contains(&m)).map(|tc| tc.name)
        }

        #[test]
        fn detects_by_marker_file() {
            assert_eq!(detect(&["Cargo.toml"]), Some("Rust"));
            assert_eq!(detect(&["go.mod"]), Some("Go"));
            assert_eq!(detect(&["Gemfile"]), Some("Ruby"));
            assert_eq!(detect(&["build.zig"]), Some("Zig"));
            assert_eq!(detect(&[]), None);
        }

        #[test]
        fn more_specific_markers_win() {
            // a yarn project also has package.json — yarn.lock must be detected first
            assert_eq!(detect(&["yarn.lock", "package.json"]), Some("Node (yarn)"));
            // a Rust project with a convenience Makefile is still Rust, not C/C++
            assert_eq!(detect(&["Cargo.toml", "Makefile"]), Some("Rust"));
            // uv beats a bare pyproject
            assert_eq!(detect(&["uv.lock", "pyproject.toml"]), Some("Python (uv)"));
        }

        #[test]
        fn rust_build_modes_are_distinct() {
            let rust = _detect_with(|m| m == "Cargo.toml").unwrap();
            assert!(matches!(rust.debug, Step::Run("cargo build")), "debug: plain, fastest to compile");
            assert!(matches!(rust.release, Step::Run(c) if c.contains("opt-level=3")), "release: optimize for speed");
            assert!(matches!(rust.tiny, Step::Run(c) if c.contains("opt-level=z")), "tiny: optimize for size");
        }

        #[test]
        fn cmake_tests_build_first_but_cargo_does_not() {
            assert!(_detect_with(|m| m == "CMakeLists.txt").unwrap().test_needs_build);
            assert!(!_detect_with(|m| m == "Cargo.toml").unwrap().test_needs_build); // cargo test builds itself
        }

        #[test]
        fn leading_doc_extracts_common_styles() {
            assert_eq!(_leading_doc("//! A tool.\n//! Does things.\nfn main() {}").as_deref(), Some("A tool.\nDoes things."));
            assert_eq!(_leading_doc("\"\"\"A Python module.\"\"\"\nimport os").as_deref(), Some("A Python module."));
            assert_eq!(_leading_doc("#!/bin/sh\n# a shell script\necho hi").as_deref(), Some("a shell script"));
            assert_eq!(_leading_doc("fn main() {}"), None); // no leading comment
        }

        #[test]
        fn manifest_descriptions_are_extracted() {
            let cargo: toml::Value = toml::from_str("[package]\ndescription = \"A neat crate\"\n").unwrap();
            assert_eq!(_toml_str(&cargo, &["package", "description"]).as_deref(), Some("A neat crate"));
            assert_eq!(_toml_str(&cargo, &["package", "missing"]), None);
            let pkg: serde_json::Value = serde_json::from_str(r#"{"description":"A neat package"}"#).unwrap();
            assert_eq!(_json_str(&pkg, &["description"]).as_deref(), Some("A neat package"));
        }
    }
}
