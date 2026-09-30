//! What changed inside each changed line of a unified diff — the engine behind the `diff` command
//! ([`crate::categories::builtins`]). Git colours whole lines, removed red and added green; this
//! also pairs each removed line with the added line that replaced it and paints the parts that
//! differ on a background of their own, the way `delta` and git's own `contrib/diff-highlight` do.
//!
//! Pure until the last step: `runs` cuts the diff into stretches of bytes, each tagged with the
//! `Part` it is painted as, and `render` only writes them. So every decision is testable as data,
//! and the palette is one function (`spec`) — the single place to retune a colour. The diff's
//! bytes are never altered: with colour off and no line numbers, the output is the input.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::io;
use std::ops::Range;
use std::sync::LazyLock;

use regex::bytes::Regex;
use termcolor::{Color, ColorSpec, WriteColor};

use crate::support::shell;

/// How much of two lines' visible text must be shared for them to count as one line edited, and
/// be highlighted: 40%, roughly the bar `delta` sets (its default `max-line-distance` of 0.6
/// pairs lines that differ by at most 60%). Below it the two are unrelated lines that happen to
/// sit side by side, and marking the little they share would be noise.
const MIN_SHARED_PERCENT: usize = 40;

/// How far ahead, in added lines, a removed line looks for the line that replaced it. An
/// insertion inside an edited region pushes its pairs apart by the insertion's length, so this is
/// the longest insertion the pairing sees across — a hundred lines, a whole function; past that
/// the block is a rewrite, not an edit. It also keeps a block's cost linear: each removed line is
/// compared with at most this many candidates.
const LOOKAHEAD: usize = 100;

/// The largest subsequence table one line pair may cost: a thousand tokens a side (4 MB), a line
/// thousands of characters long — longer than hand-written lines run. Past it (a minified file's
/// one-line body) the table would grow with the square of the line, so what lies between the
/// common prefix and suffix is marked whole instead.
const MAX_CELLS: usize = 1_000_000;

/// What gets painted — one variant per role, coloured by [`spec`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    /// A file header: `diff --git`, `index`, `---`/`+++`, mode and rename lines, `Binary files …`.
    Meta,
    /// A hunk's `@@ -a,b +c,d @@` range. The function context git prints after it stays plain.
    Hunk,
    Removed,
    Added,
    /// The part of a removed line that its replacement does not keep.
    RemovedChange,
    /// The part of an added line that the line it replaced did not have.
    AddedChange,
    /// The line-number gutter `-n` puts before each line of a hunk.
    Gutter,
}

/// The palette. The whole-line colours are git's own defaults (`color.diff.meta`, `.frag`,
/// `.old`, `.new`), so the output reads like `git diff`. The changed parts keep the terminal's
/// own text colour and get the palette's red or green behind them — the ordinary shade, not the
/// bright one, so the text stays as readable as it is anywhere else and the background alone marks
/// the change. The gutter is dimmed: it is there to be glanced at, not read.
fn spec(part: Part) -> ColorSpec {
    let mut spec = ColorSpec::new();
    match part {
        Part::Meta => spec.set_bold(true),
        Part::Hunk => spec.set_fg(Some(Color::Cyan)),
        Part::Removed => spec.set_fg(Some(Color::Red)),
        Part::Added => spec.set_fg(Some(Color::Green)),
        Part::RemovedChange => spec.set_bg(Some(Color::Red)),
        Part::AddedChange => spec.set_bg(Some(Color::Green)),
        Part::Gutter => spec.set_dimmed(true),
    };
    spec
}

/// A stretch of output and the part it is painted as — `None` leaves it plain. Borrowed from the
/// diff, except the gutter, which is composed.
type Run<'a> = (Option<Part>, Cow<'a, [u8]>);

/// Writes `diff` to `out` with every part painted, and with each hunk line's numbers in the old
/// and the new file before it when `numbered`. When `out` is not in colour mode nothing else is
/// added, so what arrives is exactly `diff` (plus the numbers).
pub(crate) fn render(diff: &[u8], numbered: bool, out: &mut impl WriteColor) -> io::Result<()> {
    for (part, bytes) in runs(diff, numbered) {
        match part {
            Some(part) => shell::paint(out, &spec(part), &bytes)?,
            None => out.write_all(&bytes)?,
        }
    }
    Ok(())
}

