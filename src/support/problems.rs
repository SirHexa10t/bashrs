//! What a provisioning run could not do — collected as it goes, so `install-stainless` can repeat
//! the whole list at the very end of the compile, where it is read rather than scrolled past among
//! the progress lines that surround it when it happens.

use crate::support::doc_style;

/// The problems met so far. Each is printed the moment it is [`report`](Problems::report)ed — in
/// context, like every progress line around it — and kept for the [`summary`](Problems::summary).
#[derive(Default)]
pub struct Problems(Vec<String>);

impl Problems {
    /// Print `line` now and keep it for the summary.
    pub fn report(&mut self, line: impl Into<String>) {
        let line = line.into();
        eprintln!("{line}");
        self.0.push(line);
    }

    /// Every problem again, under one header, for the end of the run — `None` when there were
    /// none, so a clean run adds nothing.
    pub fn summary(&self) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        let mut text = doc_style::problematic("Provisioning problems — the compile went on without them:");
        for line in &self.0 {
            text.push_str("\n  ");
            text.push_str(line);
        }
        Some(text)
    }

    /// What has been reported, in order.
    #[cfg(test)]
    pub(crate) fn lines(&self) -> &[String] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_run_has_nothing_to_repeat() {
        assert_eq!(Problems::default().summary(), None);
    }

    /// Content first — every problem, in the order it happened — then the layout: the header on
    /// a line of its own, each problem indented beneath it.
    #[test]
    fn the_summary_repeats_every_problem_in_order_under_one_header() {
        let mut problems = Problems::default();
        problems.report("tools: could not check for a newer ffmpeg build");
        problems.report(String::from("stainless/x: git failed"));
        assert_eq!(problems.lines(), ["tools: could not check for a newer ffmpeg build", "stainless/x: git failed"]);

        let summary = problems.summary().expect("two problems");
        let lines: Vec<&str> = summary.lines().collect();
        assert!(lines[0].contains("Provisioning problems"), "the header leads: {summary}");
        assert_eq!(lines[1..], ["  tools: could not check for a newer ffmpeg build", "  stainless/x: git failed"]);
    }
}
