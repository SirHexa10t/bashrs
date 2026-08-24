//! Process commands (`proc_*`), over `/proc` directly.
//!
//! Read in-process rather than shelled out to `ps`: everything wanted here is a file under
//! `/proc/<pid>/`, and parsing another tool's columnar output back into fields is exactly the kind
//! of round trip `lll` spent so long regretting. A command line can hold anything — spaces,
//! newlines, quotes — and `/proc` hands it over NUL-separated, unambiguous, with no formatting to
//! undo.

#[bashrs_macros::category(command = ProcessesCommand, prefix = "proc_")]
mod commands {
    use crate::support::doc_style::{self, _header, notice, problematic};
    use terminal_choice::prompt_yN;
    use crate::support::theme::Weight;
    use clap::Args;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Find running processes by any part of them — PID, program name, full command line, or the
    /// executable's path — show each match in its process-tree (ancestors above say who owns it,
    /// descendants beneath say what else goes down with it, matched text glowing `grep`-style),
    /// then offer to terminate the matches: SIGTERM first, SIGKILL only for whatever ignores it.
    /// With nothing to hunt at all, offers the top CPU and memory consumers as a checklist
    #[unprefixed]
    #[trailing_newline]
    pub fn murder(args: MurderArgs) {
        if args.patterns.is_empty() && args.window.is_none() {
            return _pick_hogs();
        }
        // The window filter resolves first: it is the one part that can be impossible outright
        // (no X display), and that should say so before any scanning happens.
        let by_window = args.window.as_deref().map(|needle| match crate::support::windows::list() {
            Err(why) => {
                eprintln!("murder: {why}");
                std::process::exit(1);
            }
            Ok(all) => {
                let mut owned: BTreeMap<u32, Vec<(u32, String)>> = BTreeMap::new();
                for window in all {
                    if _find_ci(&window.title, needle).is_some() {
                        owned.entry(window.pid).or_default().push((window.id, window.title));
                    }
                }
                if owned.is_empty() {
                    eprintln!("murder: no window title contains {needle:?}");
                    std::process::exit(1);
                }
                owned
            }
        });
        let (relevant, matches) = _survey(&args.patterns, by_window.as_ref());
        if matches == 0 {
            let mut asked: Vec<String> = args.patterns.clone();
            if let Some(window) = &args.window {
                asked.push(format!("window~{window}"));
            }
            match asked.as_slice() {
                [one] => eprintln!("murder: nothing matches {one:?}"),
                many => eprintln!("murder: nothing matches all {} of {many:?}", many.len()),
            }
            // The window side found something even though no process qualified — show the
            // mapping, so a pid from the wrong namespace (or one gone since) is visible instead
            // of leaving a silent contradiction between the screen and the table.
            for (pid, windows) in by_window.iter().flatten() {
                for (_, title) in windows {
                    eprintln!("  (window {title:?} maps to pid {pid})");
                }
            }
            return;
        }
        // Every needle glows — the CLI patterns in the text they hit, the window needle in the
        // title it hit.
        let mut needles = args.patterns.clone();
        needles.extend(args.window.clone());
        // With --choose the FORM is the tree view (printing it twice would just scroll it away);
        // otherwise the styled report prints and the whole match set is the target.
        if !args.choose {
            for line in _render(&relevant, &needles, args.short) {
                println!("{line}");
            }
        }
        // The action follows the target's grain. A text hunt names processes, so they get
        // signals; a window hunt names windows, and a signal can only ever hit the whole
        // process — so windows get close requests, and every other window of the app lives.
        match by_window {
            Some(owned) => _close_windows(&relevant, &owned, args.choose),
            None if args.choose => match _choose_matches(&relevant, &needles, args.short) {
                None => {} // cancelled, or nothing ticked — messaged already
                // The form WAS the confirmation: ticking boxes and pressing Submit is more
                // deliberate than a y could ever be, and a second prompt is worse than
                // redundant — any keystroke still buffered from the form would answer it.
                Some(chosen) => _kill_confirmed(&_relate(relevant, &chosen)),
            },
            None => _kill(&relevant),
        }
    }

    /// The `--choose` tree: the same rows [`_render`] prints, as a form — every match a
    /// checkbox, every context row a fixed comment, the branch drawing intact because the form
    /// is `aligned` (comments pick up the checkbox lead-in). Returns the ticked PIDs, or `None`
    /// after telling the user why there is nothing to do.
    fn _choose_matches(
        relevant: &BTreeMap<u32, Process>,
        needles: &[String],
        short: bool,
    ) -> Option<Vec<u32>> {
        let mut form = terminal_choice::Form::new()
            .title("murder — tick what dies (context rows are fixed)")
            .aligned()
            .comment(_plain_header(relevant));
        // Consecutive matches pool into one anonymous checkbox group; any context row in
        // between closes the run — that is what keeps the tree's row order intact.
        let mut pending: Vec<String> = Vec::new();
        for (pid, row) in _tree_rows(relevant, needles, short, true) {
            let process = &relevant[&pid];
            if process.relation == Relation::Match && !process.protected {
                pending.push(row);
                continue;
            }
            if !pending.is_empty() {
                let options: Vec<&str> = pending.iter().map(String::as_str).collect();
                form = form.checkboxes("", &options);
                pending.clear();
            }
            form = form.comment(row);
        }
        if !pending.is_empty() {
            let options: Vec<&str> = pending.iter().map(String::as_str).collect();
            form = form.checkboxes("", &options);
        }
        if relevant.values().any(|process| process.protected) {
            form = form.comment("* this shell's own ancestry — never offered");
        }
        match terminal_choice::run(&mut form) {
            Err(err) => {
                eprintln!("murder: {err}");
                None
            }
            Ok(terminal_choice::Outcome::Cancelled) => {
                eprintln!("murder: aborted — nothing chosen");
                None
            }
            Ok(terminal_choice::Outcome::Submitted) => {
                let chosen: Vec<u32> = form
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        terminal_choice::Item::Checkboxes { options, checked, .. } => Some(
                            options
                                .iter()
                                .zip(checked)
                                .filter(|(_, on)| **on)
                                .filter_map(|(option, _)| _picked_pid(option)),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                if chosen.is_empty() {
                    eprintln!("murder: nothing ticked — nothing to signal");
                    return None;
                }
                Some(chosen)
            }
        }
    }

    /// The tree as PLAIN rows, `(pid, row text)` in display order — what the `--choose` form is
    /// built from. Same columns, same walk, same command cell as the styled report, minus every
    /// escape code (a form option carrying colour codes would sabotage the focus highlight).
    fn _tree_rows(
        relevant: &BTreeMap<u32, Process>,
        needles: &[String],
        short: bool,
        styled: bool,
    ) -> Vec<(u32, String)> {
        let (pid_w, user_w, age_w, cpu_w, mem_w) = _column_widths(relevant.values());
        let pad = |text: &str, w: usize| {
            format!("{text}{}", " ".repeat(w.saturating_sub(text.chars().count())))
        };
        let fixed = pid_w + user_w + age_w + cpu_w + mem_w + 2 * 5;
        _tree_order(relevant)
            .into_iter()
            .map(|(pid, branch)| {
                let process = &relevant[&pid];
                let room = short.then(|| {
                    table_formatter::terminal_width()
                        .saturating_sub(fixed + branch.chars().count() + CHOOSE_LEAD)
                        .max(MIN_COMMAND)
                });
                let row = format!(
                    "{}  {}  {}  {}  {}  {branch}{}",
                    pad(&_pid_cell(process), pid_w),
                    pad(&process.user, user_w),
                    pad(&_age_label(process.age), age_w),
                    pad(&format!("{:.1}", process.cpu_percent), cpu_w),
                    pad(&_mem_cell(process), mem_w),
                    // The form keeps colours these days, so a match's command can glow there
                    // exactly as it does in the printed report.
                    match styled {
                        true => _command_cell(process, needles, room),
                        false => _command_plain(process, needles, room),
                    },
                );
                (pid, row)
            })
            .collect()
    }

    /// What the form adds in front of every row (focus mark + checkbox), so `--short` inside the
    /// form clips to the width that is actually left.
    const CHOOSE_LEAD: usize = 6;

    /// The header the form shows above the tree, padded with the same widths as the rows.
    fn _plain_header(relevant: &BTreeMap<u32, Process>) -> String {
        _widths_header(_column_widths(relevant.values()))
    }

    #[derive(Args)]
    pub struct MurderArgs {
        /// What to look for: each is a PID, or text appearing in a process's name, command line,
        /// or executable path (case-insensitive). Giving several narrows the hunt — a process
        /// must match EVERY one of them, each in whichever field it lands. Given nothing at all,
        /// the top CPU and memory consumers are offered as a checklist instead
        #[arg(value_name = "PATTERN", num_args = 1..)]
        pub patterns: Vec<String>,
        /// Hunt X11 windows instead of processes: match window titles containing this
        /// (case-insensitive) and CLOSE those windows — the titlebar-✕ request, so the owning
        /// app keeps running along with its other windows. The title carries the active tab's
        /// name, so a browser's background tab is only findable while shown. PATTERNs still
        /// narrow by the owning process (all must hold); needs X (on Wayland, only XWayland
        /// windows are visible)
        #[arg(short = 'w', long, value_name = "TITLE")]
        pub window: Option<String>,
        /// Cut each COMMAND at the window's edge instead of printing it whole — a tidier table
        /// when the arguments run long, at the price of hiding whatever lies past the edge
        /// (matched text included)
        #[arg(short = 's', long)]
        pub short: bool,
        /// Choose interactively which of the matches to act on, instead of all of them: the same
        /// tree, as a form — matches are checkboxes, context rows are fixed. (With no hunt given
        /// at all, murder is a picker already, so this adds nothing there)
        #[arg(short = 'c', long)]
        pub choose: bool,
    }

    /// Why a process is in the report at all.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Relation {
        /// The pattern found it — the row the run is about.
        Match,
        /// Below a match: not searched for, but killing its parent takes it too.
        Descendant,
        /// Above a match: context only — who owns what would die. Never touched.
        Ancestor,
    }