/// The diff cut into runs — in order, covering every byte exactly once.
fn runs(diff: &[u8], numbered: bool) -> Vec<Run<'_>> {
    let mut reader = Reader::default();
    let lines: Vec<Line> = diff
        .split_inclusive(|&byte| byte == b'\n')
        .map(|raw| {
            let (text, end) = raw.split_at(raw.len() - usize::from(raw.ends_with(b"\n")));
            let (kind, numbers) = reader.read(text);
            Line { kind, numbers, text, end }
        })
        .collect();
    let gutter = numbered.then(|| Gutter::fitting(&lines));
    let mut runs = Vec::new();
    for group in lines.chunk_by(|a, b| a.kind.in_block() == b.kind.in_block()) {
        if group[0].kind.in_block() {
            push_block(&mut runs, group, gutter);
        } else {
            for line in group {
                push_line(&mut runs, line, &[], gutter);
            }
        }
    }
    runs
}

/// One line of the diff: what it is, where it stands in each file, its text, and its line ending
/// (empty on a last line without one).
struct Line<'a> {
    kind: Kind,
    numbers: Numbers,
    text: &'a [u8],
    end: &'a [u8],
}

impl Line<'_> {
    /// A removed or added line's text past the marker, as [`Words`]. `None` for any other line,
    /// and for one that is not UTF-8 — a Latin-1 file's — whose characters cannot be told apart
    /// safely.
    fn words(&self) -> Option<Words<'_>> {
        match self.kind {
            // Past the one-byte `-`/`+`, which is ASCII — so the rest starts on a character boundary.
            Kind::Removed | Kind::Added => std::str::from_utf8(&self.text[1..]).ok().map(Words::new),
            _ => None,
        }
    }
}

/// A line's number in the old file and in the new: a removed line has no new one, an added line
/// no old one, and a header neither.
type Numbers = (Option<usize>, Option<usize>);

/// What a diff line is, which decides how it is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Meta,
    /// A hunk header whose `@@…@@` range is this many bytes long.
    Hunk(usize),
    Context,
    Removed,
    Added,
    /// `\ No newline at end of file`, annotating the line before it.
    Marker,
}

impl Kind {
    /// Whether the line is part of a change block: removed, added, or a marker riding with them.
    fn in_block(self) -> bool {
        matches!(self, Kind::Removed | Kind::Added | Kind::Marker)
    }

    /// Whether the line is part of a hunk's body — anything but a header — and so has a place in
    /// the gutter.
    fn in_hunk(self) -> bool {
        !matches!(self, Kind::Meta | Kind::Hunk(_))
    }
}

/// Tells lines apart, in order, and numbers them. Counting a hunk's lines is the only sound way
/// to know where it ends: a removed line that reads `-- x` arrives as `--- x`, exactly like the
/// next file's header.
#[derive(Default)]
struct Reader {
    /// The line numbers the current hunk has yet to show from the old file, and from the new —
    /// both empty between hunks.
    old: Range<usize>,
    new: Range<usize>,
}

impl Reader {
    fn read(&mut self, text: &[u8]) -> (Kind, Numbers) {
        // Never a header line, and never counted: it annotates a line rather than being one.
        if text.first() == Some(&b'\\') {
            return (Kind::Marker, (None, None));
        }
        if self.old.is_empty() && self.new.is_empty() {
            return match hunk_header(text) {
                Some(hunk) => {
                    (self.old, self.new) = (hunk.old, hunk.new);
                    (Kind::Hunk(hunk.range), (None, None))
                }
                None => (Kind::Meta, (None, None)),
            };
        }
        match text.first() {
            Some(b'-') => (Kind::Removed, (self.old.next(), None)),
            Some(b'+') => (Kind::Added, (None, self.new.next())),
            // An empty line is blank context printed without its space, as git does under
            // `diff.suppressBlankEmpty`.
            Some(b' ') | None => (Kind::Context, (self.old.next(), self.new.next())),
            // Nothing git prints inside a hunk; end it rather than guess.
            Some(_) => {
                (self.old, self.new) = (0..0, 0..0);
                (Kind::Meta, (None, None))
            }
        }
    }
}

/// What a hunk header announces.
#[derive(Debug, PartialEq, Eq)]
struct HunkHeader {
    /// The line numbers the hunk shows from the old file.
    old: Range<usize>,
    /// The line numbers the hunk shows from the new file.
    new: Range<usize>,
    /// Length of the `@@…@@` range, which ends where any function context begins.
    range: usize,
}

