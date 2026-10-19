//! Two directories compared the way GNU diff compares them — the walk behind `diff` on two
//! directories ([`crate::categories::builtins`]). git walks directories too, but not GNU's way: it
//! always recurses, and prints a file on one side only in full, where GNU names it once however
//! much it holds (`Only in b: node_modules`). So bashrs walks, and git is handed only the pairs of
//! files that differ, each a `git diff --no-index` of its own.
//!
//! Nothing here runs git or prints: `compare` yields what it finds, in GNU's order, and the
//! command turns each finding into output. Links are compared, never followed — what git does with
//! a link, and a walk that follows links can loop.

use std::collections::{BTreeSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

/// How much of each file one step of a byte comparison reads: enough that system calls do not
/// dominate, and nothing beside a file large enough to need many steps.
const CHUNK: u64 = 64 * 1024;

/// What [`compare`] finds — one per name in either directory, and nothing for a name whose two
/// entries match.
#[derive(Debug)]
pub(crate) enum Finding {
    /// Two files, or two links, whose contents differ: the pair for git to diff.
    Differ(PathBuf, PathBuf),
    /// A difference GNU diff reports in a line of its own rather than as a diff: the line, newline
    /// included. Bytes, because a path need not be UTF-8.
    Line(Vec<u8>),
    /// What could not be read — GNU's "trouble", which makes the exit code 2.
    Unreadable(PathBuf, io::Error),
}

/// Compares two directories: one level deep, as GNU diff does, unless `recursive`, when each pair
/// of subdirectories is compared in place, all the way down. Names come in byte order, which is
/// GNU's order under the C locale.
///
/// Lazily: each entry is compared when the walk reaches it, so a caller printing findings as they
/// come shows the first difference in a large tree without waiting for the last.
pub(crate) fn compare(old: &Path, new: &Path, recursive: bool) -> Walk {
    let mut walk = Walk { recursive, levels: Vec::new(), pending: VecDeque::new() };
    walk.enter(old.to_path_buf(), new.to_path_buf());
    walk
}

/// A comparison in progress: the pairs of directories being walked, outermost first, and the
/// findings met but not yet handed out.
pub(crate) struct Walk {
    recursive: bool,
    levels: Vec<Level>,
    /// Findings met more than one at a time — trouble on both sides of a pair.
    pending: VecDeque<Finding>,
}

/// One pair of directories being walked, and the names in them still to compare, in order.
struct Level {
    old: PathBuf,
    new: PathBuf,
    names: std::vec::IntoIter<(OsString, Sides)>,
}

/// Which of two directories have a name.
enum Sides {
    Old,
    New,
    Both,
}

impl Iterator for Walk {
    type Item = Finding;

    fn next(&mut self) -> Option<Finding> {
        loop {
            if let Some(finding) = self.pending.pop_front() {
                return Some(finding);
            }
            let level = self.levels.last_mut()?;
            let Some((name, sides)) = level.names.next() else {
                self.levels.pop();
                continue;
            };
            let found = match sides {
                Sides::Old => Some(only_in(&level.old, &name)),
                Sides::New => Some(only_in(&level.new, &name)),
                Sides::Both => {
                    let (old, new) = (level.old.join(&name), level.new.join(&name));
                    self.compare_entries(old, new)
                }
            };
            if found.is_some() {
                return found;
            }
        }
    }
}

impl Walk {
    /// Starts walking a pair of directories — or reports whichever cannot be listed.
    fn enter(&mut self, old: PathBuf, new: PathBuf) {
        match both(names(&old), names(&new)) {
            Ok((old_names, new_names)) => {
                let names = merged(&old_names, &new_names).into_iter();
                self.levels.push(Level { old, new, names });
            }
            Err(unreadable) => self.pending.extend(unreadable),
        }
    }

    /// Compares what one name holds on each side: the finding when they differ — or `None`, also
    /// when they are two directories to walk, which are entered instead.
    fn compare_entries(&mut self, old: PathBuf, new: PathBuf) -> Option<Finding> {
        let (old_meta, new_meta) = match both(metadata(&old), metadata(&new)) {
            Ok(metas) => metas,
            Err(unreadable) => {
                self.pending.extend(unreadable);
                return None;
            }
        };
        if old_meta.is_dir() && new_meta.is_dir() {
            if self.recursive {
                self.enter(old, new);
                return None;
            }
            return Some(line(&[b"Common subdirectories: ", bytes(&old), b" and ", bytes(&new)]));
        }
        if !comparable(&old_meta) || !comparable(&new_meta) {
            return Some(line(&[
                b"File ", bytes(&old), b" is a ", kind(&old_meta).as_bytes(),
                b" while file ", bytes(&new), b" is a ", kind(&new_meta).as_bytes(),
            ]));
        }
        match same(&old, &new, &old_meta, &new_meta) {
            Ok(true) => None,
            Ok(false) => Some(Finding::Differ(old, new)),
            Err(unreadable) => Some(unreadable),
        }
    }
}

/// Whether an entry can be compared by reading it: a file by its bytes, a link by its target. A
/// directory cannot, beside something that is not one, and nor can anything special — a fifo
/// with no writer would block a read forever.
fn comparable(meta: &Metadata) -> bool {
    meta.is_file() || meta.is_symlink()
}

/// Whether two comparable entries hold the same thing: two files the same bytes, two links the
/// same target — a link is compared as git compares one. A file and a link never match.
fn same(old: &Path, new: &Path, old_meta: &Metadata, new_meta: &Metadata) -> Result<bool, Finding> {
    // Lengths first: most entries that differ differ in length, and that answer costs no read.
    if old_meta.file_type() != new_meta.file_type() || old_meta.len() != new_meta.len() {
        return Ok(false);
    }
    if old_meta.is_symlink() {
        let target = |link: &Path| fs::read_link(link).map_err(unreadable(link));
        return Ok(target(old)? == target(new)?);
    }
    same_bytes(old, new)
}

/// Whether two files of one length hold the same bytes, read a [`CHUNK`] at a time so that no
/// file is ever held whole.
fn same_bytes(old: &Path, new: &Path) -> Result<bool, Finding> {
    let open = |path: &Path| File::open(path).map_err(unreadable(path));
    let (mut old_file, mut new_file) = (open(old)?, open(new)?);
    let (mut old_chunk, mut new_chunk) = (Vec::new(), Vec::new());
    loop {
        old_chunk.clear();
        new_chunk.clear();
        let read = (&mut old_file).take(CHUNK).read_to_end(&mut old_chunk).map_err(unreadable(old))?;
        (&mut new_file).take(CHUNK).read_to_end(&mut new_chunk).map_err(unreadable(new))?;
        if old_chunk != new_chunk {
            return Ok(false);
        }
        if read == 0 {
            return Ok(true);
        }
    }
}

/// The names in a directory, sorted.
fn names(dir: &Path) -> Result<BTreeSet<OsString>, Finding> {
    let names = fs::read_dir(dir).and_then(|entries| entries.map(|entry| Ok(entry?.file_name())).collect());
    names.map_err(unreadable(dir))
}

/// Two directories' names as one sorted list, each with which sides have it.
fn merged(old: &BTreeSet<OsString>, new: &BTreeSet<OsString>) -> Vec<(OsString, Sides)> {
    let sides = |name| match (old.contains(name), new.contains(name)) {
        (true, false) => Sides::Old,
        (false, true) => Sides::New,
        _ => Sides::Both,
    };
    old.union(new).map(|name| (name.clone(), sides(name))).collect()
}

/// What an entry is, without following a link.
fn metadata(path: &Path) -> Result<Metadata, Finding> {
    fs::symlink_metadata(path).map_err(unreadable(path))
}

/// Both sides' answers — or whichever of them failed, in order. GNU reports trouble on either side
/// and goes on with the rest.
fn both<T>(old: Result<T, Finding>, new: Result<T, Finding>) -> Result<(T, T), Vec<Finding>> {
    match (old, new) {
        (Ok(old), Ok(new)) => Ok((old, new)),
        (old, new) => Err(old.err().into_iter().chain(new.err()).collect()),
    }
}

/// Turns a failure to read `path` into the finding that reports it.
fn unreadable(path: &Path) -> impl FnOnce(io::Error) -> Finding + '_ {
    move |err| Finding::Unreadable(path.to_path_buf(), err)
}

