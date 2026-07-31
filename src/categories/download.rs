//! `dl_page_links` — scrape a webpage for links of given file types and fetch them with `curl`
//! (the same external the tool fetcher trusts).
//!
//! Video downloading is NOT here: `dl` wraps the `vidl` crate and so lives with the other
//! wrapped-repo commands ([`crate::categories::comfy_repos`]). The two share a prefix and a
//! notion of "downloading" and nothing else — one scrapes `<a href>`s, the other drives yt-dlp.

#[bashrs_macros::category(command = DownloadCommand, prefix = "dl_")]
mod commands {
    use std::collections::HashSet;

    use crate::support::exec::{capture_stdout, run_reporting};
    use clap::Args;

    /// Download every link of the given file types found in a webpage, into the current dir
    pub fn page_links(args: PageLinksArgs) {
        let PageLinksArgs { page, types, list } = args;
        let Some(html) = capture_stdout("curl", ["-fsSL", &page]) else {
            eprintln!("dl_page_links: could not fetch {page}");
            std::process::exit(1);
        };
        let exts: Vec<String> = types.iter().map(|ext| _normalize_ext(ext)).collect();
        let mut found = 0u32;
        let mut failed = 0u32;
        for (ext, links) in exts.iter().zip(_links_by_type(&html, &exts)) {
            if links.is_empty() {
                eprintln!("no .{ext} links in {page}");
                continue;
            }
            for link in links {
                found += 1;
                let url = _resolve_link(&page, &link);
                if list {
                    println!("{url}");
                } else {
                    println!("downloading {url}");
                    // `--no-progress-meter`, not `-s`: transfer noise off, real errors still on.
                    if !run_reporting("curl", ["-fSLO", "--retry", "2", "--no-progress-meter", &url]) {
                        failed += 1;
                    }
                }
            }
        }
        if found == 0 || failed > 0 {
            std::process::exit(1);
        }
    }

    #[derive(Args)]
    pub struct PageLinksArgs {
        /// The webpage to scan
        pub page: String,
        /// File types to fetch — e.g. `.png mp3 WEBP` (leading dot optional, case-insensitive)
        #[arg(required = true, num_args = 1..)]
        pub types: Vec<String>,
        /// Only print the resolved links, downloading nothing
        #[arg(short, long)]
        pub list: bool,
    }

    /// A type argument as a bare lowercase extension: `.PNG` and `png` both mean `png`.
    fn _normalize_ext(ext: &str) -> String {
        ext.trim_start_matches('.').to_lowercase()
    }

    /// One pass over the page: every `"`-delimited token is a link candidate (that's where HTML
    /// keeps its URLs), matched case-insensitively against every wanted type. Each hit lands
    /// once — under its first matching type — and whitespace-bearing tokens are prose, not URLs.
    fn _links_by_type(html: &str, exts: &[String]) -> Vec<Vec<String>> {
        let suffixes: Vec<String> = exts.iter().map(|ext| format!(".{ext}")).collect();
        let mut groups: Vec<Vec<String>> = vec![Vec::new(); exts.len()];
        let mut seen = HashSet::new();
        for token in html.split('"') {
            if token.is_empty() || token.chars().any(char::is_whitespace) {
                continue;
            }
            let lower = token.to_lowercase();
            if let Some(hit) = suffixes.iter().position(|suffix| lower.ends_with(suffix)) {
                if seen.insert(token.to_string()) {
                    groups[hit].push(token.to_string());
                }
            }
        }
        groups
    }

    /// Resolve one found link against the page's URL, the way a browser would: absolute links
    /// pass through; `//host/…` borrows the page's scheme; `/rooted` starts at the page's host;
    /// anything else is relative to the page's directory, with `./` and `dir/../` collapsed.
    fn _resolve_link(page: &str, link: &str) -> String {
        if link.contains("://") {
            return link.to_string();
        }
        let (scheme, rest) = page.split_once("://").unwrap_or(("https", page));
        if let Some(host_and_path) = link.strip_prefix("//") {
            return format!("{scheme}://{host_and_path}");
        }
        let host = rest.split('/').next().unwrap_or(rest);
        if link.starts_with('/') {
            return format!("{scheme}://{host}{link}");
        }
        let dir = match rest.rsplit_once('/') {
            Some((dir, _page_name)) => dir,
            None => rest, // the page sits at the host root
        };
        format!("{scheme}://{}", _squash(&format!("{dir}/{link}")))
    }

    /// Collapse `.`, empty (`//`), and `segment/..` path steps — never popping the leading
    /// segment (the host), so `../` can't climb above it.
    fn _squash(path: &str) -> String {
        let absolute = path.starts_with('/');
        let mut stack: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." if stack.len() > 1 => {
                    stack.pop();
                }
                ".." => {}
                other => stack.push(other),
            }
        }
        let joined = stack.join("/");
        if absolute {
            format!("/{joined}")
        } else {
            joined
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;



        #[test]
        fn extensions_normalize_to_bare_lowercase() {
            assert_eq!(_normalize_ext(".PNG"), "png");
            assert_eq!(_normalize_ext("mp3"), "mp3");
        }

        #[test]
        fn one_pass_groups_links_by_type_case_insensitively_and_once() {
            let html = r#"<a href="a.png"><img src="B.PNG"><a href="a.png">
                          <a href="song.mp3">a sentence ending in .png"quoted prose .mp3""#;
            let groups =
                _links_by_type(html, &["png".to_string(), "mp3".to_string()]);
            assert_eq!(groups[0], ["a.png", "B.PNG"], "case-insensitive, duplicates dropped");
            assert_eq!(groups[1], ["song.mp3"]);
        }

        #[test]
        fn whitespace_bearing_tokens_are_prose_not_links() {
            let groups = _links_by_type(r#"text "not a link .png" "real.png""#, &["png".to_string()]);
            assert_eq!(groups[0], ["real.png"]);
        }

        #[test]
        fn links_resolve_the_way_a_browser_would() {
            let page = "https://ex.com/gallery/2024/index.html";
            assert_eq!(_resolve_link(page, "http://other.org/x.png"), "http://other.org/x.png");
            assert_eq!(_resolve_link(page, "//cdn.ex.com/x.png"), "https://cdn.ex.com/x.png");
            assert_eq!(_resolve_link(page, "/root.png"), "https://ex.com/root.png");
            assert_eq!(_resolve_link(page, "img/x.png"), "https://ex.com/gallery/2024/img/x.png");
            assert_eq!(_resolve_link(page, "../up.png"), "https://ex.com/gallery/up.png");
            assert_eq!(
                _resolve_link(page, "../../../../too-far.png"),
                "https://ex.com/too-far.png",
                "`..` must never climb above the host"
            );
        }

        #[test]
        fn scheme_less_pages_resolve_as_https_and_root_pages_use_the_host_as_dir() {
            assert_eq!(_resolve_link("ex.com/dir/p.html", "x.png"), "https://ex.com/dir/x.png");
            assert_eq!(_resolve_link("https://ex.com", "x.png"), "https://ex.com/x.png");
        }

        #[test]
        fn file_urls_keep_their_absolute_path() {
            // The e2e tests (and quick local experiments) scan file:// pages; the resolved
            // links must keep the leading slash that `file://` needs.
            assert_eq!(
                _resolve_link("file:///tmp/site/page.html", "sub/a.txt"),
                "file:///tmp/site/sub/a.txt"
            );
        }

    }
}