/// Reads `@@ -a,b +c,d @@`. A count git leaves out is 1, as the format defines it.
fn hunk_header(text: &[u8]) -> Option<HunkHeader> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^@@ -([0-9]+)(?:,([0-9]+))? \+([0-9]+)(?:,([0-9]+))? @@").expect("a valid pattern")
    });
    let found = HEADER.captures(text)?;
    let number = |group, omitted| match found.get(group) {
        Some(digits) => std::str::from_utf8(digits.as_bytes()).ok()?.parse().ok(),
        None => omitted,
    };
    let side = |start, count| {
        let start: usize = number(start, None)?;
        Some(start..start + number(count, Some(1))?)
    };
    Some(HunkHeader { old: side(1, 2)?, new: side(3, 4)?, range: found.get(0)?.end() })
}

/// The line-number gutter `-n` puts before each line of a hunk: the line's number in the old file
/// and in the new, each as wide as the widest number in the diff and blank where the line has
/// none, then a bar.
#[derive(Clone, Copy)]
struct Gutter {
    width: usize,
}

impl Gutter {
    fn fitting(lines: &[Line]) -> Self {
        let widest = lines.iter().flat_map(|line| [line.numbers.0, line.numbers.1]).flatten().max();
        Gutter { width: widest.unwrap_or(0).to_string().len() }
    }

    fn text(self, numbers: Numbers) -> String {
        let cell = |number: Option<usize>| number.map_or_else(String::new, |number| number.to_string());
        format!("{:>width$} {:>width$} │", cell(numbers.0), cell(numbers.1), width = self.width)
    }
}

/// Paints one line. `changed` holds the spans to highlight in a removed or added line, as
/// offsets into its text after the `-`/`+`; empty paints the line whole.
fn push_line<'a>(runs: &mut Vec<Run<'a>>, line: &Line<'a>, changed: &[Range<usize>], gutter: Option<Gutter>) {
    if let Some(gutter) = gutter.filter(|_| line.kind.in_hunk()) {
        runs.push((Some(Part::Gutter), Cow::Owned(gutter.text(line.numbers).into_bytes())));
    }
    match line.kind {
        Kind::Meta => push(runs, Some(Part::Meta), line.text),
        Kind::Hunk(range) => {
            let (range, context) = line.text.split_at(range);
            push(runs, Some(Part::Hunk), range);
            push(runs, None, context);
        }
        Kind::Context | Kind::Marker => push(runs, None, line.text),
        Kind::Removed => push_changed(runs, line.text, changed, Part::Removed, Part::RemovedChange),
        Kind::Added => push_changed(runs, line.text, changed, Part::Added, Part::AddedChange),
    }
    push(runs, None, line.end);
}

/// A change block — the removed and added lines between two unchanged ones. Each removed line is
/// paired with the first added line, among the ones not yet paired, that is alike enough to be
/// its edited self ([`pair`]); the added lines passed over on the way, and any removed line that
/// finds none within [`LOOKAHEAD`], are painted whole. Pairs never cross: git prints each side
/// in order, so a line's replacement is never behind another's.
fn push_block<'a>(runs: &mut Vec<Run<'a>>, block: &[Line<'a>], gutter: Option<Gutter>) {
    let words: Vec<Option<Words>> = block.iter().map(Line::words).collect();
    let positions = |kind| block.iter().enumerate().filter(move |(_, line)| line.kind == kind).map(|(at, _)| at);
    let added: Vec<usize> = positions(Kind::Added).collect();
    let mut changed = vec![Spans::new(); block.len()];
    let mut unpaired = 0; // into `added`: the first line no removed line has claimed
    for old in positions(Kind::Removed) {
        let Some(old_words) = &words[old] else { continue };
        let mut candidates = added[unpaired..].iter().take(LOOKAHEAD).enumerate();
        let found = candidates.find_map(|(passed, &new)| {
            let (old_side, new_side) = pair(old_words, words[new].as_ref()?)?;
            Some((passed, new, old_side.changed(), new_side.changed()))
        });
        if let Some((passed, new, old_spans, new_spans)) = found {
            changed[old] = old_spans;
            changed[new] = new_spans;
            unpaired += passed + 1;
        }
    }
    for (line, changed) in block.iter().zip(&changed) {
        push_line(runs, line, changed, gutter);
    }
}

/// A removed or added line: its marker and whatever stayed in `whole`, and each `changed` span —
/// offsets into the text after the marker — in `change`.
fn push_changed<'a>(runs: &mut Vec<Run<'a>>, text: &'a [u8], changed: &[Range<usize>], whole: Part, change: Part) {
    let mut at = 0;
    for span in changed {
        let (start, end) = (1 + span.start, 1 + span.end);
        push(runs, Some(whole), &text[at..start]);
        push(runs, Some(change), &text[start..end]);
        at = end;
    }
    push(runs, Some(whole), &text[at..]);
}