    /// A running process, as much of it as `/proc` will show us.
    struct Process {
        pid: u32,
        ppid: u32,
        user: String,
        /// The executable's name (`/proc/<pid>/comm`), which is all a kernel thread has.
        name: String,
        /// The full command line, arguments and all — empty for a kernel thread.
        cmdline: String,
        /// Where the binary lives. Absent for a kernel thread, and for anyone else's process
        /// when we're not privileged enough to read the link.
        exe: Option<String>,
        /// Seconds since it started.
        age: u64,
        /// Which fields the pattern was found in — what the highlighting has to make visible.
        matched: Vec<&'static str>,
        /// Lifetime-average CPU share, `ps`-style: total tick time over seconds alive.
        cpu_percent: f64,
        /// Resident memory (`VmRSS`), in KiB — none for a kernel thread.
        rss_kib: Option<u64>,
        /// The matched window title, when the hunt included `--window` and one of this process's
        /// windows carried it.
        window: Option<String>,
        /// This command's own ancestry: the shell that ran it, its terminal, and so on up. Shown
        /// when relevant, never killed.
        protected: bool,
        relation: Relation,
    }

    /// Scan `/proc` once, find every match, and pull in each match's whole lineage: ancestors up
    /// to the root for ownership, descendants all the way down for blast radius. Returns the
    /// relevant processes keyed by PID — one entry each, however many matches share a lineage —
    /// and how many of them are actual matches.
    fn _survey(
        patterns: &[String],
        by_window: Option<&BTreeMap<u32, Vec<(u32, String)>>>,
    ) -> (BTreeMap<u32, Process>, usize) {
        let mut all = _snapshot();
        let matched: Vec<u32> = all
            .values_mut()
            .filter_map(|process| {
                _mark_matches(process, patterns, by_window);
                (!process.matched.is_empty()).then_some(process.pid)
            })
            .collect();
        (_relate(all, &matched), matched.len())
    }

    /// No hunt given — show where the machine is going instead: the heaviest CPU users and the
    /// heaviest memory users, each list capped at [`TOP_HOGS`] (shorter when the system simply
    /// doesn't have that many), as a checklist.
    ///
    /// A process can easily top both lists; it appears ONCE, in the CPU group, and the memory
    /// group is explicitly "beyond those" — two checkboxes for one process would make "checked
    /// here, unchecked there" a riddle, and the form library rightly refuses duplicate labels.
    ///
    /// The ticked processes go down the same ladder every other murder path uses — the form is
    /// the confirmation, exactly as it is for `--choose` (asking again after a deliberate
    /// tick-and-Submit would be redundant at best).
    fn _pick_hogs() {
        let all = _snapshot();
        let (by_cpu, by_memory) = _hog_shortlists(&all);
        if by_cpu.is_empty() && by_memory.is_empty() {
            eprintln!("murder: nothing offerable is running (kernel threads and this shell's own ancestry are never offered)");
            return;
        }
        // The same columns every other murder view uses, sized over everything offered — the
        // two lists overlap, so shared widths keep a twin's two rows byte-identical (mirroring
        // links by exact wording).
        let widths = _column_widths(by_cpu.iter().chain(by_memory.iter()).copied());
        let mut form = terminal_choice::Form::new()
            .title("murder — pick what would die")
            .comment("Tick what to terminate — Submit is the trigger; Esc leaves everything running.")
            .comment(_widths_header(widths))
            .aligned()
            // A heavy process tops both charts under the SAME wording; mirroring makes its two
            // boxes one box in two places, so it cannot be doomed under one heading and spared
            // under the other.
            .mirror_duplicates();
        let cpu_options: Vec<String> = by_cpu.iter().map(|p| _hog_row(p, widths)).collect();
        let memory_options: Vec<String> = by_memory.iter().map(|p| _hog_row(p, widths)).collect();
        if !cpu_options.is_empty() {
            let refs: Vec<&str> = cpu_options.iter().map(String::as_str).collect();
            form = form.checkboxes(CPU_GROUP, &refs);
        }
        if !memory_options.is_empty() {
            let refs: Vec<&str> = memory_options.iter().map(String::as_str).collect();
            form = form.checkboxes(MEMORY_GROUP, &refs);
        }
        match terminal_choice::run(&mut form) {
            Err(err) => eprintln!("murder: {err}"),
            Ok(terminal_choice::Outcome::Cancelled) => eprintln!("murder: aborted — nothing chosen"),
            Ok(terminal_choice::Outcome::Submitted) => {
                // A mirrored twin reports from both groups; the set collapses it to one kill.
                let picked: std::collections::BTreeSet<&str> = form
                    .checked(CPU_GROUP)
                    .into_iter()
                    .chain(form.checked(MEMORY_GROUP))
                    .collect();
                if picked.is_empty() {
                    eprintln!("murder: nothing ticked — nothing to signal");
                    return;
                }
                let chosen: Vec<u32> = picked.iter().filter_map(|option| _picked_pid(option)).collect();
                _kill_confirmed(&_relate(all, &chosen));
            }
        }
    }

    /// How many of each kind of hog the picker offers — "up to": a quiet system offers fewer.
    const TOP_HOGS: usize = 7;
    const CPU_GROUP: &str = "Top CPU consumers";
    const MEMORY_GROUP: &str = "Top memory consumers";

    /// The two shortlists, offerable processes only — nothing [`_triage`] would refuse anyway
    /// (this shell's ancestry, PID 1, kernel threads), so no box can be ticked in vain.
    ///
    /// Each list is honestly its own top: a process leading both charts appears in BOTH, worded
    /// identically — and the form runs with `mirror_duplicates`, so its two boxes are one box in
    /// two places. Ticking it under either heading ticks it under the other.
    fn _hog_shortlists(all: &BTreeMap<u32, Process>) -> (Vec<&Process>, Vec<&Process>) {
        let mut offerable: Vec<&Process> = all
            .values()
            .filter(|p| !p.protected && p.pid != 1 && !p.cmdline.is_empty())
            .collect();
        offerable.sort_by(|a, b| {
            b.cpu_percent.total_cmp(&a.cpu_percent).then_with(|| a.pid.cmp(&b.pid))
        });
        let by_cpu: Vec<&Process> = offerable.iter().copied().take(TOP_HOGS).collect();
        offerable.sort_by(|a, b| {
            b.rss_kib.unwrap_or(0).cmp(&a.rss_kib.unwrap_or(0)).then_with(|| a.pid.cmp(&b.pid))
        });
        let by_memory: Vec<&Process> = offerable.iter().copied().take(TOP_HOGS).collect();
        (by_cpu, by_memory)
    }

    /// One checkbox line, in the standard murder columns (PID leads — it is what
    /// [`_picked_pid`] reads back). The full command, not just the name: the columns are shared
    /// with every other view, and the command is what a human recognises.
    fn _hog_row(process: &Process, widths: (usize, usize, usize, usize, usize)) -> String {
        let (pid_w, user_w, age_w, cpu_w, mem_w) = widths;
        let pad = |text: &str, w: usize| {
            format!("{text}{}", " ".repeat(w.saturating_sub(text.chars().count())))
        };
        format!(
            "{}  {}  {}  {}  {}  {}",
            pad(&_pid_cell(process), pid_w),
            pad(&process.user, user_w),
            pad(&_age_label(process.age), age_w),
            pad(&format!("{:.1}", process.cpu_percent), cpu_w),
            pad(&_mem_cell(process), mem_w),
            _command_plain(process, &[], None),
        )
    }

    /// The `PID USER AGE %CPU MEM COMMAND` header for a given set of widths — the picker's
    /// version of [`_plain_header`], which sizes over a relevant-map instead.
    fn _widths_header(widths: (usize, usize, usize, usize, usize)) -> String {
        let (pid_w, user_w, age_w, cpu_w, mem_w) = widths;
        let pad = |text: &str, w: usize| {
            format!("{text}{}", " ".repeat(w.saturating_sub(text.chars().count())))
        };
        format!(
            "{}  {}  {}  {}  {}  COMMAND",
            pad("PID", pid_w),
            pad("USER", user_w),
            pad("AGE", age_w),
            pad("%CPU", cpu_w),
            pad("MEM", mem_w)
        )
    }

    /// The pid (or window id) back out of a ticked option line — always the first token, with
    /// the ancestry star tolerated ([`_pid_cell`] appends it) and colour codes stripped first:
    /// the `--choose` options carry the grep-style glow, and a pid matched BY pid glows in this
    /// very token.
    fn _picked_pid(option: &str) -> Option<u32> {
        let plain = console::strip_ansi_codes(option).into_owned();
        plain.split_whitespace().next()?.trim_end_matches('*').parse().ok()
    }

    /// Everything running right now, read once — the base of both the pattern hunt and the
    /// no-pattern picker.
    fn _snapshot() -> BTreeMap<u32, Process> {
        let users = _users();
        let boot: f64 = _uptime_seconds();
        let ticks = _clock_ticks();
        let self_pid = std::process::id();
        let ancestry = _ancestry(self_pid);
        _pids()
            .into_iter()
            // Never match the search itself: `murder firefox` carries "firefox" in its own command
            // line, so it would find — and offer to kill — the very command doing the asking.
            .filter(|pid| *pid != self_pid)
            .filter_map(|pid| _read(pid, &users, boot, ticks, &ancestry))
            .map(|process| (process.pid, process))
            .collect()
    }

    /// Reduce the full process map to the relevant lineage of `matched`, labelling each survivor.
    ///
    /// Precedence when a process qualifies twice: a match stays a match, and a process sitting
    /// *between* two matches (ancestor of one, descendant of the other) counts as a descendant —
    /// killing the match above it takes it down, so "context only" would be a lie.
    fn _relate(mut all: BTreeMap<u32, Process>, matched: &[u32]) -> BTreeMap<u32, Process> {
        let mut relation: BTreeMap<u32, Relation> = matched.iter().map(|pid| (*pid, Relation::Match)).collect();
        // Ancestors: walk each match's parent chain to the top. Stop at an already-labelled
        // process — its own chain upward has been walked before.
        for pid in matched {
            let mut current = *pid;
            while let Some(parent) = all.get(&current).map(|p| p.ppid).filter(|p| *p > 0) {
                if !all.contains_key(&parent) || relation.contains_key(&parent) {
                    break;
                }
                relation.insert(parent, Relation::Ancestor);
                current = parent;
            }
        }
        // Descendants: breadth-first below every match.
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for process in all.values() {
            children.entry(process.ppid).or_default().push(process.pid);
        }
        let mut queue: Vec<u32> = matched.to_vec();
        while let Some(pid) = queue.pop() {
            for child in children.get(&pid).into_iter().flatten() {
                match relation.get(child) {
                    Some(Relation::Match | Relation::Descendant) => continue, // already walked
                    _ => {
                        relation.insert(*child, Relation::Descendant);
                        queue.push(*child);
                    }
                }
            }
        }
        let mut relevant = BTreeMap::new();
        for (pid, label) in relation {
            if let Some(mut process) = all.remove(&pid) {
                process.relation = label;
                relevant.insert(pid, process);
            }
        }
        relevant
    }