/// `Only in <dir>: <name>` — a name one directory has and the other lacks.
fn only_in(dir: &Path, name: &OsStr) -> Finding {
    line(&[b"Only in ", bytes(dir), b": ", name.as_bytes()])
}

/// One of GNU's report lines, from its parts.
fn line(parts: &[&[u8]]) -> Finding {
    let mut line = parts.concat();
    line.push(b'\n');
    Finding::Line(line)
}

fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// GNU diff's word for what an entry is, as its `File … is a …` line says it.
fn kind(meta: &Metadata) -> &'static str {
    let kind = meta.file_type();
    if kind.is_file() {
        if meta.len() == 0 { "regular empty file" } else { "regular file" }
    } else if kind.is_dir() {
        "directory"
    } else if kind.is_symlink() {
        "symbolic link"
    } else if kind.is_fifo() {
        "fifo"
    } else if kind.is_socket() {
        "socket"
    } else if kind.is_char_device() {
        "character special file"
    } else if kind.is_block_device() {
        "block special file"
    } else {
        "weird file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    /// Two trees, `a` and `b`, under a fresh temporary root — removed again when dropped, so a
    /// failing assertion leaves nothing behind.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        /// The trees from `(path, contents)` pairs: a path ending in `/` is a directory, contents
        /// `-> t` make a link to `t`, anything else is a file's text. Each test names its own root,
        /// since tests run in parallel within one process.
        fn new(test: &str, entries: &[(&str, &str)]) -> Self {
            let root = std::env::temp_dir().join(format!("bashrs_dir_diff_{test}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            for side in ["a", "b"] {
                fs::create_dir_all(root.join(side)).unwrap();
            }
            for (path, contents) in entries {
                let at = root.join(path);
                if path.ends_with('/') {
                    fs::create_dir_all(&at).unwrap();
                    continue;
                }
                fs::create_dir_all(at.parent().unwrap()).unwrap();
                match contents.strip_prefix("-> ") {
                    Some(target) => symlink(target, &at).unwrap(),
                    None => fs::write(&at, contents).unwrap(),
                }
            }
            Fixture { root }
        }

        fn walk(&self, recursive: bool) -> Walk {
            compare(&self.root.join("a"), &self.root.join("b"), recursive)
        }

        /// What [`compare`] finds between `a` and `b`, as text with paths relative to the root.
        fn found(&self, recursive: bool) -> Vec<String> {
            self.walk(recursive).map(|finding| self.text(finding)).collect()
        }

        /// A finding as text: GNU's line as it is, a pair that differs as `differ <old> <new>`, and
        /// a failure as `unreadable <path>`.
        fn text(&self, finding: Finding) -> String {
            let relative = |path: &Path| path.strip_prefix(&self.root).unwrap().display().to_string();
            match finding {
                Finding::Differ(old, new) => format!("differ {} {}", relative(&old), relative(&new)),
                Finding::Line(line) => String::from_utf8_lossy(&line).replace(&format!("{}/", self.root.display()), ""),
                Finding::Unreadable(path, _) => format!("unreadable {}", relative(&path)),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A small edit of a small project: one file changed, one removed, a directory added, and a
    /// change one level down.
    fn edited(test: &str) -> Fixture {
        Fixture::new(test, &[
            ("a/changed.txt", "let x = 1;\n"), ("b/changed.txt", "let x = 2;\n"),
            ("a/gone.txt", "gone\n"),
            ("b/new_dir/inside.txt", "new\n"),
            ("a/same.txt", "same\n"), ("b/same.txt", "same\n"),
            ("a/sub/deep.txt", "old\n"), ("b/sub/deep.txt", "new\n"),
        ])
    }

    // ——— The walk ————————————————————————————————————————————————————————————

    /// GNU's default: the top level only. Files are compared, a name on one side is named, and a
    /// subdirectory both sides have is listed rather than entered. A match says nothing.
    #[test]
    fn two_directories_are_compared_one_level_deep() {
        assert_eq!(edited("shallow").found(false), [
            "differ a/changed.txt b/changed.txt",
            "Only in a: gone.txt\n",
            "Only in b: new_dir\n",
            "Common subdirectories: a/sub and b/sub\n",
        ]);
    }

    /// `-r`: each pair of subdirectories is compared where GNU would list it — while a directory
    /// on one side only is still named once, never walked.
    #[test]
    fn recursion_compares_each_pair_of_subdirectories_in_place() {
        assert_eq!(edited("deep").found(true), [
            "differ a/changed.txt b/changed.txt",
            "Only in a: gone.txt\n",
            "Only in b: new_dir\n",
            "differ a/sub/deep.txt b/sub/deep.txt",
        ]);
    }

    #[test]
    fn names_come_in_byte_order() {
        let fixture = Fixture::new("order", &[("a/b", ""), ("a/B", ""), ("a/_", ""), ("a/1", "")]);
        assert_eq!(fixture.found(false), ["Only in a: 1\n", "Only in a: B\n", "Only in a: _\n", "Only in a: b\n"]);
    }

    /// An entry is compared when the walk reaches it, not before: what changes after the first
    /// finding was handed out is still seen — which is what lets output start at once.
    #[test]
    fn the_walk_compares_each_entry_only_when_it_reaches_it() {
        let fixture = Fixture::new("lazy", &[
            ("a/1.txt", "one\n"), ("b/1.txt", "two\n"),
            ("a/sub/x.txt", "same\n"), ("b/sub/x.txt", "same\n"),
        ]);
        let mut walk = fixture.walk(true);
        assert_eq!(walk.next().map(|finding| fixture.text(finding)).as_deref(), Some("differ a/1.txt b/1.txt"));
        fs::write(fixture.root.join("b/sub/x.txt"), "changed since\n").unwrap();
        let rest: Vec<String> = walk.map(|finding| fixture.text(finding)).collect();
        assert_eq!(rest, ["differ a/sub/x.txt b/sub/x.txt"]);
    }

    // ——— What counts as different ————————————————————————————————————————————————

    /// Bytes decide, not lengths: two files of one length can differ, and a difference past the
    /// first chunk read is found as surely as one in it.
    #[test]
    fn files_are_compared_by_their_bytes() {
        let long = "x".repeat(2 * CHUNK as usize);
        let fixture = Fixture::new("bytes", &[
            ("a/equal_length.txt", "one\n"), ("b/equal_length.txt", "two\n"),
            ("a/long_same.txt", &long), ("b/long_same.txt", &long),
            ("a/long_tail.txt", &format!("{long}a")), ("b/long_tail.txt", &format!("{long}b")),
        ]);
        assert_eq!(fixture.found(false), [
            "differ a/equal_length.txt b/equal_length.txt",
            "differ a/long_tail.txt b/long_tail.txt",
        ]);
    }

    /// A link is compared by where it points, as git compares one: never followed, so a link to a
    /// directory is not walked even under `-r`, and a link pointing nowhere is no trouble. A file
    /// replaced by a link differs, for git to show as a change of type.
    #[test]
    fn links_are_compared_by_target_and_never_followed() {
        let fixture = Fixture::new("links", &[
            ("a/dir_link", "-> sub"), ("b/dir_link", "-> sub"),
            ("a/file_or_link", "x"), ("b/file_or_link", "-> x"),
            ("a/moved_link", "-> one"), ("b/moved_link", "-> two"),
            ("a/same_link", "-> nowhere"), ("b/same_link", "-> nowhere"),
            ("a/sub/f.txt", "old\n"), ("b/sub/f.txt", "new\n"),
        ]);
        assert_eq!(fixture.found(true), [
            "differ a/file_or_link b/file_or_link",
            "differ a/moved_link b/moved_link",
            "differ a/sub/f.txt b/sub/f.txt",
        ]);
    }

    /// One name, two kinds of thing: nothing to diff, so GNU's line says what each is — the empty
    /// file in GNU's own words — and the directory is not walked, even under `-r`.
    #[test]
    fn a_file_against_a_directory_is_named_in_gnus_words() {
        let fixture = Fixture::new("kinds", &[
            ("a/empty", ""), ("b/empty/", ""),
            ("a/full", "1\n"), ("b/full/inside.txt", "1\n"),
        ]);
        assert_eq!(fixture.found(true), [
            "File a/empty is a regular empty file while file b/empty is a directory\n",
            "File a/full is a regular file while file b/full is a directory\n",
        ]);
    }

    /// A fifo is named, never read: with no writer, opening one blocks forever — so were this
    /// rule broken, the test would hang rather than fail.
    #[test]
    fn special_files_are_named_and_never_read() {
        let fixture = Fixture::new("fifo", &[]);
        for side in ["a", "b"] {
            let made = std::process::Command::new("mkfifo").arg(fixture.root.join(side).join("pipe")).status();
            assert!(made.is_ok_and(|status| status.success()), "mkfifo is needed to pose this");
        }
        assert_eq!(fixture.found(true), ["File a/pipe is a fifo while file b/pipe is a fifo\n"]);
    }

    /// Trouble is reported where it is met, and the walk goes on: GNU's way, so that one locked
    /// directory does not hide every difference after it.
    #[test]
    fn what_cannot_be_read_is_reported_and_the_rest_still_compared() {
        let fixture = Fixture::new("locked", &[
            ("a/locked/f.txt", "1\n"), ("b/locked/f.txt", "2\n"),
            ("a/z_after.txt", "1\n"), ("b/z_after.txt", "2\n"),
        ]);
        let locked = fixture.root.join("a/locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads through any permission, so there is nothing to pose.
        let readable_anyway = fs::read_dir(&locked).is_ok();
        let found = fixture.found(true);
        // Restored before asserting, so the fixture can be removed whatever happens.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if readable_anyway {
            return;
        }
        assert_eq!(found, ["unreadable a/locked", "differ a/z_after.txt b/z_after.txt"]);
    }

    /// Two directories that cannot be listed are both reported, and nothing else is found.
    #[test]
    fn trouble_on_both_sides_is_reported_for_each() {
        let fixture = Fixture::new("missing", &[]);
        let (old, new) = (fixture.root.join("no_old"), fixture.root.join("no_new"));
        let found: Vec<String> = compare(&old, &new, true).map(|finding| fixture.text(finding)).collect();
        assert_eq!(found, ["unreadable no_old", "unreadable no_new"]);
    }
}