/// Appends a run borrowed from the diff, skipping an empty one — it would only add escape codes.
fn push<'a>(runs: &mut Vec<Run<'a>>, part: Option<Part>, bytes: &'a [u8]) {
    if !bytes.is_empty() {
        runs.push((part, Cow::Borrowed(bytes)));
    }
}

/// The byte ranges of a line that changed, in order.
type Spans = Vec<Range<usize>>;

/// A removed or added line's text, cut into tokens once: the pairing compares each removed line
/// with up to [`LOOKAHEAD`] added ones, so the cutting must not repeat.
struct Words<'a> {
    tokens: Vec<&'a str>,
    /// The same tokens sorted, for the quick bound [`pair`] tries before its full comparison.
    sorted: Vec<&'a str>,
    /// Bytes of visible text — everything but whitespace. Whitespace is left out of every measure
    /// of likeness, so that two unrelated lines at one indent do not look half alike.
    visible: usize,
}

impl<'a> Words<'a> {
    fn new(text: &'a str) -> Self {
        let tokens = tokens(text);
        let mut sorted = tokens.clone();
        sorted.sort_unstable();
        let visible = tokens.iter().filter(|token| !is_space(token)).map(|token| token.len()).sum();
        Words { tokens, sorted, visible }
    }
}

/// Whether two lines sharing `shared` visible bytes are one line edited rather than two unrelated
/// lines: at least [`MIN_SHARED_PERCENT`] of their `total` visible bytes — both lines' put
/// together, so a shared byte, present in each, counts twice.
fn alike(shared: usize, total: usize) -> bool {
    total == 0 || 2 * shared * 100 >= MIN_SHARED_PERCENT * total
}

/// The visible bytes two lines' tokens have in common regardless of order — a bound on what a
/// common subsequence can share, found in linear time where the subsequence takes quadratic.
/// Most candidates in a large block are unrelated lines, and this is where they are turned away.
fn shared_regardless_of_order(old: &Words, new: &Words) -> usize {
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while let (Some(a), Some(b)) = (old.sorted.get(i), new.sorted.get(j)) {
        match a.cmp(b) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => {
                if !is_space(a) {
                    shared += a.len();
                }
                (i, j) = (i + 1, j + 1);
            }
        }
    }
    shared
}

/// One version of an edited line, with a flag per token for whether the other version has it too.
struct Side<'a> {
    tokens: &'a [&'a str],
    shared: Vec<bool>,
}

impl Side<'_> {
    /// Bytes of visible text in the shared tokens.
    fn shared_visible(&self) -> usize {
        let shared = self.tokens.iter().zip(&self.shared).filter(|(token, &shared)| shared && !is_space(token));
        shared.map(|(token, _)| token.len()).sum()
    }

    /// Byte ranges of the tokens not shared, merged across gaps that are only whitespace: `quick
    /// brown` changed to `slow red` reads as one change, not two with a space between.
    fn changed(&self) -> Spans {
        let mut spans = Spans::new();
        let mut at = 0;
        // Whether everything since the last span is whitespace — a gap a merge may bridge.
        let mut bridgeable = false;
        for (token, &shared) in self.tokens.iter().zip(&self.shared) {
            let range = at..at + token.len();
            at = range.end;
            if shared {
                bridgeable &= is_space(token);
                continue;
            }
            match spans.last_mut() {
                Some(last) if bridgeable => last.end = range.end,
                _ => spans.push(range),
            }
            bridgeable = true;
        }
        spans
    }
}

/// Two versions of a line as the [`Side`]s of one edit — or `None` when they are not [`alike`]
/// enough to be that. Tokens in the common prefix and suffix are shared, and so are those of a
/// longest common subsequence of what lies between — unless its table would pass [`MAX_CELLS`],
/// when all of the middle counts as changed.
fn pair<'a>(old: &'a Words<'a>, new: &'a Words<'a>) -> Option<(Side<'a>, Side<'a>)> {
    let total = old.visible + new.visible;
    if !alike(shared_regardless_of_order(old, new), total) {
        return None; // not even ignoring order — the full comparison could only agree
    }
    let (old, new) = (old.tokens.as_slice(), new.tokens.as_slice());
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (mut old_shared, mut new_shared) = (vec![false; old.len()], vec![false; new.len()]);
    for flags in [&mut old_shared, &mut new_shared] {
        let len = flags.len();
        flags[..prefix].fill(true);
        flags[len - suffix..].fill(true);
    }
    let (old_middle, new_middle) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    if old_middle.len().saturating_mul(new_middle.len()) <= MAX_CELLS {
        for (a, b) in common_subsequence(old_middle, new_middle) {
            old_shared[prefix + a] = true;
            new_shared[prefix + b] = true;
        }
    }
    let (old, new) = (Side { tokens: old, shared: old_shared }, Side { tokens: new, shared: new_shared });
    alike(old.shared_visible(), total).then_some((old, new))
}