    // ——— Rendering ————————————————————————————————————————————————————————

    /// The report: aligned PID/USER/AGE columns, then each process at its place in the tree —
    /// `ps f` style, the branch drawing sharing the command column so the fixed columns stay
    /// aligned however deep the tree goes.
    ///
    /// Matches carry the highlighting; ancestors are dimmed whole, being context rather than
    /// consequence. Every process appears exactly once: two matches in one lineage share one tree,
    /// and the shared ancestry prints a single time. The command text is complete — never
    /// truncated to the window — because the tail of a long command line (a config path, a
    /// `--profile` name) is often the part that tells two siblings apart.
    fn _render(relevant: &BTreeMap<u32, Process>, needles: &[String], short: bool) -> Vec<String> {
        // A match found BY its PID glows there — the command text has nothing to show for it.
        let pid_shown = |process: &Process| {
            let plain = _pid_cell(process);
            match process.relation == Relation::Match && process.matched.contains(&"pid") {
                true => plain.replace(
                    &process.pid.to_string(),
                    &doc_style::_scoped(&doc_style::escape("30;41"), &process.pid.to_string()),
                ),
                false => plain,
            }
        };
        let (pid_w, user_w, age_w, cpu_w, mem_w) = _column_widths(relevant.values());
        let pad = |text: &str, w: usize| format!("{text}{}", " ".repeat(w.saturating_sub(text.chars().count())));
        let mut lines = vec![_header(&format!(
            "{}  {}  {}  {}  {}  COMMAND",
            pad("PID", pid_w),
            pad("USER", user_w),
            pad("AGE", age_w),
            pad("%CPU", cpu_w),
            pad("MEM", mem_w)
        ))];
        // With `--short`, whatever the fixed columns and the branch drawing leave of the window
        // is all the command gets — computed per row, since deeper branches leave less.
        let fixed = pid_w + user_w + age_w + cpu_w + mem_w + 2 * 5;
        let room = |branch: &str| {
            short.then(|| {
                table_formatter::terminal_width()
                    .saturating_sub(fixed + branch.chars().count())
                    .max(MIN_COMMAND)
            })
        };

        for (pid, branch) in _tree_order(relevant) {
            let process = &relevant[&pid];
            // Padding is measured on the plain cell; the glow adds no visible width.
            let pid_pad = " ".repeat(pid_w.saturating_sub(_pid_cell(process).chars().count()));
            let row = format!(
                "{}{pid_pad}  {}  {}  {}  {}  {branch}{}",
                pid_shown(process),
                pad(&process.user, user_w),
                pad(&_age_label(process.age), age_w),
                pad(&format!("{:.1}", process.cpu_percent), cpu_w),
                pad(&_mem_cell(process), mem_w),
                _command_cell(process, needles, room(&branch)),
            );
            lines.push(match process.relation {
                // Context is dimmed whole: it explains ownership, and nothing about it changes.
                Relation::Ancestor => doc_style::_scoped(&doc_style::_wrap(&[&Weight::Dark]), &row),
                _ => row,
            });
        }
        if relevant.values().any(|process| process.protected) {
            lines.push(notice("* this shell's own ancestry — murder will refuse to signal these"));
        }
        lines
    }

    /// The PID column's text: the pid, starred when it is this shell's own ancestry.
    fn _pid_cell(process: &Process) -> String {
        format!("{}{}", process.pid, if process.protected { "*" } else { "" })
    }

    /// The MEM column's text — `-` for a kernel thread, which owns no user memory.
    fn _mem_cell(process: &Process) -> String {
        process.rss_kib.map(_mem_label).unwrap_or_else(|| "-".to_string())
    }