/// A longest common subsequence of two token lists, as index pairs in order — the textbook
/// table, holding the answer for every pair of suffixes so that it reads forward.
fn common_subsequence(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let width = b.len() + 1;
    let at = |i: usize, j: usize| i * width + j;
    let mut table = vec![0_u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[at(i, j)] = if a[i] == b[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            pairs.push((i, j));
            (i, j) = (i + 1, j + 1);
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Cuts a line into the units a change is measured in: runs of word characters (letters, digits,
/// `_`), runs of whitespace, and every other character alone — so a change reads as whole words
/// and single punctuation marks.
fn tokens(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut run = None;
    for (at, c) in line.char_indices() {
        let class = Class::of(c);
        if run.is_some_and(|previous| previous != class || previous == Class::Other) {
            tokens.push(&line[start..at]);
            start = at;
        }
        run = Some(class);
    }
    if start < line.len() {
        tokens.push(&line[start..]);
    }
    tokens
}

/// The kinds of character a token is a run of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

impl Class {
    fn of(c: char) -> Self {
        if c.is_alphanumeric() || c == '_' {
            Self::Word
        } else if c.is_whitespace() {
            Self::Space
        } else {
            Self::Other
        }
    }
}

/// Whether a token is whitespace. A token is a run of one [`Class`], so its first character says.
fn is_space(token: &str) -> bool {
    token.starts_with(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use termcolor::Buffer;

    /// The painted diff in a notation a test can read: each painted run as `<tag:…>`, plain text
    /// as it is. Tags name parts, not colours, so retuning the palette breaks nothing here.
    fn painted(diff: impl AsRef<[u8]>) -> String {
        painted_with(diff, false)
    }

    /// [`painted`] with line numbers on.
    fn numbered(diff: impl AsRef<[u8]>) -> String {
        painted_with(diff, true)
    }

    fn painted_with(diff: impl AsRef<[u8]>, numbered: bool) -> String {
        let tag = |part| match part {
            Part::Meta => "meta",
            Part::Hunk => "hunk",
            Part::Removed => "old",
            Part::Added => "new",
            Part::RemovedChange => "OLD",
            Part::AddedChange => "NEW",
            Part::Gutter => "n",
        };
        runs(diff.as_ref(), numbered)
            .into_iter()
            .map(|(part, bytes)| {
                let text = String::from_utf8_lossy(&bytes);
                match part {
                    Some(part) => format!("<{}:{text}>", tag(part)),
                    None => text.into_owned(),
                }
            })
            .collect()
    }

    /// The two versions of a line with what changed in brackets — or `None` when [`pair`] finds
    /// them not alike enough to be one line edited.
    fn marked(old: &str, new: &str) -> Option<(String, String)> {
        let bracketed = |text: &str, spans: Spans| {
            let mut out = String::new();
            let mut at = 0;
            for span in spans {
                out += &format!("{}[{}]", &text[at..span.start], &text[span.clone()]);
                at = span.end;
            }
            out + &text[at..]
        };
        let (old_words, new_words) = (Words::new(old), Words::new(new));
        let (old_side, new_side) = pair(&old_words, &new_words)?;
        Some((bracketed(old, old_side.changed()), bracketed(new, new_side.changed())))
    }

    fn both(old: &str, new: &str) -> Option<(String, String)> {
        Some((old.to_string(), new.to_string()))
    }

    // ——— What counts as changed ———————————————————————————————————————————————

    /// The change is found at the grain a reader thinks in — the word, not the letter — with what
    /// stayed on either side of it left alone.
    #[test]
    fn a_changed_word_is_marked_on_both_sides() {
        assert_eq!(marked("let x = 1;", "let x = 42;"), both("let x = [1];", "let x = [42];"));
    }

    #[test]
    fn separate_changes_are_marked_separately() {
        assert_eq!(marked("let a = f(x);", "let b = f(y);"), both("let [a] = f([x]);", "let [b] = f([y]);"));
    }

    /// A run of changed words reads as one change, the spaces between them included.
    #[test]
    fn changes_separated_only_by_whitespace_merge() {
        assert_eq!(
            marked("the quick brown fox jumps", "the slow red fox jumps"),
            both("the [quick brown] fox jumps", "the [slow red] fox jumps"),
        );
    }

    /// Indentation is text too: re-indenting a line marks exactly the whitespace that moved.
    #[test]
    fn an_indentation_change_marks_the_indentation() {
        assert_eq!(marked("    return x;", "  return x;"), both("[    ]return x;", "[  ]return x;"));
    }

    /// Trailing whitespace is marked though nothing visible changed — the one edit a plain
    /// red/green diff leaves a reader squinting for.
    #[test]
    fn trailing_whitespace_is_marked() {
        assert_eq!(marked("x = 1  ", "x = 1"), both("x = 1[  ]", "x = 1"));
    }

    /// Two lines that merely sit side by side in a block are not one line edited. Marking the
    /// little they share — a space, a semicolon — would be noise, so neither is marked.
    #[test]
    fn unrelated_lines_are_not_marked() {
        assert_eq!(marked("return total;", "let y = compute(a, b);"), None);
    }

    /// Counting whitespace, these two would be 60% alike — all of it the indentation they share.
    #[test]
    fn a_shared_indent_does_not_make_lines_alike() {
        assert_eq!(marked("            return total;", "            let y = z;"), None);
    }

    /// A minified file's single line would make the subsequence table grow with its square. Past
    /// the cap, what lies between the common prefix and suffix is marked whole: still the right
    /// region, found in linear time.
    #[test]
    fn past_the_table_cap_the_whole_middle_is_marked() {
        let common = "common ".repeat(200);
        let old = format!("{common}{}end", "a c ".repeat(600));
        let new = format!("{common}{}end", "b c ".repeat(600));
        let (old_words, new_words) = (Words::new(&old), Words::new(&new));
        let (old_side, new_side) = pair(&old_words, &new_words).expect("half the text is shared");
        // One span from the end of the prefix to the start of ` c end` — where, under the cap,
        // the shared `c`s would have cut each side into 600.
        let middle = |line: &str| common.len()..line.len() - " c end".len();
        assert_eq!(old_side.changed(), [middle(&old)]);
        assert_eq!(new_side.changed(), [middle(&new)]);
    }

    #[test]
    fn tokens_are_words_whitespace_runs_and_single_marks() {
        assert_eq!(tokens("let x_1 = f(y);"), ["let", " ", "x_1", " ", "=", " ", "f", "(", "y", ")", ";"]);
        assert_eq!(tokens("a  \tb"), ["a", "  \t", "b"]);
        assert_eq!(tokens("->"), ["-", ">"], "each mark alone, so `->` to `=>` changes one character");
        assert_eq!(tokens("naïve café"), ["naïve", " ", "café"], "letters are letters in any script");
        assert!(tokens("").is_empty());
    }

    // ——— Pairing the lines of a block ———————————————————————————————————————————

    /// The feature itself: a removed line and its replacement, each with what changed standing
    /// out from the rest of the line.
    #[test]
    fn a_changed_line_pair_has_its_changes_highlighted() {
        assert_eq!(
            painted("@@ -1,2 +1,2 @@\n context\n-let x = 1;\n+let x = 42;\n"),
            "<hunk:@@ -1,2 +1,2 @@>\n context\n<old:-let x = ><OLD:1><old:;>\n<new:+let x = ><NEW:42><new:;>\n",
        );
    }

    /// A line inserted among edited ones pushes a pair apart; the pairing looks past it, and the
    /// inserted line is painted whole.
    #[test]
    fn an_inserted_line_does_not_break_the_pair_around_it() {
        assert_eq!(
            painted("@@ -1 +1,2 @@\n-x = 1\n+a new line\n+x = 2\n"),
            "<hunk:@@ -1 +1,2 @@>\n<old:-x = ><OLD:1>\n<new:+a new line>\n<new:+x = ><NEW:2>\n",
        );
    }

    /// A removed line that finds no partner claims none: the added lines stay for the removed
    /// lines after it.
    #[test]
    fn an_unmatched_removed_line_leaves_the_added_lines_to_the_next() {
        assert_eq!(
            painted("@@ -1,2 +1 @@\n-something else\n-x = 1\n+x = 2\n"),
            "<hunk:@@ -1,2 +1 @@>\n<old:-something else>\n<old:-x = ><OLD:1>\n<new:+x = ><NEW:2>\n",
        );
    }

    /// Both sides are printed in order, so pairs never cross: once `a` has paired with the later
    /// `a`, the `b` behind it is out of reach for the `b` before.
    #[test]
    fn pairs_never_cross() {
        assert_eq!(
            painted("@@ -1,2 +1,2 @@\n-a = 1\n-b = 2\n+b = 20\n+a = 10\n"),
            "<hunk:@@ -1,2 +1,2 @@>\n<old:-a = ><OLD:1>\n<old:-b = 2>\n<new:+b = 20>\n<new:+a = ><NEW:10>\n",
        );
    }

    /// The search for a partner reaches exactly [`LOOKAHEAD`] added lines: an insertion that long
    /// is still seen across, one line longer is not — the bound that keeps a huge block linear.
    #[test]
    fn a_partner_is_found_within_the_lookahead_and_not_beyond() {
        let block = |inserted: usize| {
            let filler: String = (0..inserted).map(|n| format!("+inserted line {n}\n")).collect();
            format!("@@ -1 +1,{} @@\n-x = 1\n{filler}+x = 2\n", inserted + 1)
        };
        let ends_paired = |diff: &str| painted(diff).ends_with("<new:+x = ><NEW:2>\n");
        assert!(ends_paired(&block(LOOKAHEAD - 1)), "the partner is the last line looked at");
        assert!(!ends_paired(&block(LOOKAHEAD)), "one further, and it is not looked at");
    }

    // ——— Reading the diff ————————————————————————————————————————————————————

    #[test]
    fn a_hunk_header_announces_both_sides_lines_and_where_its_range_ends() {
        let range = "@@ -12,3 +12,4 @@".len();
        assert_eq!(hunk_header(b"@@ -12,3 +12,4 @@ fn main() {"), Some(HunkHeader { old: 12..15, new: 12..16, range }));
        // A count git leaves out is 1; an empty side is 0 lines.
        assert_eq!(hunk_header(b"@@ -5 +0,0 @@"), Some(HunkHeader { old: 5..6, new: 0..0, range: 13 }));
        assert_eq!(hunk_header(b"@@@ -1 -1 +1 @@@"), None, "a combined diff's header is not ours");
        assert_eq!(hunk_header(b"@@ nonsense"), None);
    }

    /// Headers are painted as headers and a hunk's range as a range, while the function context
    /// git appends after the range stays plain — git's own layout of colour.
    #[test]
    fn headers_and_hunk_ranges_are_painted_and_function_context_is_not() {
        let diff = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@ fn main() {\n-a\n+b\n";
        assert_eq!(
            painted(diff),
            "<meta:diff --git a/f b/f>\n<meta:index 1..2 100644>\n<meta:--- a/f>\n<meta:+++ b/f>\n\
             <hunk:@@ -1 +1 @@> fn main() {\n<old:-a>\n<new:+b>\n",
        );
    }

    /// Only counting a hunk's lines tells a removed `-- x` (printed `--- x`) from the next file's
    /// `--- a/x` header — and a hunk ends exactly when its counts run out.
    #[test]
    fn a_hunk_ends_when_its_counts_run_out_and_not_before() {
        let diff = "@@ -1,2 +1 @@\n--- not a header\n-x\n+y\ndiff --git a/g b/g\n--- a/g\n+++ b/g\n";
        assert_eq!(
            painted(diff),
            "<hunk:@@ -1,2 +1 @@>\n<old:--- not a header>\n<old:-x>\n<new:+y>\n\
             <meta:diff --git a/g b/g>\n<meta:--- a/g>\n<meta:+++ b/g>\n",
        );
    }

    /// Under `diff.suppressBlankEmpty` git prints blank context as an empty line, not a lone
    /// space. It still counts toward the hunk, or the hunk would outlive its lines and read the
    /// next file's header as a removed line.
    #[test]
    fn an_empty_line_inside_a_hunk_is_blank_context() {
        let diff = "@@ -1,3 +1,3 @@\n a\n\n-b\n+c\ndiff --git a/g b/g\n--- a/g\n";
        assert_eq!(
            painted(diff),
            "<hunk:@@ -1,3 +1,3 @@>\n a\n\n<old:-b>\n<new:+c>\n<meta:diff --git a/g b/g>\n<meta:--- a/g>\n",
        );
    }

    /// `\ No newline at end of file` annotates the line above it: plain, and no obstacle to
    /// pairing the lines around it — even after the hunk's counts have run out.
    #[test]
    fn the_no_newline_marker_is_plain_and_pairing_reaches_across_it() {
        let diff = "@@ -1 +1 @@\n-let x = 1;\n\\ No newline at end of file\n+let x = 2;\n\\ No newline at end of file\n";
        assert_eq!(
            painted(diff),
            "<hunk:@@ -1 +1 @@>\n<old:-let x = ><OLD:1><old:;>\n\\ No newline at end of file\n\
             <new:+let x = ><NEW:2><new:;>\n\\ No newline at end of file\n",
        );
    }

    /// A line that is not UTF-8 — a Latin-1 file's — keeps its colour and is never cut: its
    /// characters cannot be told apart safely.
    #[test]
    fn a_line_that_is_not_utf8_is_painted_whole() {
        assert_eq!(
            painted(b"@@ -1 +1 @@\n-caf\xe9 = 1\n+caf\xe9 = 2\n"),
            "<hunk:@@ -1 +1 @@>\n<old:-caf\u{fffd} = 1>\n<new:+caf\u{fffd} = 2>\n",
        );
    }

    // ——— Line numbers ————————————————————————————————————————————————————————

    /// Each hunk line gets its number in the old file and in the new: a context line has both, a
    /// removed line only the old, an added line only the new. Headers stand outside the gutter.
    #[test]
    fn line_numbers_give_each_hunk_line_its_place_in_both_files() {
        let diff = "--- a/f\n+++ b/f\n@@ -3,3 +3,4 @@\n ctx\n-old\n+new\n+more\n end\n";
        assert_eq!(
            numbered(diff),
            "<meta:--- a/f>\n<meta:+++ b/f>\n<hunk:@@ -3,3 +3,4 @@>\n\
             <n:3 3 │> ctx\n<n:4   │><old:-old>\n<n:  4 │><new:+new>\n<n:  5 │><new:+more>\n<n:5 6 │> end\n",
        );
    }

    /// Numbers are right-aligned to the widest in the whole diff, so the gutter is one column
    /// from the first hunk to the last; a marker line gets a blank gutter to stay aligned.
    #[test]
    fn line_numbers_share_one_width_across_the_diff_and_markers_keep_the_gutter() {
        let diff = "@@ -1 +1 @@\n-a = 1\n+a = 2\n@@ -99,2 +99,2 @@\n b\n-c\n+d\n\\ No newline at end of file\n";
        assert_eq!(
            numbered(diff),
            "<hunk:@@ -1 +1 @@>\n<n:  1     │><old:-a = ><OLD:1>\n<n:      1 │><new:+a = ><NEW:2>\n\
             <hunk:@@ -99,2 +99,2 @@>\n<n: 99  99 │> b\n<n:100     │><old:-c>\n<n:    100 │><new:+d>\n\
             <n:        │>\\ No newline at end of file\n",
        );
    }

    // ——— Writing it ——————————————————————————————————————————————————————————

    /// Without colour the renderer adds nothing: the output is the input, byte for byte — what a
    /// pipe or a file receives, and the proof that no run is lost, doubled or reordered.
    #[test]
    fn without_colour_the_output_is_the_input() {
        let diff: &[u8] = b"diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@ ctx\n keep\r\n\
            -caf\xe9 = 1\n+caf\xe9 = 2\n-let x = 1;\n+let x = 2;\n\\ No newline at end of file";
        let mut out = Buffer::no_color();
        render(diff, false, &mut out).unwrap();
        assert_eq!(out.as_slice(), diff);
    }

    /// In colour, each run is written in its part's colour — checked against termcolor writing
    /// that colour itself, so the palette can change without this test noticing.
    #[test]
    fn in_colour_each_run_is_written_in_its_parts_colour() {
        let mut out = Buffer::ansi();
        render(b"@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n", true, &mut out).unwrap();
        let output = out.into_inner();
        let expected = [
            (Part::Hunk, "@@ -1 +1 @@"),
            (Part::Gutter, "1   │"),
            (Part::Removed, "-let x = "),
            (Part::RemovedChange, "1"),
            (Part::Added, "+let x = "),
            (Part::AddedChange, "2"),
        ];
        for (part, text) in expected {
            let mut alone = Buffer::ansi();
            alone.set_color(&spec(part)).unwrap();
            alone.write_all(text.as_bytes()).unwrap();
            alone.reset().unwrap();
            let alone = alone.into_inner();
            assert!(output.windows(alone.len()).any(|window| window == alone), "{part:?} is not painted as {part:?}");
        }
    }
}