    /// Column widths over every relevant row — shared by the styled report, the `--choose`
    /// form's rows and its header, so all three always agree.
    fn _column_widths<'a>(
        rows: impl Iterator<Item = &'a Process> + Clone,
    ) -> (usize, usize, usize, usize, usize) {
        let width = |pick: &dyn Fn(&Process) -> String, floor: usize| {
            rows.clone().map(|p| pick(p).chars().count()).max().unwrap_or(0).max(floor)
        };
        (
            width(&|p| _pid_cell(p), 3),
            width(&|p: &Process| p.user.clone(), 4),
            width(&|p| _age_label(p.age), 3),
            width(&|p| format!("{:.1}", p.cpu_percent), 4),
            width(&|p| _mem_cell(p), 3),
        )
    }

    /// Every relevant process in display order with its branch drawing — the ONE tree walk,
    /// consumed by the styled report and the `--choose` form alike, so the two views can never
    /// disagree about shape.
    ///
    /// Parent → children restricted to the relevant set; roots are those whose parent isn't in
    /// it; children in PID order, which roughly reads as creation order. Depth-first with an
    /// explicit stack: (pid, branch prefix for this row, prefix for its kids).
    fn _tree_order(relevant: &BTreeMap<u32, Process>) -> Vec<(u32, String)> {
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        let mut roots: Vec<u32> = Vec::new();
        for process in relevant.values() {
            match relevant.contains_key(&process.ppid) {
                true => children.entry(process.ppid).or_default().push(process.pid),
                false => roots.push(process.pid),
            }
        }
        let mut ordered = Vec::with_capacity(relevant.len());
        let mut stack: Vec<(u32, String, String)> = Vec::new();
        for root in roots.iter().rev() {
            stack.push((*root, String::new(), String::new()));
        }
        while let Some((pid, branch, descend)) = stack.pop() {
            let kids = children.get(&pid).cloned().unwrap_or_default();
            let last = kids.len().saturating_sub(1);
            for (nth, child) in kids.into_iter().enumerate().rev() {
                let (twig, carry) = if nth == last { ("└─ ", "   ") } else { ("├─ ", "│  ") };
                stack.push((child, format!("{descend}{twig}"), format!("{descend}{carry}")));
            }
            ordered.push((pid, branch));
        }
        ordered
    }

    /// One process's command text: the full command line (or `[name]` for a kernel thread), with
    /// every occurrence of the pattern glowing the way `g`'s matches do.
    ///
    /// When the match lives somewhere the command line doesn't show — a `comm` name differing
    /// from argv, or an executable reached through `/proc/self/exe` — the matched field is
    /// appended, so no row ever leaves the reader guessing why it qualified.
    /// The whole cell is composed PLAIN first — label, then any hidden-field appendix — then
    /// clipped when `room` says so, then highlighted in one pass. That order is load-bearing:
    /// clipping coloured text would count escape bytes as width, and highlighting before the
    /// appendix would let a later needle match inside the inserted colour codes. The plain half
    /// stands alone as [`_command_plain`] because the `--choose` form needs exactly it: escape
    /// codes inside a form option would sabotage the focus highlight.
    fn _command_cell(process: &Process, needles: &[String], room: Option<usize>) -> String {
        let plain = _command_plain(process, needles, room);
        match process.relation {
            Relation::Match => _highlight(&plain, needles),
            _ => plain, // context rows don't glow: nothing in them "matched"
        }
    }

    fn _command_plain(process: &Process, needles: &[String], room: Option<usize>) -> String {
        let mut plain = _command_label(process);
        if process.relation == Relation::Match {
            // A needle the command text doesn't show landed elsewhere — append that field, so
            // the row explains itself even when only part of the conjunction is visible.
            if needles.iter().any(|needle| !needle.is_empty() && _find_ci(&plain, needle).is_none()) {
                let mut hidden: Vec<String> = Vec::new();
                if process.matched.contains(&"name") {
                    hidden.push(format!("name: {}", process.name));
                }
                if process.matched.contains(&"exe") {
                    if let Some(exe) = &process.exe {
                        hidden.push(format!("exe: {exe}"));
                    }
                }
                if process.matched.contains(&"window") {
                    if let Some(title) = &process.window {
                        hidden.push(format!("window: {title}"));
                    }
                }
                if !hidden.is_empty() {
                    plain.push_str(&format!("  ({})", hidden.join(", ")));
                }
            }
        }
        if let Some(room) = room {
            plain = _clip(&plain, room);
        }
        plain
    }

    /// Cut `text` to `room` columns, marking that something was removed. Counts characters, not
    /// bytes — an argument holding a path with accents must not be cut mid-character.
    fn _clip(text: &str, room: usize) -> String {
        match text.chars().count() > room {
            true => text.chars().take(room.saturating_sub(1)).collect::<String>() + "…",
            false => text.to_string(),
        }
    }

    /// Never squeeze a clipped command below this, however narrow the window — a row with no
    /// command in it tells the reader nothing at all.
    const MIN_COMMAND: usize = 20;

    /// Resident memory for eyes: KB as counted, one decimal from MB up.
    fn _mem_label(kib: u64) -> String {
        const MIB: f64 = 1024.0;
        match kib {
            0..=1023 => format!("{kib}KB"),
            _ => {
                let mib = kib as f64 / MIB;
                if mib < MIB { format!("{mib:.1}MB") } else { format!("{:.1}GB", mib / MIB) }
            }
        }
    }

    /// Wrap every case-insensitive occurrence of any needle in the match colours `g` and `gg`
    /// use — black on a red block. Pure text in, coloured text out.
    ///
    /// All the needles are located on the plain text FIRST and their spans merged, then the
    /// colours go in as one pass. Highlighting one needle and then searching the result would
    /// let a later needle match inside the inserted escape codes — a needle of `30` would find
    /// the `30` in `\x1b[30;41m` itself — or match text a code now interrupts.
    fn _highlight(text: &str, needles: &[String]) -> String {
        let mut spans: Vec<(usize, usize)> =
            needles.iter().flat_map(|needle| _all_ci(text, needle)).collect();
        spans.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in spans.into_iter().map(|(start, len)| (start, start + len)) {
            match merged.last_mut() {
                Some((_, tail)) if start <= *tail => *tail = (*tail).max(end),
                _ => merged.push((start, end)),
            }
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        for (start, end) in merged {
            out.push_str(&text[cursor..start]);
            out.push_str(&doc_style::_scoped(&doc_style::escape("30;41"), &text[start..end]));
            cursor = end;
        }
        out.push_str(&text[cursor..]);
        out
    }

    /// Every case-insensitive occurrence of `needle` in `text`, as byte `(start, length)` spans.
    fn _all_ci(text: &str, needle: &str) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut offset = 0;
        while let Some((start, len)) = _find_ci(&text[offset..], needle) {
            spans.push((offset + start, len));
            offset += start + len;
        }
        spans
    }

    /// The first case-insensitive occurrence of `needle` in `haystack`, as `(byte offset, byte
    /// length)` — the length measured in the haystack, whose spelling is what gets highlighted.
    ///
    /// Compared character by character through each side's `to_lowercase()`, rather than by
    /// lowercasing the whole haystack and searching that: case-folding can change a string's
    /// byte length (ẞ→ß doesn't round-trip, İ grows), so offsets found in a folded copy don't
    /// necessarily exist in the original.
    fn _find_ci(haystack: &str, needle: &str) -> Option<(usize, usize)> {
        if needle.is_empty() {
            return None;
        }
        for (start, _) in haystack.char_indices() {
            if let Some(len) = _starts_ci(&haystack[start..], needle) {
                return Some((start, len));
            }
        }
        None
    }

    /// If `haystack` begins with `needle` case-insensitively, how many of its bytes that takes.
    ///
    /// The needle side is one peekable stream of folded characters, consumed only when there is a
    /// haystack character to weigh it against — an earlier draft pulled from both sides in one
    /// tuple match, which silently ate a needle character every time a haystack character's fold
    /// ran out, and matched nothing at all.
    fn _starts_ci(haystack: &str, needle: &str) -> Option<usize> {
        let mut want = needle.chars().flat_map(char::to_lowercase).peekable();
        let mut taken = 0;
        for ch in haystack.chars() {
            if want.peek().is_none() {
                return Some(taken);
            }
            for folded in ch.to_lowercase() {
                match want.next() {
                    Some(expected) if expected == folded => {}
                    // A mismatch — or the needle ending in the *middle* of this character's
                    // fold (ß folds to "ss"; a needle ending after one `s` cannot highlight
                    // half a ß), which rejects the match rather than splitting a character.
                    _ => return None,
                }
            }
            taken += ch.len_utf8();
        }
        want.peek().is_none().then_some(taken)
    }

    // ——— Matching ——————————————————————————————————————————————————————————

    /// Record every field the patterns appear in — or nothing at all unless EVERY pattern lands
    /// somewhere. The patterns are a conjunction: each may hit a different field (`murder chrome
    /// 1043` is the chrome that owns that PID), but one missing everywhere disqualifies the
    /// process, which is what makes extra patterns *narrow* the hunt.
    ///
    /// A PID is matched whole rather than as a substring: searching `42` should not drag in 1042
    /// and 4200, which have nothing to do with it. Everything else is a case-insensitive
    /// substring, since a fragment of a path or an argument is the normal way to recognise a
    /// process you half-remember.
    fn _mark_matches(
        process: &mut Process,
        patterns: &[String],
        by_window: Option<&BTreeMap<u32, Vec<(u32, String)>>>,
    ) {
        // The window filter is one more conjunct: with `--window`, owning a matched window is
        // required — and sufficient by itself when no text patterns were given.
        if let Some(owned) = by_window {
            match owned.get(&process.pid) {
                Some(windows) => {
                    let titles: Vec<&str> =
                        windows.iter().map(|(_, title)| title.as_str()).collect();
                    process.window = Some(titles.join("; "));
                    process.matched.push("window");
                }
                None => return,
            }
        }
        let mut fields: Vec<&'static str> = Vec::new();
        for pattern in patterns {
            let before = fields.len();
            if pattern.parse::<u32>() == Ok(process.pid) {
                fields.push("pid");
            }
            if _find_ci(&process.name, pattern).is_some() {
                fields.push("name");
            }
            // An ancestor's command line necessarily contains this invocation, patterns and all,
            // so matching on it would report the calling shell for *every* search. Its name, PID
            // and executable are still fair game — those match for real reasons.
            if !process.protected && _find_ci(&process.cmdline, pattern).is_some() {
                fields.push("cmdline");
            }
            if process.exe.as_deref().is_some_and(|exe| _find_ci(exe, pattern).is_some()) {
                fields.push("exe");
            }
            if fields.len() == before {
                // This pattern found nothing — the conjunction fails. Clear the window mark too:
                // half a match is not a match.
                process.matched.clear();
                return;
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        process.matched.extend(fields);
        process.matched = std::mem::take(&mut process.matched)
            .into_iter()
            .filter(|field| seen.insert(*field))
            .collect();
    }

    // ——— Reading /proc ————————————————————————————————————————————————————

    /// Everything under `/proc` whose name is a number — one per running process.
    fn _pids() -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.parse().ok()))
            .collect()
    }

    /// Every process between this one and the top: the shell that ran it, that shell's terminal,
    /// and so on. Walked through `/proc/<pid>/stat`'s parent field, and bounded so a corrupt or
    /// recycled parent can't spin here forever.
    fn _ancestry(from: u32) -> std::collections::BTreeSet<u32> {
        let mut chain = std::collections::BTreeSet::new();
        let mut pid = from;
        for _ in 0..MAX_ANCESTRY {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { break };
            let Some(parent) = _ppid(&stat).filter(|parent| *parent > 0) else { break };
            if !chain.insert(parent) {
                break; // already seen — a cycle, which shouldn't happen but shouldn't hang either
            }
            pid = parent;
        }
        chain
    }

    /// Depth limit for the parent walk. Real ancestry is a handful deep; this only exists so a
    /// malformed `/proc` cannot turn the walk into a loop.
    const MAX_ANCESTRY: usize = 64;

    /// Assemble one process, or `None` if it exited while we were reading it — which is normal,
    /// not an error: `/proc` is a live view, and a scan of it is always slightly out of date.
    fn _read(
        pid: u32,
        users: &BTreeMap<u32, String>,
        boot: f64,
        ticks: u64,
        ancestry: &std::collections::BTreeSet<u32>,
    ) -> Option<Process> {
        let dir = PathBuf::from("/proc").join(pid.to_string());
        let name = std::fs::read_to_string(dir.join("comm")).ok()?.trim_end().to_string();
        let raw = std::fs::read(dir.join("cmdline")).ok()?;
        let stat = std::fs::read_to_string(dir.join("stat")).ok()?;
        let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
        let uid = _real_uid(&status).unwrap_or(u32::MAX);
        let started = _start_ticks(&stat).unwrap_or(0) as f64 / ticks.max(1) as f64;
        let age_exact = (boot - started).max(0.0);
        Some(Process {
            pid,
            ppid: _ppid(&stat).unwrap_or(0),
            user: users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
            name,
            cmdline: _cmdline(&raw),
            // Unreadable is the common case for another user's process, and says nothing
            // interesting — treat it the same as a kernel thread having none.
            exe: std::fs::read_link(dir.join("exe")).ok().map(|p| p.to_string_lossy().into_owned()),
            age: age_exact as u64,
            matched: Vec::new(),
            // `ps`-style lifetime average: all the CPU it ever used over all the time it existed.
            // A steady 100%-burner reads 100; an idle daemon that once worked hard fades slowly.
            cpu_percent: _cpu_ticks(&stat).unwrap_or(0) as f64
                / ticks.max(1) as f64
                / age_exact.max(0.5)
                * 100.0,
            rss_kib: _rss_kib(&status),
            window: None,
            protected: ancestry.contains(&pid),
            relation: Relation::Ancestor,
        })
    }

    /// Fields 14+15 of `/proc/<pid>/stat` — user plus kernel CPU time, in clock ticks — read past
    /// the parenthesised name for the same reason [`_start_ticks`] is.
    fn _cpu_ticks(stat: &str) -> Option<u64> {
        let tail = &stat[stat.rfind(')')? + 1..];
        let mut fields = tail.split_whitespace();
        let utime: u64 = fields.nth(11)?.parse().ok()?;
        let stime: u64 = fields.next()?.parse().ok()?;
        Some(utime + stime)
    }

    /// `VmRSS:` out of `/proc/<pid>/status`, in KiB — absent for a kernel thread, which owns no
    /// user memory at all.
    fn _rss_kib(status: &str) -> Option<u64> {
        status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    /// `/proc/<pid>/cmdline` is NUL-separated and usually NUL-terminated; joining on spaces gives
    /// back something readable. A kernel thread has none at all, and is left empty so the report
    /// can show its `[name]` instead of a blank column.
    ///
    /// Control characters become `?`, the `ps` convention: an argument can legally hold newlines
    /// (which would break the tree apart) or even ANSI escapes (which could forge rows or hide
    /// the highlighting), and neither gets to.
    fn _cmdline(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw)
            .split('\0')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .map(|ch| if ch.is_control() { '?' } else { ch })
            .collect()
    }

    /// The real UID out of `/proc/<pid>/status`, whose `Uid:` line is real, effective, saved and
    /// filesystem — the first is the one that answers "whose process is this".
    fn _real_uid(status: &str) -> Option<u32> {
        status
            .lines()
            .find_map(|line| line.strip_prefix("Uid:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    }

    /// Field 22 of `/proc/<pid>/stat`: when the process started, in clock ticks since boot.
    ///
    /// Parsed from the last `)` rather than by splitting the whole line, because field 2 is the
    /// executable name in parentheses and it may contain both spaces and parentheses of its own —
    /// a process helpfully named `foo (bar) baz` would otherwise shift every later field along.
    /// After that closing paren the fields are fixed-width tokens, and field 22 is the 20th.
    fn _start_ticks(stat: &str) -> Option<u64> {
        let tail = &stat[stat.rfind(')')? + 1..];
        tail.split_whitespace().nth(19)?.parse().ok()
    }

    /// Field 4 of `/proc/<pid>/stat`: the parent's PID. Read past the parenthesised name for the
    /// same reason [`_start_ticks`] does.
    fn _ppid(stat: &str) -> Option<u32> {
        let tail = &stat[stat.rfind(')')? + 1..];
        tail.split_whitespace().nth(1)?.parse().ok()
    }

    /// Seconds since boot, from `/proc/uptime` — the clock every process's age is measured
    /// against. Kept fractional: the display rounds ages to seconds anyway, but the CPU average
    /// divides by this, and truncating first inflates a young process's share (a burner alive
    /// 1.9s divided by a truncated 1 reads as nearly double — it showed 120% on one core).
    fn _uptime_seconds() -> f64 {
        std::fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok())
            .unwrap_or(0.0)
    }

    /// Clock ticks per second, which is what `/proc/<pid>/stat` counts start times in. It is 100
    /// on essentially every Linux, but it is a configured kernel value and asking is free.
    fn _clock_ticks() -> u64 {
        // SAFETY: `sysconf` reads a static configuration value and touches nothing of ours.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks > 0 { ticks as u64 } else { 100 }
    }

    /// UID to username, from `/etc/passwd`. Read once for the whole scan rather than per process,
    /// and missing simply means the number is shown instead.
    fn _users() -> BTreeMap<u32, String> {
        let Ok(text) = std::fs::read_to_string("/etc/passwd") else { return BTreeMap::new() };
        text.lines().filter_map(_passwd_entry).collect()
    }

    /// Pure: one `/etc/passwd` line as `(uid, name)`. Fields are `name:password:uid:gid:…`.
    fn _passwd_entry(line: &str) -> Option<(u32, String)> {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let uid: u32 = fields.nth(1)?.parse().ok()?;
        (!name.is_empty()).then(|| (uid, name.to_string()))
    }

    /// A duration a person can read at a glance: the two largest units that apply, so `3d4h`
    /// rather than `275400s`, and never more precision than the question deserves.
    fn _age_label(seconds: u64) -> String {
        let (days, hours) = (seconds / 86_400, (seconds % 86_400) / 3_600);
        let (minutes, secs) = ((seconds % 3_600) / 60, seconds % 60);
        match (days, hours, minutes) {
            (0, 0, 0) => format!("{secs}s"),
            (0, 0, _) => format!("{minutes}m{secs}s"),
            (0, _, _) => format!("{hours}h{minutes}m"),
            _ => format!("{days}d{hours}h"),
        }
    }

    /// What a process is called in the report: its command line, or its name in brackets when it
    /// has none — the `[kthreadd]` convention `ps` uses for kernel threads, which own no binary.
    fn _command_label(process: &Process) -> String {
        match process.cmdline.is_empty() {
            true => format!("[{}]", process.name),
            false => process.cmdline.clone(),
        }
    }

    /// The termination itself, gated the way the plan always said it would be.
    ///
    /// **Confirm, always** — [`_prompt_yN`], so a reflexive Enter leaves everything running, and
    /// only when stdin is a terminal: piped in, there is nobody to ask, and a killer must never
    /// let a pipe answer for a person. The tree above is what the question refers to: matches get
    /// the signal, their descendants are the acknowledged blast radius, dimmed ancestors are
    /// bystanders.
    ///
    /// **Signals in order.** `SIGTERM` to every target, a grace period for cleanup, then
    /// `SIGKILL` only for whatever is still standing — and those are named, because ignoring
    /// SIGTERM is a fact about a program worth knowing.
    ///
    /// **Refusals are printed, not silent** ([`_triage`]): PID 1, the starred ancestry, kernel
    /// threads. A row that appeared in the tree and then wasn't signalled should say why.
    fn _kill(relevant: &BTreeMap<u32, Process>) {
        _kill_gated(relevant, true)
    }

    /// The ladder without the `[y/N]` — for a target set the user JUST confirmed through the
    /// `--choose` form. Triage and its explanations still run; only the second question goes.
    fn _kill_confirmed(relevant: &BTreeMap<u32, Process>) {
        _kill_gated(relevant, false)
    }

    fn _kill_gated(relevant: &BTreeMap<u32, Process>, ask: bool) {
        use std::io::IsTerminal;
        let targets: Vec<&Process> =
            relevant.values().filter(|process| process.relation == Relation::Match).collect();
        let (killable, refused) = _triage(&targets);
        for (process, why) in &refused {
            eprintln!("{}", notice(&format!("murder: sparing PID '{}' — {why}", process.pid)));
        }
        if killable.is_empty() {
            eprintln!("murder: nothing left to signal");
            return;
        }
        if !std::io::stdin().is_terminal() {
            eprintln!("murder: refusing to kill non-interactively — run it from a terminal");
            return;
        }
        let heirs =
            relevant.values().filter(|process| process.relation == Relation::Descendant).count();
        let blast = match heirs {
            0 => String::new(),
            1 => " (the 1 descendant beneath them goes too)".to_string(),
            n => format!(" (the {n} descendants beneath them go too)"),
        };
        // Naming the tree count tells apart "one family" from "unrelated strays": four processes
        // across one tree is a service and its workers; across four, four separate victims.
        let span = match killable.len() {
            0 | 1 => String::new(),
            _ => format!(" across {}", _count(_tree_count(relevant, &killable), "process-tree", "process-trees")),
        };
        if ask {
            let question =
                format!("Terminate {}{span}{blast}?", _count(killable.len(), "process", "processes"));
            if !prompt_yN(&question) {
                eprintln!("murder: aborted — nothing signalled");
                return;
            }
        } else {
            // Form-confirmed: restate what was agreed to, since the form scrolled away.
            eprintln!(
                "murder: as chosen — {}{span}{blast}",
                _count(killable.len(), "process", "processes")
            );
        }
        let mut waiting: Vec<u32> = Vec::new();
        for process in &killable {
            match _signal(process.pid, libc::SIGTERM) {
                Ok(()) => waiting.push(process.pid),
                Err(err) => eprintln!(
                    "{}",
                    problematic(&format!("murder: {} refused the signal: {err}{}", process.pid,
                        if err.kind() == std::io::ErrorKind::PermissionDenied { " (not yours — sudo?)" } else { "" }))
                ),
            }
        }
        let asked = waiting.len();
        // The grace period: SIGTERM is a request, and cleanup takes a moment. Poll rather than
        // sleep the whole allowance, so a prompt exit ends the wait early.
        for _ in 0..GRACE_POLLS {
            waiting.retain(|pid| _alive(*pid));
            if waiting.is_empty() {
                break;
            }
            std::thread::sleep(GRACE_POLL);
        }
        waiting.retain(|pid| _alive(*pid));
        for pid in &waiting {
            let _ = _signal(*pid, libc::SIGKILL);
        }
        match waiting.as_slice() {
            [] => eprintln!("murder: {} terminated", _count(asked, "process", "processes")),
            stubborn => eprintln!(
                "{}",
                notice(&format!(
                    "murder: {} went on SIGTERM; {} ignored it and got SIGKILL: {}",
                    asked - stubborn.len(),
                    stubborn.len(),
                    stubborn.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                ))
            ),
        }
    }

    /// `3 processes`, `1 process` — a count with its noun properly declined, because "process(es)"
    /// is the writer dodging a job the caller can do in one branch.
    fn _count(n: usize, one: &str, many: &str) -> String {
        format!("{n} {}", if n == 1 { one } else { many })
    }

    /// How many independent subtrees the kill set spans: each killable match with no killable
    /// match anywhere above it roots one. Two workers under a matched service are one tree; two
    /// unrelated daemons are two.
    fn _tree_count(relevant: &BTreeMap<u32, Process>, killable: &[&Process]) -> usize {
        let set: std::collections::BTreeSet<u32> = killable.iter().map(|p| p.pid).collect();
        killable
            .iter()
            .filter(|process| {
                let mut current = process.pid;
                // Bounded by the map's size: ppid chains in a live snapshot terminate, and a
                // corrupt one must not turn this into a loop.
                for _ in 0..=relevant.len() {
                    let Some(parent) = relevant.get(&current).map(|p| p.ppid) else { break };
                    if set.contains(&parent) {
                        return false; // a killable ancestor claims this one for its own tree
                    }
                    if !relevant.contains_key(&parent) {
                        break;
                    }
                    current = parent;
                }
                true
            })
            .count()
    }

    /// Close the matched windows — [`_kill`]'s sibling for the window hunt, same gates, milder
    /// verb. Only windows of processes that survived the whole conjunction close (a text pattern
    /// may have excluded an owner), and this shell's own ancestry is spared here too: closing
    /// the terminal this runs in is still self-harm, just politer.
    ///
    /// No SIGKILL rung on this ladder, deliberately. A window that ignores its close request
    /// usually means the app is asking something ("save changes?") or is hung — and the only
    /// harder verb X offers is against the whole client, which is exactly what the window hunt
    /// exists to avoid. The survivor report says so and points at the process hunt instead.
    fn _close_windows(
        relevant: &BTreeMap<u32, Process>,
        owned: &BTreeMap<u32, Vec<(u32, String)>>,
        choose: bool,
    ) {
        use std::io::IsTerminal;
        let mut targets: Vec<(u32, String)> = Vec::new();
        let mut apps = std::collections::BTreeSet::new();
        for process in relevant.values().filter(|p| p.relation == Relation::Match) {
            let Some(windows) = owned.get(&process.pid) else { continue };
            if process.protected {
                eprintln!(
                    "{}",
                    notice(&format!("murder: sparing {}'s window — this shell's own ancestry", process.pid))
                );
                continue;
            }
            apps.insert(process.pid);
            targets.extend(windows.iter().cloned());
        }
        if targets.is_empty() {
            eprintln!("murder: nothing left to close");
            return;
        }
        if !std::io::stdin().is_terminal() {
            eprintln!("murder: refusing to close windows non-interactively — run it from a terminal");
            return;
        }
        if choose {
            let options: Vec<String> =
                targets.iter().map(|(id, title)| format!("{id} · {title}")).collect();
            let refs: Vec<&str> = options.iter().map(String::as_str).collect();
            let mut form = terminal_choice::Form::new()
                .title("murder — tick the windows to close")
                .checkboxes("", &refs);
            match terminal_choice::run(&mut form) {
                Err(err) => return eprintln!("murder: {err}"),
                Ok(terminal_choice::Outcome::Cancelled) => {
                    return eprintln!("murder: aborted — nothing chosen")
                }
                Ok(terminal_choice::Outcome::Submitted) => {
                    let ticked: std::collections::BTreeSet<u32> = form
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            terminal_choice::Item::Checkboxes { options, checked, .. } => Some(
                                options
                                    .iter()
                                    .zip(checked)
                                    .filter(|(_, on)| **on)
                                    .filter_map(|(option, _)| _picked_pid(option)),
                            ),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    targets.retain(|(id, _)| ticked.contains(id));
                    if targets.is_empty() {
                        return eprintln!("murder: nothing ticked — nothing to close");
                    }
                }
            }
        }
        let keeps = if apps.len() == 1 { "app keeps" } else { "apps keep" };
        let ask = format!(
            "Close {}? The owning {keeps} running, other windows included",
            _count(targets.len(), "window", "windows")
        );
        if !prompt_yN(&ask) {
            eprintln!("murder: aborted — nothing closed");
            return;
        }
        let ids: Vec<u32> = targets.iter().map(|(id, _)| *id).collect();
        if let Err(why) = crate::support::windows::close(&ids) {
            eprintln!("{}", problematic(&format!("murder: {why}")));
            return;
        }
        // The same grace idea as the signal ladder, without the hammer at the end: wait for the
        // windows to actually go, then name whatever stayed.
        let mut open = ids.clone();
        for _ in 0..GRACE_POLLS {
            open = crate::support::windows::still_open(&open);
            if open.is_empty() {
                break;
            }
            std::thread::sleep(GRACE_POLL);
        }
        open = crate::support::windows::still_open(&open);
        match open.as_slice() {
            [] => eprintln!("murder: {} closed", _count(ids.len(), "window", "windows")),
            stubborn => {
                let names: Vec<&str> = targets
                    .iter()
                    .filter(|(id, _)| stubborn.contains(id))
                    .map(|(_, title)| title.as_str())
                    .collect();
                eprintln!(
                    "{}",
                    notice(&format!(
                        "murder: {} closed; still open: {} — the app may be asking to confirm \
                         (unsaved work?), or is hung. To kill the whole process instead, hunt it \
                         by text: murder <name>",
                        _count(ids.len() - stubborn.len(), "window", "windows"),
                        names.join(", ")
                    ))
                );
            }
        }
    }

    /// How long a SIGTERM'd process gets to clean up before SIGKILL: [`GRACE_POLLS`] ×
    /// [`GRACE_POLL`] = two seconds, checked in small steps so quick exits end the wait early.
    const GRACE_POLLS: u32 = 20;
    const GRACE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

    /// Split the matches into what may be signalled and what must not, each refusal carrying its
    /// reason. Pure, so the refusal policy is testable without killing anything.
    fn _triage<'a>(targets: &[&'a Process]) -> (Vec<&'a Process>, Vec<(&'a Process, &'static str)>) {
        let mut killable = Vec::new();
        let mut refused = Vec::new();
        for process in targets {
            if process.pid == 1 {
                refused.push((*process, "PID 1: killing init takes the machine down with it"));
            } else if process.protected {
                refused.push((*process, "this shell's own ancestry (the starred rows)"));
            } else if process.cmdline.is_empty() {
                refused.push((*process, "a kernel thread — not something a signal can reach"));
            } else {
                killable.push(*process);
            }
        }
        (killable, refused)
    }

    /// Send `signal` to `pid`, surfacing the OS's verdict.
    fn _signal(pid: u32, signal: libc::c_int) -> std::io::Result<()> {
        // SAFETY: `kill` with a valid signal number touches nothing of ours; the OS checks the
        // target and our permission to signal it, and says no via errno.
        match unsafe { libc::kill(pid as libc::pid_t, signal) } {
            0 => Ok(()),
            _ => Err(std::io::Error::last_os_error()),
        }
    }

    /// Whether `pid` is still something SIGKILL could act on. A zombie is done — its exit only
    /// awaits the parent's acknowledgement, and no signal changes that — so it counts as gone
    /// rather than "stubborn".
    fn _alive(pid: u32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => !matches!(_state(&stat), Some('Z' | 'X')),
            Err(_) => false,
        }
    }

    /// Field 3 of `/proc/<pid>/stat`, the state letter — read past the parenthesised name for the
    /// same reason [`_start_ticks`] is.
    fn _state(stat: &str) -> Option<char> {
        stat[stat.rfind(')')? + 1..].split_whitespace().next()?.chars().next()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn sample(pid: u32, ppid: u32, cmdline: &str) -> Process {
            Process {
                pid,
                ppid,
                user: "me".into(),
                name: "proc".into(),
                cmdline: cmdline.into(),
                exe: None,
                age: 0,
                matched: Vec::new(),
                cpu_percent: 0.0,
                rss_kib: None,
                window: None,
                protected: false,
                relation: Relation::Ancestor,
            }
        }

        fn matched(patterns: &[&str], mut process: Process) -> Option<String> {
            let owned: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
            _mark_matches(&mut process, &owned, None);
            (!process.matched.is_empty()).then(|| process.matched.join(","))
        }

        fn needles(patterns: &[&str]) -> Vec<String> {
            patterns.iter().map(|p| p.to_string()).collect()
        }

        /// `/proc/<pid>/stat`'s second field is the executable name in parentheses, and a name can
        /// contain spaces and parentheses of its own. Splitting the line naively shifts every
        /// later field, so a process called `foo (bar) baz` would report a wild start time.
        #[test]
        fn start_time_survives_a_hostile_process_name() {
            // Every field holds its own number, so the assertion reads as "field 22, please":
            // 1 is the pid, 2 the parenthesised name, 3 the state, and 4 onwards count up.
            let fields = |name: &str| {
                let tail: Vec<String> = (4..=52).map(|n| n.to_string()).collect();
                format!("7 ({name}) S {}", tail.join(" "))
            };
            assert_eq!(_start_ticks(&fields("bash")), Some(22));
            assert_eq!(_start_ticks(&fields("foo (bar) baz")), Some(22));
            assert_eq!(_start_ticks(&fields("has space")), Some(22));
            assert_eq!(_start_ticks("garbage with no paren"), None);
        }

        #[test]
        fn the_parent_pid_is_read_past_a_hostile_name() {
            assert_eq!(_ppid("7 (bash) S 42 3 4 5"), Some(42));
            assert_eq!(_ppid("7 (foo (bar) baz) S 42 3 4 5"), Some(42));
            assert_eq!(_ppid("nonsense"), None);
        }

        #[test]
        fn the_command_line_is_rebuilt_from_its_nul_separated_parts() {
            assert_eq!(_cmdline(b"vim\0-p\0a.txt\0"), "vim -p a.txt");
            assert_eq!(_cmdline(b"solo\0"), "solo");
            // A kernel thread has no command line at all; the report shows its name instead.
            assert_eq!(_cmdline(b""), "");
        }

        /// An argument can hold a newline (which would break the tree apart) or an ANSI escape
        /// (which could forge rows or hide the highlighting). Neither survives into the report:
        /// control characters become `?`, the `ps` convention.
        #[test]
        fn control_characters_in_arguments_are_defanged() {
            assert_eq!(_cmdline(b"evil\nname\0--x\0"), "evil?name --x");
            assert_eq!(_cmdline(b"fake\x1b[31mred\0"), "fake?[31mred");
            assert_eq!(_cmdline(b"tab\there\0"), "tab?here");
        }

        #[test]
        fn the_owning_uid_is_the_first_of_the_four() {
            let status = "Name:\tbash\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";
            assert_eq!(_real_uid(status), Some(1000));
            assert_eq!(_real_uid("Name:\tbash\n"), None);
        }

        #[test]
        fn passwd_lines_map_uid_to_name() {
            assert_eq!(_passwd_entry("root:x:0:0:root:/root:/bin/bash"), Some((0, "root".into())));
            assert_eq!(_passwd_entry("me:x:1000:1000::/home/me:/bin/sh"), Some((1000, "me".into())));
            assert_eq!(_passwd_entry("# a comment"), None);
            assert_eq!(_passwd_entry(""), None);
        }

        /// A PID matches whole, never as a substring — searching `42` must not drag in 1042.
        /// Everything else matches on a fragment, which is how you find a process you only
        /// half-remember.
        #[test]
        fn matching_is_whole_for_pids_and_partial_for_text() {
            let firefox = |pid: u32| {
                let mut process = sample(pid, 1, "/usr/lib/firefox/firefox --profile Default");
                process.name = "Firefox".into();
                process.exe = Some("/usr/lib/firefox/firefox".into());
                process
            };
            assert_eq!(matched(&["42"], firefox(42)).as_deref(), Some("pid"));
            assert_eq!(matched(&["42"], firefox(1042)), None, "a PID is not a substring search");
            assert_eq!(matched(&["FIREFOX"], firefox(9)).as_deref(), Some("name,cmdline,exe"));
            assert_eq!(matched(&["--profile"], firefox(9)).as_deref(), Some("cmdline"), "arguments count");
            assert_eq!(matched(&["chrome"], firefox(9)), None);
        }

        /// An ancestor's command line holds this very invocation, so matching on it would report
        /// the calling shell for every single search — `murder nonsense` found one before this.
        /// Its other fields still match, because those match for real reasons.
        #[test]
        fn an_ancestors_command_line_is_not_searched() {
            let ancestor = || {
                let mut process = sample(500, 1, "/bin/bash -c murder nonsense");
                process.name = "bash".into();
                process.exe = Some("/bin/bash".into());
                process.protected = true;
                process
            };
            assert!(matched(&["nonsense"], ancestor()).is_none(), "the invocation is not a match");
            assert_eq!(
                matched(&["bash"], ancestor()).as_deref(),
                Some("name,exe"),
                "name and exe still count; the cmdline is the one field withheld"
            );
            assert_eq!(matched(&["500"], ancestor()).as_deref(), Some("pid"));
        }

        #[test]
        fn ages_read_as_the_two_units_that_matter() {
            assert_eq!(_age_label(9), "9s");
            assert_eq!(_age_label(90), "1m30s");
            assert_eq!(_age_label(3_700), "1h1m");
            assert_eq!(_age_label(275_400), "3d4h");
        }

        // ——— highlighting ———————————————————————————————————————————————

        #[test]
        fn highlighting_finds_the_pattern_whatever_its_case() {
            let out = _highlight("Firefox --Profile", &needles(&["profile"]));
            assert!(out.contains("\x1b[30;41m"), "the g-style match block: {out:?}");
            assert!(out.contains("Profile"), "the ORIGINAL spelling is what glows: {out:?}");
            assert_eq!(_highlight("no hit here", &needles(&["zzz"])), "no hit here", "untouched when absent");
            assert_eq!(_highlight("text", &needles(&[""])), "text", "an empty needle highlights nothing");
        }

        #[test]
        fn every_occurrence_glows_not_just_the_first() {
            let out = _highlight("aXbXc", &needles(&["x"]));
            assert_eq!(out.matches("\x1b[30;41m").count(), 2, "{out:?}");
        }

        /// Several needles glow in one pass over the plain text. Highlighting one and then
        /// searching the result would let a needle match inside the inserted escape codes — `30`
        /// is literally in `\x1b[30;41m` — or split a span another needle already coloured.
        #[test]
        fn several_needles_glow_without_matching_the_colour_codes() {
            let out = _highlight("port 30 of firefox", &needles(&["firefox", "30"]));
            assert_eq!(out.matches("\x1b[30;41m").count(), 2, "one block each: {out:?}");
            // Overlapping needles merge into one block instead of nesting broken spans.
            let merged = _highlight("firefox", &needles(&["fire", "refox"]));
            assert_eq!(merged.matches("\x1b[30;41m").count(), 1, "{merged:?}");
            assert!(merged.contains("firefox"), "the whole word glows once: {merged:?}");
        }

        /// The conjunction: every pattern must land somewhere, each wherever it lands — so extra
        /// patterns narrow. `chrome 1043` is the chrome that owns that PID, not every chrome.
        #[test]
        fn every_pattern_must_match_for_the_process_to_count() {
            let firefox = |pid: u32| {
                let mut process = sample(pid, 1, "/usr/lib/firefox/firefox --profile Default");
                process.name = "Firefox".into();
                process.exe = Some("/usr/lib/firefox/firefox".into());
                process
            };
            assert_eq!(
                matched(&["firefox", "--profile"], firefox(9)).as_deref(),
                Some("name,cmdline,exe"),
                "both landed; the fields are the union, deduplicated"
            );
            assert_eq!(
                matched(&["firefox", "9"], firefox(9)).as_deref(),
                Some("name,cmdline,exe,pid"),
                "one pattern by text, the other by PID"
            );
            assert_eq!(matched(&["firefox", "chrome"], firefox(9)), None, "one miss disqualifies");
            assert_eq!(matched(&["firefox", "42"], firefox(9)), None, "a wrong PID disqualifies too");
        }

        #[test]
        fn case_insensitive_search_respects_multibyte_boundaries() {
            assert_eq!(_find_ci("çat cAt", "cat"), Some((5, 3)), "the ç is not a c");
            assert_eq!(_find_ci("ÜNICODE", "ünicode"), Some((0, 8)), "folded across cases");
            assert_eq!(_find_ci("plain", "zzz"), None);
        }

        // ——— the tree ————————————————————————————————————————————————————

        /// Assemble a small world: init(1) → bash(100) → {match(200) → child(300), match(250)}.
        fn world() -> BTreeMap<u32, Process> {
            let mut all = BTreeMap::new();
            for (pid, ppid, cmd) in [
                (1, 0, "init"),
                (100, 1, "bash"),
                (200, 100, "target --alpha"),
                (300, 200, "target-worker"),
                (250, 100, "target --beta"),
                (999, 1, "unrelated"),
            ] {
                all.insert(pid, sample(pid, ppid, cmd));
            }
            all
        }

        /// Two matches under one shell: one lineage, printed once — the shared ancestry must not
        /// repeat per match, and the unrelated process must not appear at all.
        #[test]
        fn one_lineage_prints_once_with_every_relation_labelled() {
            let relevant = _relate(world(), &[200, 250]);
            let relation = |pid: u32| relevant.get(&pid).map(|p| p.relation);
            assert_eq!(relation(200), Some(Relation::Match));
            assert_eq!(relation(250), Some(Relation::Match));
            assert_eq!(relation(300), Some(Relation::Descendant), "killing 200 takes 300 with it");
            assert_eq!(relation(100), Some(Relation::Ancestor));
            assert_eq!(relation(1), Some(Relation::Ancestor));
            assert_eq!(relation(999), None, "not in any match's lineage");

            let lines = _render(&relevant, &needles(&["target"]), false);
            for pid in ["100", "200", "250", "300"] {
                let rows = lines.iter().filter(|l| l.contains(&format!("{pid} "))).count();
                assert_eq!(rows, 1, "pid {pid} must appear exactly once:\n{lines:#?}");
            }
            // The tree hangs together: the two matches and the grandchild are drawn as branches.
            assert_eq!(lines.iter().filter(|l| l.contains("├─ ") || l.contains("└─ ")).count(), 4, "{lines:#?}");
        }

        /// A process between two matches — ancestor of one, descendant of the other — dies with
        /// the upper match, so calling it "context" would be a lie.
        #[test]
        fn a_process_between_two_matches_counts_as_a_descendant() {
            let mut all = world();
            all.insert(210, sample(210, 200, "middleman"));
            all.insert(220, sample(220, 210, "target --gamma"));
            let relevant = _relate(all, &[200, 220]);
            assert_eq!(relevant[&210].relation, Relation::Descendant);
        }

        /// The command column shows the full text — a long command line is the data, and its tail
        /// (the profile, the config path) is often the part that distinguishes two siblings.
        #[test]
        fn commands_print_in_full_and_matches_glow() {
            let long = format!("daemon --with {} --profile target", "x".repeat(300));
            let mut all = BTreeMap::new();
            all.insert(1, sample(1, 0, "init"));
            all.insert(50, sample(50, 1, &long));
            let relevant = _relate(all, &[50]);
            let lines = _render(&relevant, &needles(&["target"]), false);
            let row = lines.iter().find(|l| l.contains("daemon")).expect("the match row");
            assert!(row.contains(&"x".repeat(300)), "nothing may be truncated");
            assert!(row.contains("\x1b[30;41m"), "the match glows in place: {row:?}");
        }

        /// A match the command line doesn't show — the Mullvad case, where argv says
        /// `/proc/self/exe` and only the executable's real path matched — names the field that
        /// hit, so no row leaves the reader guessing why it qualified.
        #[test]
        fn an_invisible_match_names_the_field_that_hit() {
            let mut process = sample(70, 1, "/proc/self/exe --type=utility");
            process.name = "mullvad-gui".into();
            process.exe = Some("/opt/Mullvad VPN/mullvad-gui".into());
            let mut all = BTreeMap::new();
            all.insert(1, sample(1, 0, "init"));
            all.insert(70, process);
            let mut with_marks = all;
            _mark_matches(with_marks.get_mut(&70).unwrap(), &needles(&["mullvad"]), None);
            let relevant = _relate(with_marks, &[70]);
            let lines = _render(&relevant, &needles(&["mullvad"]), false);
            let row = lines.iter().find(|l| l.contains("/proc/self/exe")).expect("the match row");
            assert!(row.contains("name:") && row.contains("exe:"), "the hidden hits are named: {row:?}");
            assert!(row.contains("\x1b[30;41m"), "and they glow: {row:?}");
        }

        /// A match found by its PID has nothing to show for it in the command text — the PID
        /// cell itself glows, so the row still explains why it is here.
        #[test]
        fn a_pid_match_glows_in_the_pid_column() {
            let mut all = BTreeMap::new();
            all.insert(1, sample(1, 0, "init"));
            all.insert(77, sample(77, 1, "quiet-daemon"));
            _mark_matches(all.get_mut(&77).unwrap(), &needles(&["77"]), None);
            let relevant = _relate(all, &[77]);
            let lines = _render(&relevant, &needles(&["77"]), false);
            let row = lines.iter().find(|l| l.contains("quiet-daemon")).expect("the match row");
            assert!(row.contains("\x1b[30;41m77"), "the PID cell carries the glow: {row:?}");
        }

        // ——— the kill gate ————————————————————————————————————————————————

        /// The refusal policy, pinned without a single signal being sent: PID 1, the shell's own
        /// starred ancestry, and kernel threads are spared — each with its reason — and everything
        /// else is fair game once the prompt is answered.
        #[test]
        fn triage_spares_init_ancestry_and_kernel_threads() {
            let mut init = sample(1, 0, "init");
            init.relation = Relation::Match;
            let mut own_shell = sample(90, 1, "bash");
            own_shell.protected = true;
            let kernel_thread = {
                let mut kt = sample(55, 2, "");
                kt.name = "kworker/0:1".into();
                kt
            };
            let ordinary = sample(300, 90, "stale-daemon --serve");
            let targets = [&init, &own_shell, &kernel_thread, &ordinary];
            let (killable, refused) = _triage(&targets);
            assert_eq!(killable.iter().map(|p| p.pid).collect::<Vec<_>>(), [300]);
            let reasons: BTreeMap<u32, &str> =
                refused.iter().map(|(process, why)| (process.pid, *why)).collect();
            assert!(reasons[&1].contains("PID 1"));
            assert!(reasons[&90].contains("ancestry"));
            assert!(reasons[&55].contains("kernel thread"));
        }

        #[test]
        fn counts_decline_their_nouns() {
            assert_eq!(_count(1, "process", "processes"), "1 process");
            assert_eq!(_count(4, "process", "processes"), "4 processes");
            assert_eq!(_count(0, "process-tree", "process-trees"), "0 process-trees");
        }

        /// Two matches under one shell are two trees (killing one leaves the other); a match
        /// inside another match's subtree is the same tree — its fate is already decided above.
        #[test]
        fn the_tree_count_is_of_independent_kill_roots() {
            let siblings = _relate(world(), &[200, 250]);
            let killable: Vec<&Process> =
                siblings.values().filter(|p| p.relation == Relation::Match).collect();
            assert_eq!(_tree_count(&siblings, &killable), 2, "unrelated siblings: two trees");

            let nested = _relate(world(), &[200, 300]); // 300 is 200's child
            let killable: Vec<&Process> =
                nested.values().filter(|p| p.relation == Relation::Match).collect();
            assert_eq!(_tree_count(&nested, &killable), 1, "a match inside a match: one tree");
        }

        #[test]
        fn cpu_ticks_and_rss_read_their_fields() {
            // Fields hold their own numbers (see `start_time_survives_a_hostile_process_name`):
            // utime is field 14, stime 15 — so the sum reads 14 + 15.
            let tail: Vec<String> = (4..=52).map(|n| n.to_string()).collect();
            let stat = format!("7 (foo (bar) baz) S {}", tail.join(" "));
            assert_eq!(_cpu_ticks(&stat), Some(29));
            assert_eq!(_cpu_ticks("garbage"), None);
            assert_eq!(_rss_kib("Name:\tx\nVmRSS:\t  51300 kB\n"), Some(51_300));
            assert_eq!(_rss_kib("Name:\tkworker\n"), None, "kernel threads own no user memory");
        }

        #[test]
        fn memory_reads_in_the_unit_that_fits() {
            assert_eq!(_mem_label(800), "800KB");
            assert_eq!(_mem_label(51_300), "50.1MB");
            assert_eq!(_mem_label(2_202_010), "2.1GB");
        }

        /// `--window` is one more conjunct: owning a matched window is required when the flag is
        /// given — sufficient alone, and disqualifying when absent even if every text pattern hit.
        #[test]
        fn the_window_filter_is_a_conjunct_like_any_pattern() {
            let owned =
                BTreeMap::from([(9, vec![(0xC0FFEE_u32, "Site That Breaks - Browser".to_string())])]);
            let mut with_window = sample(9, 1, "/usr/lib/firefox/firefox");
            _mark_matches(&mut with_window, &[], Some(&owned));
            assert_eq!(with_window.matched, ["window"], "a window match alone suffices");
            assert_eq!(with_window.window.as_deref(), Some("Site That Breaks - Browser"));

            let mut without = sample(10, 1, "/usr/lib/firefox/firefox");
            _mark_matches(&mut without, &needles(&["firefox"]), Some(&owned));
            assert!(without.matched.is_empty(), "text hit, but owns no matched window");

            let mut half = sample(9, 1, "/usr/lib/firefox/firefox");
            _mark_matches(&mut half, &needles(&["chrome"]), Some(&owned));
            assert!(half.matched.is_empty(), "window hit, but a text pattern missed");
        }

        /// `--short` cuts the command at the window's edge — the matched text may be in the
        /// hidden part, and that is the documented price.
        #[test]
        fn short_mode_clips_the_command_to_the_window() {
            let long = format!("daemon {} --profile target", "x".repeat(300));
            let mut all = BTreeMap::new();
            all.insert(1, sample(1, 0, "init"));
            all.insert(50, sample(50, 1, &long));
            _mark_matches(all.get_mut(&50).unwrap(), &needles(&["target"]), None);
            let relevant = _relate(all, &[50]);
            let full = _render(&relevant, &needles(&["target"]), false);
            let clipped = _render(&relevant, &needles(&["target"]), true);
            let row = |lines: &[String]| lines.iter().find(|l| l.contains("daemon")).unwrap().clone();
            assert!(row(&full).contains(&"x".repeat(300)), "unclipped shows everything");
            assert!(!row(&clipped).contains(&"x".repeat(100)), "clipped does not");
            assert!(row(&clipped).contains('…'), "and says something was removed: {:?}", row(&clipped));
        }

        // ——— the no-pattern picker ————————————————————————————————————————

        fn hog(pid: u32, cpu: f64, rss: Option<u64>) -> Process {
            let mut process = sample(pid, 1, "some-daemon --running");
            process.name = "some-daemon".into(); // the comm name is what the checklist shows
            process.cpu_percent = cpu;
            process.rss_kib = rss;
            process
        }

        /// Each shortlist caps at [`TOP_HOGS`] and is honestly its own top: a monster leading
        /// both charts appears in BOTH, worded identically — `mirror_duplicates` on the form is
        /// what makes those two boxes one, so the wording HAS to be the same.
        #[test]
        fn hog_shortlists_are_each_their_own_honest_top() {
            let mut all = BTreeMap::new();
            for pid in 0..20u32 {
                // pid 10 is the monster: hottest CPU and biggest memory at once.
                let cpu = if pid == 10 { 99.0 } else { f64::from(pid) };
                let rss = if pid == 10 { 9_000_000 } else { 1_000 + u64::from(pid) };
                all.insert(100 + pid, hog(100 + pid, cpu, Some(rss)));
            }
            let (by_cpu, by_memory) = _hog_shortlists(&all);
            assert_eq!(by_cpu.len(), TOP_HOGS);
            assert_eq!(by_memory.len(), TOP_HOGS);
            assert_eq!(by_cpu[0].pid, 110, "the monster leads the CPU list");
            assert_eq!(by_memory[0].pid, 110, "and the memory list too — no exclusion games");
            let widths = _column_widths(by_cpu.iter().chain(by_memory.iter()).copied());
            assert_eq!(
                _hog_row(by_cpu[0], widths),
                _hog_row(by_memory[0], widths),
                "twin entries carry identical wording, or mirroring could not link them"
            );
        }

        /// "Up to 7": a quiet system offers what it has — three, one, or nothing — without
        /// glitching on the shortfall.
        #[test]
        fn hog_shortlists_survive_a_nearly_empty_system() {
            let mut three = BTreeMap::new();
            for pid in [5u32, 6, 7] {
                three.insert(pid, hog(pid, f64::from(pid), Some(u64::from(pid) * 100)));
            }
            let (by_cpu, by_memory) = _hog_shortlists(&three);
            assert_eq!(by_cpu.len(), 3, "all three make the CPU list");
            assert_eq!(by_memory.len(), 3, "and the memory list — mirroring merges the twins");

            let one = BTreeMap::from([(5u32, hog(5, 1.0, None))]);
            let (by_cpu, by_memory) = _hog_shortlists(&one);
            assert_eq!((by_cpu.len(), by_memory.len()), (1, 1));

            let none = BTreeMap::new();
            let (by_cpu, by_memory) = _hog_shortlists(&none);
            assert!(by_cpu.is_empty() && by_memory.is_empty(), "an empty system offers nothing");
        }

        /// The picker never offers what the trigger would refuse: no box can be ticked in vain.
        #[test]
        fn hogs_exclude_what_could_never_be_killed() {
            let mut all = BTreeMap::new();
            all.insert(1, hog(1, 90.0, Some(9_999_999)));
            let mut own = hog(50, 80.0, Some(8_888_888));
            own.protected = true;
            all.insert(50, own);
            let mut kernel = hog(60, 70.0, None);
            kernel.cmdline = String::new();
            all.insert(60, kernel);
            all.insert(70, hog(70, 1.0, Some(100)));
            let (by_cpu, by_memory) = _hog_shortlists(&all);
            assert_eq!(by_cpu.iter().map(|p| p.pid).collect::<Vec<_>>(), [70]);
            assert_eq!(by_memory.iter().map(|p| p.pid).collect::<Vec<_>>(), [70]);
        }

        /// The `--choose` form is the tree, re-expressed: matches become checkbox options in an
        /// anonymous group, context rows become comments, order intact, alignment on — and every
        /// option's pid reads back out, star and all.
        #[test]
        fn the_choose_form_mirrors_the_tree() {
            let relevant = _relate(world(), &[200, 250]);
            let needles = needles(&["target"]);
            let form = {
                // Reuse the assembly _choose_matches performs, minus the interactive run: build
                // rows the same way and check the shape a user would face.
                let mut form = terminal_choice::Form::new().aligned().comment(_plain_header(&relevant));
                let mut pending: Vec<String> = Vec::new();
                for (pid, row) in _tree_rows(&relevant, &needles, false, false) {
                    let process = &relevant[&pid];
                    if process.relation == Relation::Match && !process.protected {
                        pending.push(row);
                        continue;
                    }
                    if !pending.is_empty() {
                        let options: Vec<&str> = pending.iter().map(String::as_str).collect();
                        form = form.checkboxes("", &options);
                        pending.clear();
                    }
                    form = form.comment(row);
                }
                if !pending.is_empty() {
                    let options: Vec<&str> = pending.iter().map(String::as_str).collect();
                    form = form.checkboxes("", &options);
                }
                form
            };
            assert!(form.aligned, "tree columns need the aligned comments");
            let mut option_pids = Vec::new();
            let mut comment_rows = 0;
            for item in &form.items {
                match item {
                    terminal_choice::Item::Comment(_) => comment_rows += 1,
                    terminal_choice::Item::Checkboxes { label, options, .. } => {
                        assert!(label.is_empty(), "tree groups are anonymous — no heading line");
                        option_pids.extend(options.iter().filter_map(|o| _picked_pid(o)));
                    }
                    other => panic!("nothing else belongs in this form: {other:?}"),
                }
            }
            option_pids.sort_unstable();
            assert_eq!(
                option_pids,
                [200, 250],
                "exactly the matches are tickable — a descendant is blast radius, not a choice"
            );
            assert!(comment_rows >= 4, "header, ancestors AND the descendant stay fixed");
        }

        /// The pid survives the round trip out of a STARRED pid cell too — `1234*` is how the
        /// tree spells this shell's own ancestry.
        #[test]
        fn picked_pids_tolerate_the_ancestry_star() {
            assert_eq!(_picked_pid("1234*  claude  3s  0.0  1.2MB  └─ bash"), Some(1234));
            assert_eq!(_picked_pid("77 · quiet-daemon · 0.0% cpu · -"), Some(77));
        }

        /// The checkbox line must carry the pid back out — the whole selection rests on it —
        /// and it wears the standard murder columns, header included.
        #[test]
        fn a_ticked_option_reads_back_to_its_pid() {
            let process = hog(4242, 12.34, Some(51_300));
            let widths = _column_widths(std::iter::once(&process));
            let option = _hog_row(&process, widths);
            assert_eq!(_picked_pid(&option), Some(4242), "{option:?}");
            assert!(
                option.contains("some-daemon --running") && option.contains("50.1MB"),
                "the full command in the standard columns: {option:?}"
            );
            let header = _widths_header(widths);
            assert_eq!(
                header.find("COMMAND"),
                option.find("some-daemon"),
                "header and row share their columns: {header:?} vs {option:?}"
            );
            assert_eq!(_picked_pid("not a pid at all"), None);
            // The -c options glow, and a pid matched BY pid glows in this very token.
            assert_eq!(_picked_pid("\x1b[30;41m4242\x1b[0m*  me  1s"), Some(4242));
        }

        /// A zombie is done — no signal changes anything about it — so the grace-period poll must
        /// count it as gone rather than "stubborn, needs SIGKILL".
        #[test]
        fn the_liveness_check_reads_the_state_letter() {
            assert_eq!(_state("7 (bash) S 42 3"), Some('S'));
            assert_eq!(_state("7 (foo (bar) baz) Z 42 3"), Some('Z'));
            assert_eq!(_state("garbage"), None);
        }
    }
}
