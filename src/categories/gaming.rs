//! Gaming commands (`gm_*`): the games on this computer. Where the itch app keeps one
//! (`gm_id_itch`) and where Steam does (`gm_id_steam`), and running a Windows game through Proton, in
//! a sandbox (`gm_run_exe`, over the Protons `gm_protons` lists).
//!
//! **itch.** The itch app keeps everything in its data folder (`~/.config/itch`): its database
//! (`db/butler.db`), which knows each game the app has seen and each install it made, in any
//! install location; the installs, each holding a receipt that names its game
//! (`.itch/receipt.json.gz`), read as well, for when the database can't be; and the saves: an HTML5
//! game's are its browser storage, a partition named for its id (`Partitions/gamesession_<id>`,
//! kept after an uninstall); a native game's, while the app's sandbox runs it, are in a private
//! home inside its install (`.itch/home`). itch.io itself is asked only about a game this computer
//! knows nothing of.
//!
//! **Steam.** Steam keeps a game in one of its libraries — its own folder, and those its
//! `libraryfolders.vdf` lists ([`crate::tools::proton`] reads where they are): the install in
//! `steamapps/common`, in the folder the game's manifest names (`steamapps/appmanifest_<id>.acf`),
//! and its Proton prefix in `steamapps/compatdata/<id>`. In Steam's own folder, `userdata` holds a
//! folder per account: each game's Steam Cloud files (`<account>/<id>`), and the non-Steam games the
//! account added (`<account>/config/shortcuts.vdf`, binary KeyValues), each under the id Steam gave
//! it, kept in that file — none rebuilds it from the game's path or name. All is read off this
//! computer: a store page carries the id.
//!
//! **Proton.** `gm_run_exe` runs a game through the newest Proton there is ([`crate::tools::proton`]),
//! with the verb Steam uses, inside a bubblewrap sandbox: the system read-only, the game's folder,
//! and one folder of the game's own — its Wine prefix and the home the sandbox gives it, one per
//! game in `~/Games/manual_run_proton_prefixes`, outside `~/.bashrs` — none of which may be, or
//! hold, the user's own folders or a whole filesystem. No user namespaces of the game's own, and a
//! syscall filter ([`crate::support::seccomp`]), set in place inside the sandbox by this binary,
//! started again there before Proton. No network unless asked; Wine's own Wayland driver unless
//! asked for X11. A
//! computer with no Proton gets the latest one bundled first — the only time the command itself
//! needs the network.

#[bashrs_macros::category(command = GamingCommand, prefix = "gm_")]
mod commands {
    use std::collections::{BTreeSet, HashSet};
    use std::ffi::{OsStr, OsString};
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use clap::Args;

    use crate::conf::config_file;
    use crate::support::args::NoArgs;
    use crate::support::comfy_repos;
    use crate::support::doc_style::_header;
    use crate::support::exec;
    use crate::support::seccomp;
    use crate::support::shell_dialect::Shell;
    use crate::support::shell_quote::{self, Quoting};
    use crate::tools::{self, proton, proton::Proton};

    // --- gm_id_itch -----------------------------------------------------------------------------

    /// Print where the itch app keeps a game: its install folder, its saves' folder, then its
    /// itch.io id, a line each, for those that exist
    ///
    /// The game is named by its page (https://creator.itch.io/game, the https:// optional) or, where
    /// that tells it apart on this computer, by its name alone (game). What this computer knows
    /// answers first: the itch app's database, and the installs it made in every install location.
    /// Only about a page it knows nothing of is itch.io asked. The saves are an HTML5 game's browser
    /// storage (kept after an uninstall), or the private home the app's sandbox gives a native
    /// game, in its install.
    pub fn id_itch(args: IdItchArgs) {
        let Some(query) = Query::parse(&args.game) else {
            eprintln!("gm_id_itch: {} names no itch.io game (give its page, creator.itch.io/game, or its name)", args.game.trim());
            std::process::exit(1);
        };
        let itch = _itch_dir(std::env::var_os("XDG_CONFIG_HOME").as_deref(), &crate::conf::home());
        let (id, installs) = match Known::gather(&itch).find(&query) {
            Found::One { id, installs } => (id, installs),
            Found::Several(pages) => {
                let pages: Vec<String> = pages.iter().map(GamePage::url).collect();
                eprintln!("gm_id_itch: more than one game here is called {} — give the page of the one you mean: {}", args.game.trim(), pages.join(", "));
                std::process::exit(1);
            }
            Found::Nothing => match &query {
                Query::Name(name) => {
                    let slug = name.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c));
                    let example = if slug { format!("creator.itch.io/{name}") } else { "creator.itch.io/game".to_string() };
                    eprintln!("gm_id_itch: no game called {} is on this computer — give its page's address ({example})", args.game.trim());
                    std::process::exit(1);
                }
                Query::Page(page) => _published_id(page).map(|id| (id, Vec::new())).unwrap_or_else(|why| {
                    eprintln!("gm_id_itch: {} isn't on this computer, and {why}", page.url());
                    std::process::exit(1);
                }),
            },
        };
        for line in _lines(&itch, &installs, id) {
            println!("{line}");
        }
    }

    /// The game to look up.
    #[derive(Args)]
    pub struct IdItchArgs {
        /// The game's itch.io page (https://creator.itch.io/game, or creator.itch.io/game), or its
        /// name alone (game) where that's enough to find it on this computer
        pub game: String,
    }

    /// How a game is named: by its page, or by its name alone (the last part of its page's
    /// address, its title, or its install's folder).
    #[derive(Debug, PartialEq)]
    enum Query {
        Page(GamePage),
        Name(String),
    }

    impl Query {
        /// What `input` names: a page where it reads as one, else a name, letter case ignored.
        /// `None` for an address that is no game page (anything with a `/` in it that isn't one),
        /// or for nothing.
        fn parse(input: &str) -> Option<Self> {
            let input = input.trim();
            if let Some(page) = GamePage::parse(input) {
                Some(Query::Page(page))
            } else if input.is_empty() || input.contains('/') {
                None
            } else {
                Some(Query::Name(input.to_lowercase()))
            }
        }
    }

    /// An itch.io game page, as the two parts that tell one game from another: the creator's
    /// subdomain and the game's name in the path.
    #[derive(Debug, Clone, PartialEq)]
    struct GamePage {
        creator: String,
        game: String,
    }

    impl GamePage {
        /// The page an address names, as pasted: the scheme optional; the query, the fragment and
        /// any deeper path (a devlog, the purchase page) dropped; letter case ignored. `None` for
        /// an address that is no game page: not on a creator's `.itch.io` subdomain, or with no
        /// game in its path.
        fn parse(address: &str) -> Option<Self> {
            let address = address.trim();
            let rest = address.split_once("://").map_or(address, |(_scheme, rest)| rest);
            let rest = rest.split(['?', '#']).next()?;
            let mut parts = rest.split('/');
            let host = parts.next()?.to_ascii_lowercase();
            let creator = host.strip_suffix(".itch.io")?;
            let game = parts.next()?.to_ascii_lowercase();
            if creator.is_empty() || creator.contains('.') || game.is_empty() {
                return None;
            }
            Some(Self { creator: creator.to_string(), game })
        }

        /// The page's address, as itch.io writes it.
        fn url(&self) -> String {
            format!("https://{}.itch.io/{}", self.creator, self.game)
        }
    }

    /// What this computer knows of itch games: each game the app has seen, and each install, by its
    /// game's id and its folder.
    #[derive(Debug, Default)]
    struct Known {
        games: Vec<KnownGame>,
        installs: Vec<(u64, PathBuf)>,
    }

    /// A game the app has seen: its id, its page, and its title.
    #[derive(Debug)]
    struct KnownGame {
        id: u64,
        page: GamePage,
        title: String,
    }

    /// What a [`Query`] finds in what's [`Known`].
    #[derive(Debug, PartialEq)]
    enum Found {
        /// One game: its id, and the folders of its installs that are there.
        One { id: u64, installs: Vec<PathBuf> },
        /// More than one game goes by the name: their pages.
        Several(Vec<GamePage>),
        Nothing,
    }

    impl Known {
        /// What the itch app at `itch` keeps track of: its database (games and installs, where
        /// python can read it), and the receipts in each install location (the database's, and the
        /// default one, `apps/`), which tell of each install again, should the database be
        /// unreadable.
        fn gather(itch: &Path) -> Self {
            let database = _database(&itch.join(DATABASE)).unwrap_or_default();
            let mut known = Known::default();
            for (id, url, title) in &database.games {
                known.add_game(*id, url, title);
            }
            for (id, folder) in database.installs {
                known.add_install(id, folder);
            }
            let mut locations = HashSet::new();
            for location in std::iter::once(itch.join(APPS_DIR)).chain(database.locations) {
                if locations.insert(location.clone()) {
                    for (folder, game) in _receipts(&location) {
                        known.add_game(game.id, &game.url, &game.title);
                        known.add_install(game.id, folder);
                    }
                }
            }
            known
        }

        /// A game, by its id, its page's address and its title; once.
        fn add_game(&mut self, id: u64, url: &str, title: &str) {
            if let Some(page) = GamePage::parse(url) {
                if !self.games.iter().any(|known| known.id == id) {
                    self.games.push(KnownGame { id, page, title: title.trim().to_lowercase() });
                }
            }
        }

        /// An install, by its game's id and its folder; once.
        fn add_install(&mut self, id: u64, folder: PathBuf) {
            if !self.installs.iter().any(|(_, known)| *known == folder) {
                self.installs.push((id, folder));
            }
        }

        /// The game `query` names: by its page; or by its name, which is a page's last part, a
        /// title, or an install's folder, letter case ignored.
        fn find(&self, query: &Query) -> Found {
            let ids: BTreeSet<u64> = match query {
                Query::Page(page) => self.games.iter().filter(|known| known.page == *page).map(|known| known.id).collect(),
                Query::Name(name) => {
                    let by_page = self.games.iter().filter(|known| known.page.game == *name || known.title == *name).map(|known| known.id);
                    let by_folder = self
                        .installs
                        .iter()
                        .filter(|(_, folder)| folder.file_name().is_some_and(|folder| folder.to_string_lossy().to_lowercase() == *name))
                        .map(|(id, _)| *id);
                    by_page.chain(by_folder).collect()
                }
            };
            match ids.len() {
                0 => Found::Nothing,
                1 => {
                    let id = *ids.first().unwrap_or(&0);
                    Found::One { id, installs: self.installs_of(id) }
                }
                _ => Found::Several(ids.iter().filter_map(|id| self.page_of(*id)).collect()),
            }
        }

        /// The folders of the game's installs that are there, in path order.
        fn installs_of(&self, id: u64) -> Vec<PathBuf> {
            let mut folders: Vec<PathBuf> =
                self.installs.iter().filter(|(game, folder)| *game == id && folder.is_dir()).map(|(_, folder)| folder.clone()).collect();
            folders.sort();
            folders
        }

        /// The game's page.
        fn page_of(&self, id: u64) -> Option<GamePage> {
            self.games.iter().find(|known| known.id == id).map(|known| known.page.clone())
        }
    }

    /// The game a receipt names: its id, its page's address and its title.
    #[derive(Debug, PartialEq)]
    struct Receipt {
        id: u64,
        url: String,
        title: String,
    }

    /// The installs in an install location, by their receipts: each one's folder and the game its
    /// receipt names, in folder order. A folder with no receipt, or one that doesn't read as the
    /// app's, is no install of a game (the app keeps its downloads in `apps/` too).
    fn _receipts(location: &Path) -> Vec<(PathBuf, Receipt)> {
        let Ok(entries) = std::fs::read_dir(location) else {
            return Vec::new();
        };
        let mut found: Vec<(PathBuf, Receipt)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let folder = entry.path();
                let receipt = _receipt(&folder.join(RECEIPT))?;
                Some((folder, receipt))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    /// The game the receipt at `path` names.
    fn _receipt(path: &Path) -> Option<Receipt> {
        let mut json = String::new();
        flate2::read::GzDecoder::new(std::fs::File::open(path).ok()?).read_to_string(&mut json).ok()?;
        _game_in(&json)
    }

    /// The `game` record of a receipt's JSON: its id and page's address, which it must have, and its
    /// title.
    fn _game_in(json: &str) -> Option<Receipt> {
        let receipt: serde_json::Value = serde_json::from_str(json).ok()?;
        let game = receipt.get("game")?;
        Some(Receipt {
            id: game.get("id")?.as_u64()?,
            url: game.get("url")?.as_str()?.to_string(),
            title: game.get("title").and_then(serde_json::Value::as_str).unwrap_or_default().to_string(),
        })
    }

    /// What the itch app's database tells: each game it has seen (id, page address and title), each
    /// install (its game's id and its folder), and each install location's folder.
    #[derive(Debug, Default, PartialEq)]
    struct Database {
        games: Vec<(u64, String, String)>,
        installs: Vec<(u64, PathBuf)>,
        locations: Vec<PathBuf>,
    }

    /// The itch app's database at `db`, read through python's `sqlite3`, the way vidl reads a
    /// browser's cookie store: read-only, and quietly. `None` where there's no database, no python
    /// to read it, or an answer that doesn't read; the receipts still tell of the installs then.
    fn _database(db: &Path) -> Option<Database> {
        if !db.is_file() {
            return None;
        }
        let output = Command::new(tools::resolve("python3"))
            .args(["-I", "-c", DATABASE_QUERY])
            .arg(db)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output.status.success().then(|| _database_from(&String::from_utf8_lossy(&output.stdout))).flatten()
    }

    /// [`_database`]'s answer, read from the JSON [`DATABASE_QUERY`] prints.
    fn _database_from(json: &str) -> Option<Database> {
        let answer: serde_json::Value = serde_json::from_str(json).ok()?;
        let list = |key: &str| answer.get(key).and_then(serde_json::Value::as_array).cloned().unwrap_or_default();
        let text = |row: &serde_json::Value, at: usize| row.get(at).and_then(serde_json::Value::as_str).map(str::to_string);
        Some(Database {
            games: list("games")
                .iter()
                .filter_map(|row| Some((row.get(0)?.as_u64()?, text(row, 1)?, text(row, 2).unwrap_or_default())))
                .collect(),
            installs: list("installs").iter().filter_map(|row| Some((row.get(0)?.as_u64()?, PathBuf::from(text(row, 1)?)))).collect(),
            locations: list("locations").iter().filter_map(|path| path.as_str().map(PathBuf::from)).collect(),
        })
    }

    /// The query [`_database`] runs, the database's path its argument: the games with their pages
    /// and titles, the installs with their folders (a custom one, else the install location's folder joined with
    /// the install's folder name), and the install locations' folders, as JSON. The database is
    /// opened read-only, by a URI so any path reads, with its write-ahead log: the app may be
    /// running.
    const DATABASE_QUERY: &str = r#"import json, os, pathlib, sqlite3, sys
db = sqlite3.connect(pathlib.Path(sys.argv[1]).as_uri() + "?mode=ro", uri=True)
locations = dict(db.execute("select id, path from install_locations where path is not null"))
installs = []
for game, location, folder, custom in db.execute("select game_id, install_location_id, install_folder_name, custom_install_folder from caves"):
    if custom:
        installs.append([game, custom])
    elif location in locations and folder:
        installs.append([game, os.path.join(locations[location], folder)])
games = db.execute("select id, url, title from games where url is not null").fetchall()
print(json.dumps({"games": games, "installs": installs, "locations": list(locations.values())}))
"#;

    /// The game's id as itch.io publishes it, in the page's `data.json`, or why there's none: no
    /// way to reach itch.io, or no such game there.
    fn _published_id(page: &GamePage) -> Result<u64, String> {
        let data = format!("{}/data.json", page.url());
        match exec::capture_with_code("curl", ["-sL", "--max-time", "20", "-w", "\n%{http_code}", &data]) {
            Some((code, out)) => _published_answer(code, &String::from_utf8_lossy(&out)),
            None => Err("curl, which would ask itch.io, couldn't run".to_string()),
        }
    }

    /// What itch.io's answer for a `data.json` means: curl's exit code, and its output (the body,
    /// then a line of the HTTP status). A curl that failed never reached itch.io: no connection, no
    /// name lookup, a timeout.
    fn _published_answer(code: i32, out: &str) -> Result<u64, String> {
        if code != 0 {
            return Err("itch.io couldn't be reached (is this computer online?)".to_string());
        }
        let (body, status) = out.rsplit_once('\n').unwrap_or(("", out));
        match status.trim() {
            "200" => _id_in(body).ok_or_else(|| "itch.io's answer had no id in it".to_string()),
            "404" => Err("itch.io has no game there".to_string()),
            status => Err(format!("itch.io answered {status}")),
        }
    }

    /// The `id` in a page's `data.json`.
    fn _id_in(json: &str) -> Option<u64> {
        serde_json::from_str::<serde_json::Value>(json).ok()?.get("id")?.as_u64()
    }

    /// What `gm_id_itch` prints, in order: each install's folder; the folders the saves are in (the
    /// browser storage partition named for the id, then each install's sandbox home), those that
    /// exist; the id.
    fn _lines(itch: &Path, installs: &[PathBuf], id: u64) -> Vec<String> {
        let partition = itch.join(PARTITIONS_DIR).join(format!("{PARTITION_PREFIX}{id}"));
        let homes = installs.iter().map(|install| install.join(SANDBOX_HOME));
        let saves = std::iter::once(partition).chain(homes).filter(|folder| folder.is_dir());
        installs
            .iter()
            .cloned()
            .chain(saves)
            .map(|folder| folder.display().to_string())
            .chain(std::iter::once(id.to_string()))
            .collect()
    }

    /// The itch app's data folder, where Electron keeps an app's data on Linux: under
    /// `$XDG_CONFIG_HOME`, else under `~/.config`. A relative `XDG_CONFIG_HOME` is ignored, as the
    /// XDG spec says to.
    fn _itch_dir(xdg_config_home: Option<&OsStr>, home: &Path) -> PathBuf {
        match xdg_config_home.map(Path::new).filter(|dir| dir.is_absolute()) {
            Some(dir) => dir.join(ITCH_DIR),
            None => home.join(".config").join(ITCH_DIR),
        }
    }

    /// The itch app's data folder's name in the config directory (Electron names it for the app).
    const ITCH_DIR: &str = "itch";
    /// The app's database, in its data folder.
    const DATABASE: &str = "db/butler.db";
    /// The install location the app installs into by default ("appdata"), in its data folder.
    const APPS_DIR: &str = "apps";
    /// The receipt the app writes into each install, naming the game: gzipped JSON.
    const RECEIPT: &str = ".itch/receipt.json.gz";
    /// The private home the app's sandbox mounts over `$HOME` for a native game, in its install.
    const SANDBOX_HOME: &str = ".itch/home";
    /// Where Electron keeps the app's browser storage partitions, in its data folder.
    const PARTITIONS_DIR: &str = "Partitions";
    /// An HTML5 game's partition name before its id: the app runs one in `persist:gamesession_<id>`.
    const PARTITION_PREFIX: &str = "gamesession_";

    // --- gm_id_steam ----------------------------------------------------------------------------

    /// Print where Steam keeps a game: its install folder, its saves' folders, then its Steam app
    /// id, a line each, for those that exist
    ///
    /// The game is named by its store page (https://store.steampowered.com/app/620/Portal_2, the
    /// https:// optional), by its app id (620), or, where that tells it apart on this computer, by
    /// its name alone: its title in Steam, or its install's folder. A non-Steam game added to Steam
    /// is found the same ways, by the id Steam gave it. Everything is read off this computer, in
    /// every Steam library; a page needs no lookup, as it carries the id. The saves are the game's
    /// Proton prefix, in its library's steamapps/compatdata (the folder gm_run_exe --prefix takes),
    /// and its Steam Cloud folder, in each account's folder in Steam's userdata.
    pub fn id_steam(args: IdSteamArgs) {
        let Some(query) = SteamQuery::parse(&args.game) else {
            eprintln!("gm_id_steam: {} names no Steam game (give its store page, store.steampowered.com/app/<id>, its app id, or its name)", args.game.trim());
            std::process::exit(1);
        };
        let known = SteamKnown::gather(&crate::conf::home());
        let (id, installs) = match known.find(&query) {
            SteamFound::One { id, installs } => (id, installs),
            SteamFound::Several(games) => {
                let games: Vec<String> = games.iter().map(|(id, title)| format!("{id} ({title})")).collect();
                eprintln!("gm_id_steam: more than one game here is called {} — give the id of the one you mean: {}", args.game.trim(), games.join(", "));
                std::process::exit(1);
            }
            SteamFound::Nothing => {
                eprintln!("gm_id_steam: no game called {} is on this computer — give its store page (store.steampowered.com/app/<id>) or its app id", args.game.trim());
                std::process::exit(1);
            }
        };
        for line in _steam_lines(&known, &installs, id) {
            println!("{line}");
        }
    }

    /// The game to look up.
    #[derive(Args)]
    pub struct IdSteamArgs {
        /// The game's Steam store page (https://store.steampowered.com/app/620/Portal_2, or without the
        /// https://), its app id (620), or its name alone (its title, or its install's folder) where
        /// that's enough to find it on this computer; a name of digits alone is read as an id
        pub game: String,
    }

    /// How a Steam game is named: by its id (as such, or in a page or a link), or by its name alone
    /// (its title, or its install's folder).
    #[derive(Debug, PartialEq)]
    enum SteamQuery {
        Id(u32),
        Name(String),
    }

    impl SteamQuery {
        /// What `input` names: the id it is or holds ([`_steam_id_in`]), else a name, letter case
        /// ignored. `None` for an address that holds no game's id (anything with a `/` in it that
        /// isn't one), or for nothing.
        fn parse(input: &str) -> Option<Self> {
            let input = input.trim();
            if let Some(id) = _steam_id_in(input) {
                Some(SteamQuery::Id(id))
            } else if input.is_empty() || input.contains('/') {
                None
            } else {
                Some(SteamQuery::Name(input.to_lowercase()))
            }
        }
    }

    /// The app id `input` is or holds: a number ([`_app_id_of`]); a store or community page, the
    /// scheme optional (`store.steampowered.com/app/620/Portal_2`, `steamcommunity.com/app/620`),
    /// the query, the fragment and whatever follows the id dropped; or a `steam://` link that runs a
    /// game (`steam://rungameid/620`, as Steam's desktop shortcuts have it, or `steam://run/620`).
    fn _steam_id_in(input: &str) -> Option<u32> {
        if let Some(link) = input.strip_prefix("steam://") {
            let (verb, rest) = link.split_once('/')?;
            let runs = verb.eq_ignore_ascii_case("rungameid") || verb.eq_ignore_ascii_case("run");
            return if runs { _app_id_of(rest.split('/').next().unwrap_or_default()) } else { None };
        }
        if !input.is_empty() && input.bytes().all(|byte| byte.is_ascii_digit()) {
            return _app_id_of(input);
        }
        let rest = input.split_once("://").map_or(input, |(_scheme, rest)| rest);
        let mut parts = rest.split(['?', '#']).next()?.split('/');
        let host = parts.next()?.to_ascii_lowercase();
        if !STEAM_PAGE_HOSTS.contains(&host.as_str()) || !parts.next()?.eq_ignore_ascii_case("app") {
            return None;
        }
        _app_id_of(parts.next()?)
    }

    /// The app id a game id in decimal gives: an app's is its own; the 64-bit one Steam runs a
    /// non-Steam game by holds the game's id in its upper half, and in its lower Steam's mark for
    /// one ([`SHORTCUT_GAME_ID`]). `None` for anything else, and for 0.
    fn _app_id_of(digits: &str) -> Option<u32> {
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number: u64 = digits.parse().ok()?;
        match u32::try_from(number) {
            Ok(id) => (id != 0).then_some(id),
            Err(_) if number & 0xFFFF_FFFF == SHORTCUT_GAME_ID => u32::try_from(number >> 32).ok(),
            Err(_) => None,
        }
    }

    /// What this computer knows of Steam games: every Steam library (each Steam's own folder, and
    /// those it lists), every account's folder in each Steam's `userdata`, and the games — Steam's,
    /// from the manifests in its libraries, and the non-Steam ones the accounts added.
    #[derive(Debug, Default)]
    struct SteamKnown {
        libraries: Vec<PathBuf>,
        accounts: Vec<PathBuf>,
        games: Vec<SteamGame>,
    }

    /// A game Steam knows: its app id, its title there, the folder it's installed in (an app's,
    /// under `steamapps/common`; a non-Steam game's, its .exe's), and a non-Steam game's .exe.
    #[derive(Debug, Clone, PartialEq)]
    struct SteamGame {
        id: u32,
        title: String,
        folder: Option<PathBuf>,
        exe: Option<PathBuf>,
    }

    impl SteamGame {
        /// Whether the .exe at `exe` (a canonical path) runs this game: it's a non-Steam game's own
        /// .exe, or one in an app's install folder.
        fn runs(&self, exe: &Path) -> bool {
            match (&self.exe, &self.folder) {
                (Some(own), _) => _canonical(own) == exe,
                (None, Some(folder)) => exe.starts_with(_canonical(folder)),
                (None, None) => false,
            }
        }
    }

    /// What a [`SteamQuery`] finds in what's [`SteamKnown`].
    #[derive(Debug, PartialEq)]
    enum SteamFound {
        /// One game: its id, and the folders it's installed in that are there.
        One { id: u32, installs: Vec<PathBuf> },
        /// More than one game goes by the name: their ids and titles.
        Several(Vec<(u32, String)>),
        Nothing,
    }

    impl SteamKnown {
        /// What the Steams under `home` keep: their libraries ([`proton::libraries`]), each once
        /// however it's reached (a Steam's own folder is listed in its `libraryfolders.vdf` too);
        /// the accounts in their `userdata`; the apps the libraries' manifests name, and the
        /// non-Steam games in each account's `shortcuts.vdf`.
        fn gather(home: &Path) -> Self {
            let mut known = SteamKnown::default();
            let mut seen = HashSet::new();
            for root in proton::steam_roots(home) {
                for library in proton::libraries(&root) {
                    if seen.insert(std::fs::canonicalize(&library).unwrap_or_else(|_| library.clone())) {
                        known.libraries.push(library);
                    }
                }
                known.accounts.extend(_accounts(&root.join(USERDATA_DIR)));
            }
            for library in &known.libraries {
                known.games.extend(_manifests(library));
            }
            for account in &known.accounts {
                known.games.extend(_shortcuts_in(&std::fs::read(account.join(SHORTCUTS_FILE)).unwrap_or_default()));
            }
            known
        }

        /// The game `query` names: by its id, whatever is known of it here (an id is enough to look
        /// for its folders); or by its name — a title, or an install's folder — letter case ignored,
        /// a non-Steam game's title with or without the `.exe` Steam leaves on it.
        fn find(&self, query: &SteamQuery) -> SteamFound {
            let name = match query {
                SteamQuery::Id(id) => return SteamFound::One { id: *id, installs: self.installs_of(*id) },
                SteamQuery::Name(name) => name.as_str(),
            };
            let called = |game: &&SteamGame| {
                let title = game.title.trim().to_lowercase();
                let folder = game.folder.as_deref().and_then(Path::file_name).map(|folder| folder.to_string_lossy().to_lowercase());
                title == name || title.strip_suffix(".exe") == Some(name) || folder.as_deref() == Some(name)
            };
            let ids: BTreeSet<u32> = self.games.iter().filter(called).map(|game| game.id).collect();
            match ids.len() {
                0 => SteamFound::Nothing,
                1 => {
                    let id = *ids.first().unwrap_or(&0);
                    SteamFound::One { id, installs: self.installs_of(id) }
                }
                _ => SteamFound::Several(ids.iter().map(|&id| (id, self.title_of(id))).collect()),
            }
        }

        /// The folders the game is installed in that are there, in path order.
        fn installs_of(&self, id: u32) -> Vec<PathBuf> {
            let mut folders: Vec<PathBuf> =
                self.games.iter().filter(|game| game.id == id).filter_map(|game| game.folder.clone()).filter(|folder| folder.is_dir()).collect();
            folders.sort();
            folders.dedup();
            folders
        }

        /// The game's title, as Steam shows it.
        fn title_of(&self, id: u32) -> String {
            self.games.iter().find(|game| game.id == id).map(|game| game.title.trim().to_string()).unwrap_or_default()
        }
    }

    /// The accounts' folders in a Steam's `userdata`, each named for its account's id, in order.
    fn _accounts(userdata: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(userdata) else {
            return Vec::new();
        };
        let mut accounts: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().as_bytes().iter().all(u8::is_ascii_digit))
            .map(|entry| entry.path())
            .filter(|folder| folder.is_dir())
            .collect();
        accounts.sort();
        accounts
    }

    /// The apps installed in the Steam library at `library`, by their manifests
    /// (`steamapps/appmanifest_<id>.acf`), in id order: each one's id, title and install folder. A
    /// manifest that doesn't read, or names no id, is no app.
    fn _manifests(library: &Path) -> Vec<SteamGame> {
        let steamapps = library.join(STEAMAPPS_DIR);
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            return Vec::new();
        };
        let mut games: Vec<SteamGame> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with(MANIFEST_PREFIX) && name.ends_with(MANIFEST_SUFFIX)))
            .filter_map(|entry| _manifest_game(&std::fs::read_to_string(entry.path()).ok()?, &steamapps))
            .collect();
        games.sort_by_key(|game| game.id);
        games
    }

    /// The app an app manifest names, installed under `steamapps`: its `appid`, which it must have,
    /// its `name`, and its `installdir`, its folder in `steamapps/common`.
    fn _manifest_game(acf: &str, steamapps: &Path) -> Option<SteamGame> {
        let pairs = _vdf_pairs(acf);
        let value = |key: &str| pairs.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value.as_str());
        let id: u32 = value("appid")?.trim().parse().ok()?;
        let folder = value("installdir").filter(|dir| !dir.is_empty()).map(|dir| steamapps.join(COMMON_DIR).join(dir));
        Some(SteamGame { id, title: value("name").unwrap_or_default().to_string(), folder, exe: None })
    }

    /// The `"key" "value"` pairs of Valve's text KeyValues (an app manifest), at any depth, in
    /// order: a line holding two quoted strings is a pair, as Steam writes them, each string's
    /// escapes (`\"`, `\\`, `\n`, `\t`) undone.
    fn _vdf_pairs(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|line| match <[String; 2]>::try_from(_quoted(line)) {
                Ok([key, value]) => Some((key, value)),
                Err(_) => None,
            })
            .collect()
    }

    /// The quoted strings on a line of Valve's text KeyValues, their escapes undone; one with no
    /// closing quote is left out.
    fn _quoted(line: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut chars = line.chars();
        while chars.any(|c| c == '"') {
            let mut text = String::new();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some('t') => text.push('\t'),
                        Some(escaped) => text.push(escaped),
                        None => return found,
                    },
                    Some(c) => text.push(c),
                    None => return found,
                }
            }
            found.push(text);
        }
        found
    }

    /// The non-Steam games an account added to Steam, as its `shortcuts.vdf` keeps them (Valve's
    /// binary KeyValues: a `shortcuts` map holding a map per game): each one's id, its title, its
    /// .exe (quoted, as Steam writes it, or not) and that .exe's folder — none for a command that is
    /// no path. One with no id is passed over; a file that doesn't read gives none.
    fn _shortcuts_in(vdf: &[u8]) -> Vec<SteamGame> {
        let mut at = 0;
        let Some(root) = _binary_map(vdf, &mut at) else {
            return Vec::new();
        };
        let Some(Binary::Map(shortcuts)) = _entry(&root, "shortcuts") else {
            return Vec::new();
        };
        shortcuts
            .iter()
            .filter_map(|(_, shortcut)| {
                let Binary::Map(fields) = shortcut else { return None };
                let Some(&Binary::Number(id)) = _entry(fields, "appid") else { return None };
                let exe = Some(PathBuf::from(_text(fields, "exe").trim().trim_matches('"'))).filter(|exe| exe.is_absolute());
                let folder = exe.as_deref().and_then(Path::parent).map(Path::to_path_buf);
                Some(SteamGame { id, title: _text(fields, "appname").to_string(), folder, exe })
            })
            .collect()
    }

    /// A value in Valve's binary KeyValues: a map, text, or a 32-bit number.
    #[derive(Debug, PartialEq)]
    enum Binary {
        Map(Vec<(String, Binary)>),
        Text(String),
        Number(u32),
    }

    /// The entry named `key` in a map, its letter case ignored: Steam has written both `appname`
    /// and `AppName`, `exe` and `Exe`.
    fn _entry<'a>(map: &'a [(String, Binary)], key: &str) -> Option<&'a Binary> {
        map.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
    }

    /// The text entry named `key` in a map ([`_entry`]), empty where there's none.
    fn _text<'a>(map: &'a [(String, Binary)], key: &str) -> &'a str {
        match _entry(map, key) {
            Some(Binary::Text(text)) => text,
            _ => "",
        }
    }

    /// The map starting at `at` in binary KeyValues, through its end mark; `at` moves past it. Each
    /// entry is a type byte, a NUL-ended name, then the value: a map (0), NUL-ended text (1), or a
    /// little-endian 32-bit number (2); [`BINARY_MAP_END`] ends the map. `None` for another type,
    /// or data that ends first.
    fn _binary_map(data: &[u8], at: &mut usize) -> Option<Vec<(String, Binary)>> {
        let mut map = Vec::new();
        loop {
            let kind = *data.get(*at)?;
            *at += 1;
            if kind == BINARY_MAP_END {
                return Some(map);
            }
            let name = _binary_text(data, at)?;
            let value = match kind {
                0 => Binary::Map(_binary_map(data, at)?),
                1 => Binary::Text(_binary_text(data, at)?),
                2 => {
                    let bytes: [u8; 4] = data.get(*at..*at + 4)?.try_into().ok()?;
                    *at += 4;
                    Binary::Number(u32::from_le_bytes(bytes))
                }
                _ => return None,
            };
            map.push((name, value));
        }
    }

    /// The NUL-ended text starting at `at`, as UTF-8 (lossily); `at` moves past its NUL.
    fn _binary_text(data: &[u8], at: &mut usize) -> Option<String> {
        let rest = data.get(*at..)?;
        let end = rest.iter().position(|&byte| byte == 0)?;
        *at += end + 1;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    /// What `gm_id_steam` prints, in order: each folder the game is installed in; the folders its
    /// saves are in, those that exist — its Proton prefix in each library, then its Steam Cloud
    /// folder in each account's; the id.
    fn _steam_lines(known: &SteamKnown, installs: &[PathBuf], id: u32) -> Vec<String> {
        let prefixes = known.libraries.iter().map(|library| library.join(STEAMAPPS_DIR).join(COMPATDATA_DIR).join(id.to_string()));
        let cloud = known.accounts.iter().map(|account| account.join(id.to_string()));
        let saves = prefixes.chain(cloud).filter(|folder| folder.is_dir());
        installs
            .iter()
            .cloned()
            .chain(saves)
            .map(|folder| folder.display().to_string())
            .chain(std::iter::once(id.to_string()))
            .collect()
    }

    /// A Steam library's own folder, in it.
    const STEAMAPPS_DIR: &str = "steamapps";
    /// Where a library keeps its games, each in the folder its manifest names: in `steamapps`.
    const COMMON_DIR: &str = "common";
    /// Where a library keeps its games' Proton prefixes, each named for its game's id: in
    /// `steamapps`.
    const COMPATDATA_DIR: &str = "compatdata";
    /// An app manifest's name before its app's id, in `steamapps`.
    const MANIFEST_PREFIX: &str = "appmanifest_";
    /// An app manifest's name after its app's id.
    const MANIFEST_SUFFIX: &str = ".acf";
    /// Where Steam keeps a folder per account, named for the account's id: in Steam's own folder.
    const USERDATA_DIR: &str = "userdata";
    /// The non-Steam games an account added to Steam: in its folder.
    const SHORTCUTS_FILE: &str = "config/shortcuts.vdf";
    /// The hosts of the Steam pages that name a game by its id (`/app/<id>`): the store's and the
    /// community's.
    const STEAM_PAGE_HOSTS: &[&str] = &["store.steampowered.com", "steamcommunity.com", "www.steamcommunity.com"];
    /// The lower half of the 64-bit id Steam runs a non-Steam game by: type 2, a shortcut, in bits
    /// 24–31; the game's own id is the upper half.
    const SHORTCUT_GAME_ID: u64 = 0x0200_0000;
    /// The byte that ends a map in binary KeyValues.
    const BINARY_MAP_END: u8 = 8;

    // --- gm_protons -----------------------------------------------------------------------------

    /// List the Protons this computer has, newest first: when each was built, whether it runs games
    /// on Wayland itself, and which one gm_run_exe takes (by default, and with --xwayland)
    pub fn protons(_args: NoArgs) {
        let protons = proton::installed();
        if protons.is_empty() {
            eprintln!("gm_protons: no Proton on this computer (gm_run_exe downloads the latest proton-cachyos when it first runs)");
            std::process::exit(1);
        }
        let lines = _proton_rows(&protons, config_file::always_bundle_middleware());
        for line in table_formatter::format_table(&lines, &comfy_repos::listing_table_options()).unwrap_or(lines) {
            println!("{line}");
        }
    }

    /// The listing's rows under its header: each Proton's name, build date, what it runs games on,
    /// whether `gm_run_exe` takes it (by default, or with `--xwayland`), and its folder.
    fn _proton_rows(protons: &[Proton], bundle_first: bool) -> Vec<String> {
        let pick = |wayland| _choose(protons, None, wayland, bundle_first).ok().map(|choice| choice.proton.dir.clone());
        let (by_default, with_xwayland) = (pick(true), pick(false));
        let mut lines = vec![_header(&PROTONS_HEADER.join("  "))];
        for proton in protons {
            let taken = if by_default.as_ref() == Some(&proton.dir) {
                "yes"
            } else if with_xwayland.as_ref() == Some(&proton.dir) {
                "with --xwayland"
            } else {
                "-"
            };
            let runs_on = if proton.wayland { "Wayland" } else { "X11" };
            lines.push([proton.name.as_str(), &_date(proton.built), runs_on, taken, &proton.dir.display().to_string()].join("\t"));
        }
        lines
    }

    /// The listing's column names.
    const PROTONS_HEADER: &[&str] = &["Proton", "Built", "Runs on", "Taken", "Folder"];

    /// `secs`, Unix time, as its date in UTC: `YYYY-MM-DD` (Howard Hinnant's days-to-civil).
    fn _date(secs: u64) -> String {
        let days = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
        let era = days.div_euclid(146_097);
        let day_of_era = days.rem_euclid(146_097);
        let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        format!("{year:04}-{month:02}-{day:02}")
    }

    // --- gm_run_exe -----------------------------------------------------------------------------

    /// Run a Windows game (.exe) through Proton, in a sandbox that sees only its own folders
    ///
    /// The newest Proton here runs it (gm_protons lists them; the latest proton-cachyos is
    /// downloaded where there is none), on Wine's own Wayland driver where that Proton has one, and
    /// with the network cut off. The sandbox (bubblewrap) shows the system read-only, and lets the
    /// game write only to its own folder and to one of its own under
    /// ~/Games/manual_run_proton_prefixes: its Wine prefix, where Windows games keep their saves,
    /// and the home it sees. Neither may be, or hold, one of your own folders (your home,
    /// Downloads, …) or a whole filesystem, nor may a folder --allow gives it. Inside, a syscall
    /// filter (Flatpak's) refuses the calls a game has no use for, and the game can make no
    /// namespaces of its own. The GPU, controllers and sound reach it. Where CachyOS's
    /// game-performance (or Feral's GameMode) is installed, the game runs under it. A game another
    /// launcher has run (Steam, so far) can go on from that launcher's prefix: --prefix-continue
    /// runs it there, --take-ownership copies it into the game's own folder first. Options go
    /// before the .exe; what follows it is the game's.
    pub fn run_exe(args: RunExeArgs) {
        let fail = |why: String| -> ! {
            eprintln!("gm_run_exe: {why}");
            std::process::exit(1)
        };
        let exe = match std::fs::canonicalize(&args.exe) {
            Ok(folder) if folder.is_dir() => _exe_in(&folder).unwrap_or_else(|why| fail(why)),
            Ok(exe) if exe.is_file() => exe,
            _ => fail(format!("{} is no file", args.exe.display())),
        };
        let game_dir = match &args.game_dir {
            Some(dir) => std::fs::canonicalize(dir).unwrap_or_else(|_| fail(format!("{} is no folder", dir.display()))),
            None => exe.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        if !exe.starts_with(&game_dir) {
            fail(format!("{} isn't inside the game's folder, {}", exe.display(), game_dir.display()));
        }
        let allowed: Vec<PathBuf> = args
            .allow
            .iter()
            .map(|dir| std::fs::canonicalize(dir).unwrap_or_else(|_| fail(format!("{} is no folder", dir.display()))))
            .collect();
        // No folder the game may write to may be, or hold, the user's own folders (the home,
        // Downloads, …) or a whole filesystem: the game could write to all of it.
        let own = _users_folders(&_canonical(&crate::conf::home()), &|key| std::env::var_os(key));
        let mounts = _mount_points();
        if let Some(why) = _too_broad(&game_dir, &own, &mounts) {
            let remedy = match args.game_dir {
                Some(_) => "name a folder that is the game's own with --game-dir",
                None => "put the .exe in a folder of its own, or name the game's folder with --game-dir",
            };
            fail(format!("the game could write to all of its folder, {}, and {why} ({remedy})", game_dir.display()));
        }
        for dir in &allowed {
            if let Some(why) = _too_broad(dir, &own, &mounts) {
                fail(format!("--allow {}: the game could write to all of it, and {why} (allow a folder inside it instead)", dir.display()));
            }
        }
        if !exec::on_path("bwrap") {
            fail("its sandbox is bubblewrap (bwrap), which isn't installed".to_string());
        }
        if !SYSTEM_PYTHONS.iter().any(|python| Path::new(python).is_file()) {
            fail("Proton runs on the system's python3, which isn't installed".to_string());
        }
        // The game's own folder; or, to go on from another launcher's, that one — where it is, or
        // copied into the game's own folder, which mustn't be there yet. Settled before a Proton is
        // fetched, so that nothing comes down for a run that can't happen.
        let given = args.prefix.as_deref().map(|dir| std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
        let own_folder = given.unwrap_or_else(|| crate::conf::home().join(PREFIXES_DIR).join(_prefix_name(&game_dir)));
        if let Some(why) = _too_broad(&own_folder, &own, &mounts).filter(|_| !args.prefix_continue) {
            fail(format!("--prefix {}: the game could write to all of it, and {why} (name a folder that is the game's own)", own_folder.display()));
        }
        if args.take_ownership && std::fs::symlink_metadata(&own_folder).is_ok() {
            fail(format!("{} is already there: the game has a prefix of its own (move it away to take over another)", own_folder.display()));
        }
        let past = (args.take_ownership || args.prefix_continue).then(|| _past_prefix(&exe).unwrap_or_else(|why| fail(why)));
        let prefix = match &past {
            Some(past) if args.prefix_continue => past.folder.clone(),
            _ => own_folder,
        };

        let mut protons = proton::installed();
        // A Proton asked for by its folder needn't be where Steam keeps them.
        if let Some(asked) = args.proton.as_deref().and_then(|asked| proton::at(Path::new(asked))) {
            let canonical = |dir: &Path| std::fs::canonicalize(dir).ok();
            if !protons.iter().any(|known| canonical(&known.dir) == canonical(&asked.dir)) {
                protons.push(asked);
            }
        }
        if protons.is_empty() && args.proton.is_none() {
            eprintln!("gm_run_exe: no Proton on this computer — downloading the latest proton-cachyos (about 330 MB)");
            if let Err(why) = tools::bundle_now(proton::PROTON_DIR) {
                fail(format!("no Proton on this computer, and none could be downloaded: {why}"));
            }
            protons = proton::installed();
        }
        let host = Host::here();
        let want_wayland = !args.xwayland && host.wayland.is_some();
        let bundle_first = config_file::always_bundle_middleware();
        let choice = _choose(&protons, args.proton.as_deref(), want_wayland, bundle_first).unwrap_or_else(|why| fail(why));
        let plan = Plan {
            proton: &choice.proton.dir,
            exe: &exe,
            game_dir: &game_dir,
            prefix: &prefix,
            allowed: &allowed,
            game_args: &args.game_args,
            network: args.network,
            wayland: choice.wayland,
        };
        let mut command: Vec<OsString> = Vec::new();
        if !args.no_performance {
            command.extend(_performance_wrapper().map(OsString::from));
        }
        command.push("bwrap".into());
        command.extend(_bwrap_args(&plan, &host));

        if args.dry_run {
            if let Some(past) = past.as_ref().filter(|_| args.take_ownership) {
                eprintln!("gm_run_exe: would copy {} into {} first", past.described(), prefix.display());
            }
            let shell = Shell::caller();
            let words: Vec<String> = command.iter().map(|word| shell_quote::word(shell, &word.to_string_lossy(), Quoting::WhereNeeded)).collect();
            println!("{}", words.join(" "));
            return;
        }
        if let Some(past) = &past {
            if args.take_ownership {
                eprintln!("gm_run_exe: copying {} into {}", past.described(), prefix.display());
                _copy_prefix(&past.folder, &prefix).unwrap_or_else(|why| fail(why));
            } else {
                eprintln!("gm_run_exe: going on from {}, where it is", past.described());
            }
        }
        for dir in [prefix.join(HOME_DIR), prefix.join(STEAM_CLIENT_DIR)] {
            if let Err(err) = std::fs::create_dir_all(&dir) {
                fail(format!("couldn't make {}: {err}", dir.display()));
            }
        }
        eprintln!("gm_run_exe: {}", _summary(&choice, &plan, &host, args.xwayland));
        let status = Command::new(&command[0]).args(&command[1..]).env("PATH", _path_without_shims()).status();
        match status {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(err) => fail(format!("couldn't start {}: {err}", command[0].to_string_lossy())),
        }
    }

    /// The game, its arguments, and how to run it.
    #[derive(Args)]
    pub struct RunExeArgs {
        /// The game's .exe, or the game's folder where that holds just one (crash handlers and
        /// uninstallers aside)
        pub exe: PathBuf,
        /// Arguments for the game
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        pub game_args: Vec<String>,
        /// Run on X11 (XWayland) instead of Wine's own Wayland driver: for a game that misbehaves on
        /// Wayland, or to take the newest Proton when it has no Wayland driver
        #[arg(long)]
        pub xwayland: bool,
        /// Let the game reach the network, which it is cut off from by default
        #[arg(long)]
        pub network: bool,
        /// The Proton to run it on instead of the newest: a name gm_protons lists (or part of one),
        /// or a Proton's folder
        #[arg(long, value_name = "NAME|DIR")]
        pub proton: Option<String>,
        /// The game's folder, which it may write to, when the .exe sits deeper in it (bin/,
        /// Binaries/Win64/); the .exe's own folder by default
        #[arg(long, value_name = "DIR")]
        pub game_dir: Option<PathBuf>,
        /// One more folder the game may read and write; may be given again
        #[arg(long, value_name = "DIR")]
        pub allow: Vec<PathBuf>,
        /// The game's own folder (its Wine prefix and the home it sees), instead of the one made for
        /// it under ~/Games/manual_run_proton_prefixes; a Steam game's prefix is the folder
        /// gm_id_steam names in steamapps/compatdata
        #[arg(long, value_name = "DIR")]
        pub prefix: Option<PathBuf>,
        /// Copy the prefix another launcher made for this .exe (Steam's, so far) into the game's own
        /// folder (the default one, or --prefix), then run the game there. Refused where that folder
        /// is already there, or no launcher has a prefix for it; where several have, a numbered
        /// menu asks which
        #[arg(long, conflicts_with = "prefix_continue")]
        pub take_ownership: bool,
        /// Run the game on the prefix another launcher made for this .exe (Steam's, so far), where it
        /// is, as --prefix with its folder would; found as --take-ownership finds it, with the same
        /// menu and the same refusals
        #[arg(long, conflicts_with = "prefix")]
        pub prefix_continue: bool,
        /// Run without game-performance or GameMode, even where installed
        #[arg(long)]
        pub no_performance: bool,
        /// Print the command that would run, and run nothing
        #[arg(long)]
        pub dry_run: bool,
    }

    /// The Proton a run takes, whether it runs the game on Wayland, and what's worth saying about the
    /// pick.
    struct Choice<'a> {
        proton: &'a Proton,
        wayland: bool,
        note: Option<String>,
    }

    /// Which Proton runs the game. One asked for by `--proton`: a Proton's folder, or a name among
    /// `protons` (or part of one, the newest such). Else the bundle where
    /// `[tools] always_bundle_middleware` makes it bashrs's own pick, else the newest of them all —
    /// and, for a game to run on Wayland, the newest with a Wayland driver, unless none has one. It
    /// runs on Wayland only where asked to and able to.
    fn _choose<'a>(protons: &'a [Proton], asked: Option<&str>, wayland: bool, bundle_first: bool) -> Result<Choice<'a>, String> {
        if let Some(asked) = asked {
            let folder = std::fs::canonicalize(asked).ok();
            let lower = asked.to_lowercase();
            let named = protons.iter().find(|proton| {
                folder.as_ref().is_some_and(|folder| std::fs::canonicalize(&proton.dir).is_ok_and(|dir| dir == *folder))
                    || proton.name.to_lowercase().contains(&lower)
            });
            let proton = match named {
                Some(proton) => proton,
                None => return Err(format!("{asked} is no Proton's folder, and no Proton here is called that (gm_protons lists them)")),
            };
            let note = (wayland && !proton.wayland).then(|| format!("{} has no Wayland driver, so the game runs on X11", proton.name));
            return Ok(Choice { proton, wayland: wayland && proton.wayland, note });
        }
        let candidates: Vec<&Proton> = if bundle_first && protons.iter().any(|proton| proton.bundled) {
            protons.iter().filter(|proton| proton.bundled).collect()
        } else {
            protons.iter().collect()
        };
        let Some(&newest) = candidates.first() else {
            return Err("there's no Proton on this computer".to_string());
        };
        if !wayland {
            return Ok(Choice { proton: newest, wayland: false, note: None });
        }
        Ok(match candidates.iter().copied().find(|proton| proton.wayland) {
            Some(proton) if proton.dir == newest.dir => Choice { proton, wayland: true, note: None },
            Some(proton) => Choice {
                proton,
                wayland: true,
                note: Some(format!("the newest Proton, {}, has no Wayland driver (--xwayland runs the game on it)", newest.name)),
            },
            None => Choice { proton: newest, wayland: false, note: Some("no Proton here has a Wayland driver, so the game runs on X11".to_string()) },
        })
    }

    /// What one run puts together: its Proton, the game and its arguments, the game's own folder
    /// (the prefix), the folders it may write to, and whether it gets the network and Wayland.
    struct Plan<'a> {
        proton: &'a Path,
        exe: &'a Path,
        game_dir: &'a Path,
        prefix: &'a Path,
        allowed: &'a [PathBuf],
        game_args: &'a [String],
        network: bool,
        wayland: bool,
    }

    /// What of this computer the sandbox shows the game, found by [`Host::here`]; apart from the
    /// sandbox's arguments, so those can be built, and tested, for any computer.
    #[derive(Debug, Default)]
    struct Host {
        /// The top-level system folders shown read-only, each with its link's target where it is a
        /// link (`/bin` → `usr/bin`, as on merged-`/usr` systems).
        system: Vec<(PathBuf, Option<PathBuf>)>,
        /// The device nodes a game uses: GPUs, controllers, sound, and ntsync, Wine's fast sync.
        devices: Vec<PathBuf>,
        /// The user's runtime folder (`XDG_RUNTIME_DIR`), holding the display and sound sockets.
        runtime_dir: Option<PathBuf>,
        /// The Wayland display: its name as `WAYLAND_DISPLAY` gives it, and its socket.
        wayland: Option<(String, PathBuf)>,
        /// The X display (`DISPLAY`), and its authority file (`XAUTHORITY`) where there is one.
        x11: Option<(String, Option<PathBuf>)>,
        /// The sound servers' sockets: PipeWire's and PulseAudio's.
        sound: Vec<PathBuf>,
        /// udev's device database, through which controllers are found.
        udev: Option<PathBuf>,
        /// The folder `/etc/resolv.conf` links into, outside what's shown anyway (systemd-resolved
        /// keeps it in `/run`): name lookups, for a game given the network.
        resolver: Option<PathBuf>,
        /// The environment a game takes along: locale, timezone, and what tunes Proton and the
        /// graphics stack ([`_passed_along`]).
        env: Vec<(String, OsString)>,
        /// Whether this bubblewrap lays a throwaway layer over a folder (`--tmp-overlay`, from
        /// version 0.8), so a Proton that writes into its own folder can, without changing it.
        overlays: bool,
        /// Whether this bubblewrap can keep the sandbox from making user namespaces of its own
        /// (`--disable-userns`): from version 0.8, and only where it isn't installed setuid.
        userns_locked: bool,
        /// This program, put in the sandbox to set the syscall filter in place before Proton starts
        /// ([`_filtered_exec`]); none where there's no filter for this architecture.
        filter_exe: Option<PathBuf>,
    }

    impl Host {
        /// This computer, as the running user's session sees it.
        fn here() -> Self {
            let system = SYSTEM_DIRS
                .iter()
                .map(Path::new)
                .filter(|dir| dir.exists())
                .map(|dir| (dir.to_path_buf(), std::fs::read_link(dir).ok()))
                .collect();
            let mut devices: Vec<PathBuf> = DEVICES.iter().map(PathBuf::from).filter(|device| device.exists()).collect();
            if let Ok(entries) = std::fs::read_dir("/dev") {
                let mut numbered: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .filter(|entry| NUMBERED_DEVICES.iter().any(|kind| entry.file_name().to_string_lossy().starts_with(kind)))
                    .map(|entry| entry.path())
                    .collect();
                numbered.sort();
                devices.extend(numbered);
            }
            let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|dir| dir.is_absolute() && dir.is_dir());
            let wayland = std::env::var("WAYLAND_DISPLAY").ok().filter(|name| !name.is_empty()).and_then(|name| {
                let socket = match Path::new(&name).is_absolute() {
                    true => PathBuf::from(&name),
                    false => runtime_dir.as_ref()?.join(&name),
                };
                socket.exists().then_some((name, socket))
            });
            let x11 = std::env::var("DISPLAY")
                .ok()
                .filter(|display| !display.is_empty())
                .map(|display| (display, std::env::var_os("XAUTHORITY").map(PathBuf::from).filter(|file| file.is_file())));
            let sound = runtime_dir.iter().flat_map(|dir| SOUND_SOCKETS.iter().map(move |socket| dir.join(socket))).filter(|socket| socket.exists()).collect();
            let resolver = std::fs::canonicalize("/etc/resolv.conf")
                .ok()
                .filter(|target| !target.starts_with("/etc") && !target.starts_with("/usr"))
                .and_then(|target| target.parent().map(Path::to_path_buf));
            let from_0_8 = exec::capture_stdout("bwrap", ["--version"]).is_some_and(|version| _from_0_8(&version));
            Host {
                system,
                devices,
                runtime_dir,
                wayland,
                x11,
                sound,
                udev: Some(PathBuf::from("/run/udev")).filter(|udev| udev.is_dir()),
                resolver,
                env: std::env::vars_os().filter_map(|(key, value)| Some((key.into_string().ok()?, value))).filter(|(key, _)| _passed_along(key)).collect(),
                overlays: from_0_8,
                userns_locked: from_0_8 && !_setuid("bwrap"),
                filter_exe: seccomp::program(std::env::consts::ARCH).and_then(|_| std::env::current_exe().ok()),
            }
        }
    }

    /// Whether the bubblewrap whose `--version` says `version` is 0.8 or later, which brought
    /// throwaway overlays and `--disable-userns`.
    fn _from_0_8(version: &str) -> bool {
        let numbers: Vec<u32> = version.split_whitespace().last().unwrap_or("").split('.').map_while(|part| part.parse().ok()).collect();
        matches!(numbers.as_slice(), [major, minor, ..] if (*major, *minor) >= (0, 8))
    }

    /// Whether `program`, as `PATH` finds it, is installed setuid.
    fn _setuid(program: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        exec::path_hits(program).first().and_then(|path| std::fs::metadata(path).ok()).is_some_and(|meta| meta.permissions().mode() & 0o4000 != 0)
    }

    /// The sandbox's first step: this program started again inside it (`bashrs
    /// --gm-run-exe-filtered PROGRAM ARGS…`, claimed through the binary's `REEXEC_HANDLERS`), which
    /// sets the syscall filter in place ([`seccomp`]) and becomes PROGRAM — Proton's run, which the
    /// filter then holds for good. A filter that can't be set stops the run there, so that no game
    /// runs unfiltered where a filter was meant.
    pub(crate) fn _filtered_exec() -> bool {
        use std::os::unix::process::CommandExt;
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(OsStr::new(FILTERED_MARKER)) {
            return false;
        }
        let Some(program) = args.next() else {
            eprintln!("gm_run_exe: the sandbox was given nothing to run");
            std::process::exit(127);
        };
        let filter = seccomp::program(std::env::consts::ARCH).ok_or_else(|| std::io::Error::other(format!("there's none for {}", std::env::consts::ARCH)));
        if let Err(err) = filter.and_then(|filter| seccomp::install(&filter)) {
            eprintln!("gm_run_exe: couldn't set the syscall filter in place: {err}");
            std::process::exit(127);
        }
        let err = Command::new(&program).args(args).exec();
        eprintln!("gm_run_exe: couldn't start {}: {err}", program.to_string_lossy());
        std::process::exit(127);
    }

    /// The sandbox's arguments for `bwrap`, ending with what it runs. A namespace of its own for
    /// everything, the network's included unless the game gets it, and IPC's unless it runs on
    /// X11, whose shared memory would be cut off; the user's, in which no more can be made, where
    /// bubblewrap can see to that. The system read-only, as it is here; devices, the display's and
    /// sound's sockets; Proton under a throwaway layer (read-only, where bubblewrap has no such
    /// layers); read-write the game's folder, the folders allowed it, and its own folder, the
    /// prefix. A new session, so the game can't type into this terminal, and the sandbox goes when
    /// its starter does. Then a clean environment, and [`RUN_SCRIPT`] in the .exe's folder — set
    /// going by this program, which sets the syscall filter in place first ([`_filtered_exec`]),
    /// where there's a filter for this architecture.
    fn _bwrap_args(plan: &Plan, host: &Host) -> Vec<OsString> {
        let mut args: Vec<OsString> = Vec::new();
        let mut add = |words: &[&OsStr]| args.extend(words.iter().map(|word| word.to_os_string()));
        add(&["--die-with-parent", "--new-session", "--unshare-pid", "--unshare-uts", "--unshare-cgroup-try"].map(OsStr::new));
        if host.userns_locked {
            add(&["--unshare-user", "--disable-userns"].map(OsStr::new));
        }
        if plan.wayland {
            add(&["--unshare-ipc".as_ref()]);
        }
        if !plan.network {
            add(&["--unshare-net".as_ref()]);
        }
        for (dir, link) in &host.system {
            match link {
                Some(target) => add(&["--symlink".as_ref(), target.as_os_str(), dir.as_os_str()]),
                None => add(&["--ro-bind".as_ref(), dir.as_os_str(), dir.as_os_str()]),
            }
        }
        add(&["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"].map(OsStr::new));
        for device in &host.devices {
            add(&["--dev-bind".as_ref(), device.as_os_str(), device.as_os_str()]);
        }
        let read_only = |path: &Path| -> [OsString; 3] { ["--ro-bind".into(), path.into(), path.into()] };
        let mut shown: Vec<&Path> = host.udev.iter().map(PathBuf::as_path).collect();
        if plan.network {
            shown.extend(host.resolver.as_deref());
        }
        if let Some(runtime) = &host.runtime_dir {
            add(&["--perms".as_ref(), "0700".as_ref(), "--dir".as_ref(), runtime.as_os_str()]);
        }
        match (&host.wayland, &host.x11) {
            (Some((_, socket)), _) if plan.wayland => shown.push(socket),
            (_, Some((_, authority))) if !plan.wayland => {
                shown.push(Path::new(X11_SOCKETS));
                shown.extend(authority.as_deref());
            }
            _ => {}
        }
        shown.extend(host.sound.iter().map(PathBuf::as_path));
        for path in shown {
            args.extend(read_only(path));
        }
        // Valve's Protons write into their own folder (a lock, fixes after Steam's updates, their
        // default prefix): given a throwaway layer, they can, and what they write goes with the
        // sandbox. Never the folder itself: a game that changed Proton would reach every game it
        // runs, the unsandboxed ones too.
        match host.overlays {
            true => args.extend(["--overlay-src".into(), plan.proton.into(), "--tmp-overlay".into(), plan.proton.into()]),
            false => args.extend(read_only(plan.proton)),
        }
        if let Some(filter) = &host.filter_exe {
            args.extend(["--ro-bind".into(), filter.into(), FILTER_IN_SANDBOX.into()]);
        }
        for dir in std::iter::once(plan.game_dir).chain(plan.allowed.iter().map(PathBuf::as_path)).chain([plan.prefix]) {
            args.extend(["--bind".into(), dir.into(), dir.into()]);
        }
        args.extend(["--chdir".into(), plan.exe.parent().unwrap_or(plan.game_dir).into(), "--clearenv".into()]);
        for (key, value) in _sandbox_env(plan, host) {
            args.extend(["--setenv".into(), key.into(), value]);
        }
        args.push("--".into());
        if host.filter_exe.is_some() {
            args.extend([FILTER_IN_SANDBOX.into(), FILTERED_MARKER.into()]);
        }
        args.extend(["/bin/sh".into(), "-c".into(), RUN_SCRIPT.into(), "sh".into(), plan.proton.into(), plan.exe.into()]);
        args.extend(plan.game_args.iter().map(OsString::from));
        args
    }

    /// The environment inside the sandbox. What the computer passes along ([`Host::env`]), the
    /// session's display and runtime folder, then what Proton needs outside Steam: its data folder
    /// (the prefix), a Steam client folder (an empty one: there is no Steam in the sandbox), the
    /// game's folder, the home the game sees. The Wayland driver switched on where it's used, and
    /// the shader caches kept whole, so what was compiled once isn't thrown away and compiled again
    /// mid-game (the user's own settings for them win).
    fn _sandbox_env(plan: &Plan, host: &Host) -> Vec<(String, OsString)> {
        let mut env = host.env.clone();
        let mut set = |key: &str, value: OsString| {
            env.retain(|(known, _)| known != key);
            env.push((key.to_string(), value));
        };
        set("HOME", plan.prefix.join(HOME_DIR).into());
        set("PATH", "/usr/local/bin:/usr/bin:/bin".into());
        set("STEAM_COMPAT_DATA_PATH", plan.prefix.into());
        set("STEAM_COMPAT_CLIENT_INSTALL_PATH", plan.prefix.join(STEAM_CLIENT_DIR).into());
        set("STEAM_COMPAT_INSTALL_PATH", plan.game_dir.into());
        set("WINEPREFIX", plan.prefix.join(PFX_DIR).into());
        if let Some(runtime) = &host.runtime_dir {
            set("XDG_RUNTIME_DIR", runtime.into());
        }
        match (&host.wayland, &host.x11) {
            (Some((name, _)), _) if plan.wayland => {
                set("WAYLAND_DISPLAY", name.into());
                set(proton::WAYLAND_SWITCH, "1".into());
            }
            (_, Some((display, authority))) if !plan.wayland => {
                set("DISPLAY", display.into());
                if let Some(authority) = authority {
                    set("XAUTHORITY", authority.into());
                }
            }
            _ => {}
        }
        for &(key, value) in SHADER_CACHES {
            if !env.iter().any(|(known, _)| known == key) {
                env.push((key.to_string(), value.into()));
            }
        }
        env
    }

    /// Whether the variable `key` goes into the sandbox: the locale and timezone, the user's name,
    /// and the variables that tune Proton, Wine and the graphics stack, as the user set them — but
    /// not what the run sets itself (the prefix, the Wayland switch).
    fn _passed_along(key: &str) -> bool {
        let tuning = PASSED_PREFIXES.iter().any(|prefix| key.starts_with(prefix));
        (PASSED_ALONG.contains(&key) || tuning) && !NOT_PASSED.contains(&key)
    }

    /// The line said as the game starts: its Proton, what it runs on, whether it has the network,
    /// and where its own folder is; then whatever the pick needs said, and what the sandbox goes
    /// without here.
    fn _summary(choice: &Choice, plan: &Plan, host: &Host, xwayland_asked: bool) -> String {
        let display = match (plan.wayland, &host.x11) {
            (true, _) => "Wayland",
            (false, Some(_)) => "X11",
            (false, None) => "no display",
        };
        let network = if plan.network { "on" } else { "off" };
        let mut line = format!("{} on {display}, network {network}; the game's own folder: {}", choice.proton.name, plan.prefix.display());
        let no_wayland = (!xwayland_asked && host.wayland.is_none()).then(|| "this session has no Wayland display".to_string());
        let unfiltered = host.filter_exe.is_none().then(|| format!("no syscall filter for {} yet: the game's calls go unchecked", std::env::consts::ARCH));
        let userns = (!host.userns_locked).then(|| "this bubblewrap can't stop the game making user namespaces (that takes 0.8 or later, not setuid)".to_string());
        for note in choice.note.clone().into_iter().chain(no_wayland).chain(unfiltered).chain(userns) {
            line.push_str(&format!("\ngm_run_exe: {note}"));
        }
        line
    }

    /// The game in `folder`, named by its folder: the one `.exe` right in it, the helpers games
    /// ship beside themselves aside ([`HELPER_EXES`]). None, or several, is said, naming what's there.
    fn _exe_in(folder: &Path) -> Result<PathBuf, String> {
        let mut exes: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|err| format!("{}: {err}", folder.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")))
            .collect();
        exes.sort();
        let names = |exes: &[&PathBuf]| -> String {
            exes.iter().filter_map(|exe| exe.file_name()).map(|name| name.to_string_lossy()).collect::<Vec<_>>().join(", ")
        };
        let games: Vec<&PathBuf> = exes.iter().filter(|exe| !_is_helper(exe)).collect();
        match games.as_slice() {
            [game] => Ok(game.to_path_buf()),
            [] if exes.is_empty() => Err(format!("{} holds no .exe", folder.display())),
            [] => Err(format!("{} holds only helpers ({}) — name the .exe to run", folder.display(), names(&exes.iter().collect::<Vec<_>>()))),
            several => Err(format!("{} holds several .exe files — name the one to run: {}", folder.display(), names(several))),
        }
    }

    /// Whether `exe` is one of the helpers games ship beside themselves, by its name's start.
    fn _is_helper(exe: &Path) -> bool {
        let name = exe.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        HELPER_EXES.iter().any(|helper| name.starts_with(helper))
    }

    /// The starts of the names of the `.exe` files games ship beside themselves that are no game:
    /// Unity's crash handler and other crash reporters, uninstallers, runtime installers.
    const HELPER_EXES: &[&str] = &["unitycrashhandler", "crashreport", "crashpad", "unins", "uninstall", "vc_redist", "vcredist", "dxsetup", "oalinst", "physx", "dotnet"];

    /// The game's own folder's default name in [`PREFIXES_DIR`]: its folder's name, then a
    /// checksum of that folder's whole path, so games in same-named folders never share one.
    fn _prefix_name(game_dir: &Path) -> String {
        let name = game_dir.file_name().map_or_else(|| "game".to_string(), |name| name.to_string_lossy().into_owned());
        let mut crc = flate2::Crc::new();
        crc.update(game_dir.as_os_str().as_bytes());
        format!("{name}-{:08x}", crc.sum())
    }

    /// A prefix another launcher made for a game: the launcher, its name for the game, and the
    /// prefix's folder — shaped as the game's own folder is, with the Wine prefix in its `pfx`.
    #[derive(Debug, Clone, PartialEq)]
    struct PastPrefix {
        launcher: &'static str,
        game: String,
        folder: PathBuf,
    }

    impl PastPrefix {
        /// How a menu and a message name it: whose prefix, for which game, and where.
        fn described(&self) -> String {
            format!("{}'s prefix for {} ({})", self.launcher, self.game, self.folder.display())
        }
    }

    /// The prefix another launcher made for the game at `exe`, for `--take-ownership` and
    /// `--prefix-continue` alike: the one there is, or, where there are several, the one picked by
    /// its number from a menu, the kind ALT+W shows. Why not, where no launcher has one, the menu
    /// can't show, or nothing is picked.
    fn _past_prefix(exe: &Path) -> Result<PastPrefix, String> {
        let found = _past_prefixes(exe, &crate::conf::home());
        let several = match found.as_slice() {
            [] => {
                let launchers: Vec<&str> = LAUNCHERS.iter().map(|(launcher, _)| *launcher).collect();
                return Err(format!("no launcher here has a prefix for {} (looked in: {})", exe.display(), launchers.join(", ")));
            }
            [one] => return Ok(one.clone()),
            several => several,
        };
        let intro = format!("More than one prefix was made for {}:", exe.display());
        let mut menu = several.iter().fold(terminal_choice::Menu::new("Which one?").intro(intro), |menu, past| menu.pick(past.described()));
        match terminal_choice::run_menu(&mut menu) {
            Ok(terminal_choice::Chosen::Picked(at)) => Ok(several[at].clone()),
            Ok(terminal_choice::Chosen::Cancelled) => Err("no prefix was picked, so nothing ran".to_string()),
            Err(err) => {
                let listed: Vec<String> = several.iter().map(PastPrefix::described).collect();
                Err(format!("more than one prefix was made for {} — {} — and the menu to pick one can't show: {err}", exe.display(), listed.join("; ")))
            }
        }
    }

    /// Every prefix another launcher made for the game at `exe`, as each of [`LAUNCHERS`] finds
    /// them under `home`, in their order.
    fn _past_prefixes(exe: &Path, home: &Path) -> Vec<PastPrefix> {
        LAUNCHERS
            .iter()
            .flat_map(|&(launcher, find)| find(exe, home).into_iter().map(move |(game, folder)| PastPrefix { launcher, game, folder }))
            .collect()
    }

    /// What finds a launcher's prefixes for an .exe under a home: for each, the launcher's name for
    /// the game, and the prefix's folder.
    type FindPrefixes = fn(&Path, &Path) -> Vec<(String, PathBuf)>;

    /// The launchers whose prefixes `--take-ownership` and `--prefix-continue` go on from: each
    /// one's name, and what finds its prefixes. A new launcher is a row.
    const LAUNCHERS: &[(&str, FindPrefixes)] = &[("Steam", _steam_prefixes_of)];

    /// The prefixes Steam made for the game at `exe`, under `home`: for each Steam entry that is the
    /// game's — a non-Steam game added with that .exe, or an app installed in a folder holding it —
    /// the prefix Proton made under its id, in each library where there is one (a folder holding
    /// the prefix proper, `pfx`: Steam also makes them empty), each once.
    fn _steam_prefixes_of(exe: &Path, home: &Path) -> Vec<(String, PathBuf)> {
        let known = SteamKnown::gather(home);
        let exe = _canonical(exe);
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        for game in known.games.iter().filter(|game| game.runs(&exe)) {
            let name = if game.title.trim().is_empty() { game.id.to_string() } else { game.title.trim().to_string() };
            for library in &known.libraries {
                let folder = library.join(STEAMAPPS_DIR).join(COMPATDATA_DIR).join(game.id.to_string());
                if folder.join(PFX_DIR).is_dir() && !found.iter().any(|(_, known)| *known == folder) {
                    found.push((name.clone(), folder));
                }
            }
        }
        found
    }

    /// `path` with its links resolved, where it exists; as it is otherwise.
    fn _canonical(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// The user's own folders, which no folder the game may write to may be or hold, each with how
    /// a message names it: the home; the folder every game's own is made in; where programs keep
    /// their settings, data, state and caches (`XDG_CONFIG_HOME` and the rest, read through `env`,
    /// else their usual places); and the folders the user's files live in ([`_user_dirs`]). Each
    /// with its links resolved; `home` is too.
    fn _users_folders(home: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<(String, PathBuf)> {
        let base = |key: &str, usual: &str| env(key).map(PathBuf::from).filter(|dir| dir.is_absolute()).unwrap_or_else(|| home.join(usual));
        let config = base("XDG_CONFIG_HOME", ".config");
        let user_dirs = _user_dirs(&std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default(), home);
        let programs = [config, base("XDG_DATA_HOME", ".local/share"), base("XDG_STATE_HOME", ".local/state"), base("XDG_CACHE_HOME", ".cache")];
        let prefixes = home.join(PREFIXES_DIR);
        [("your home folder".to_string(), home.to_path_buf()), (format!("{}, where every game's own folder is made", _shown(&prefixes, home)), prefixes)]
            .into_iter()
            .chain(programs.into_iter().chain(user_dirs).map(|dir| (format!("{}, one of your own folders", _shown(&dir, home)), dir)))
            .map(|(label, dir)| (label, _canonical(&dir)))
            .collect()
    }

    /// The folders a `user-dirs.dirs` names, a line each (`XDG_DOWNLOAD_DIR="$HOME/Downloads"`),
    /// `$HOME` read as `home` — the home itself left out, as one switched off is set to it; where it
    /// names none (no file, or nothing in it), their usual names in the home ([`USUAL_USER_DIRS`]).
    fn _user_dirs(text: &str, home: &Path) -> Vec<PathBuf> {
        let named: Vec<PathBuf> = text
            .lines()
            .filter_map(|line| {
                let (key, value) = line.trim().split_once('=')?;
                if !(key.starts_with("XDG_") && key.ends_with("_DIR")) {
                    return None;
                }
                let value = value.trim().trim_matches('"');
                let dir = match value.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None => PathBuf::from(value),
                };
                (dir.is_absolute() && dir.as_path() != home).then_some(dir)
            })
            .collect();
        match named.is_empty() {
            true => USUAL_USER_DIRS.iter().map(|name| home.join(name)).collect(),
            false => named,
        }
    }

    /// What a folder the game may write to mayn't be, said as "it is …" or "it holds …", if so: one
    /// of the user's own folders (`own`, [`_users_folders`]), or where a filesystem is mounted
    /// (`mounts`), or a folder holding one.
    fn _too_broad(folder: &Path, own: &[(String, PathBuf)], mounts: &[PathBuf]) -> Option<String> {
        let folder = _canonical(folder);
        let mounted = mounts.iter().map(|dir| (format!("{}, where a filesystem is mounted", dir.display()), dir.clone()));
        own.iter().cloned().chain(mounted).find(|(_, dir)| dir.starts_with(&folder)).map(|(label, dir)| {
            let verb = if dir == folder { "is" } else { "holds" };
            format!("it {verb} {label}")
        })
    }

    /// Where filesystems are mounted, as this process sees them ([`_mount_points_in`]).
    fn _mount_points() -> Vec<PathBuf> {
        _mount_points_in(&std::fs::read("/proc/self/mountinfo").unwrap_or_default())
    }

    /// The mount points a `mountinfo` names: each line's fifth field, the kernel's octal escapes
    /// undone (`\040` a space, `\011` a tab, `\012` a line break, `\134` a backslash).
    fn _mount_points_in(info: &[u8]) -> Vec<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        let unescaped = |field: &[u8]| {
            let mut path = Vec::with_capacity(field.len());
            let mut at = 0;
            while at < field.len() {
                let octal = field.get(at + 1..at + 4).filter(|digits| field[at] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit)));
                match octal {
                    Some(digits) => {
                        path.push(digits.iter().fold(0u8, |byte, digit| byte.wrapping_mul(8).wrapping_add(digit - b'0')));
                        at += 4;
                    }
                    None => {
                        path.push(field[at]);
                        at += 1;
                    }
                }
            }
            PathBuf::from(OsString::from_vec(path))
        };
        info.split(|&byte| byte == b'\n').filter_map(|line| line.split(|&byte| byte == b' ').nth(4)).map(unescaped).collect()
    }

    /// `path` as a message shows it: from `~` where it's in the home.
    fn _shown(path: &Path, home: &Path) -> String {
        match path.strip_prefix(home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }

    /// The usual names of the folders the user's files live in, where `user-dirs.dirs` names none.
    const USUAL_USER_DIRS: &[&str] = &["Desktop", "Documents", "Downloads", "Music", "Pictures", "Public", "Templates", "Videos"];

    /// `from`, a prefix, copied whole to `to` — links as links, modes and times kept, the data
    /// shared where the filesystem can (`cp -a --reflink=auto`) — by way of a folder beside `to`,
    /// renamed to it once the copy is whole, so that a copy cut short is never taken for a prefix.
    /// Why not, where it fails; what the copy made is gone then.
    fn _copy_prefix(from: &Path, to: &Path) -> Result<(), String> {
        let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else {
            return Err(format!("{} names no folder to copy into", to.display()));
        };
        std::fs::create_dir_all(parent).map_err(|err| format!("couldn't make {}: {err}", parent.display()))?;
        let copying = parent.join(format!(".{}.copying-{}", name.to_string_lossy(), std::process::id()));
        let copied = Command::new("cp").args(["-a", "--reflink=auto", "--"]).arg(from).arg(&copying).status();
        let done = match copied {
            Ok(status) if status.success() => std::fs::rename(&copying, to).map_err(|err| format!("couldn't move the copy to {}: {err}", to.display())),
            Ok(status) => Err(format!("copying {} failed ({status})", from.display())),
            Err(err) => Err(format!("couldn't run cp to copy {}: {err}", from.display())),
        };
        if done.is_err() {
            let _ = std::fs::remove_dir_all(&copying);
        }
        done
    }

    /// The program the sandbox runs under for performance, where one is installed: CachyOS's
    /// `game-performance` (the performance power profile while the game runs, and no idle sleep), or
    /// Feral's GameMode.
    fn _performance_wrapper() -> Option<&'static str> {
        PERFORMANCE_WRAPPERS.iter().copied().find(|wrapper| exec::on_path(wrapper))
    }

    /// `PATH` without bashrs's tool shims, for the performance wrapper: `game-performance` runs
    /// `powerprofilesctl`, a system python script needing the system's python modules, which the
    /// bundled `python3` in the shims doesn't have.
    fn _path_without_shims() -> OsString {
        let shims = tools::bin_dir();
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::env::split_paths(&path).filter(|dir| *dir != shims)).unwrap_or(path)
    }

    /// What the sandbox runs, `$1` Proton's folder and the rest the game and its arguments: Proton's
    /// `waitforexitandrun`, the verb Steam uses (it lets the prefix's previous session end, then runs
    /// the game), then `wineserver -w`, so Wine's server ends on its own and saves the prefix's
    /// registry, which closing the sandbox under it would lose. The game's exit status is the run's.
    const RUN_SCRIPT: &str =
        r#"proton=$1; shift; "$proton/proton" waitforexitandrun "$@"; status=$?; "$proton/files/bin/wineserver" -w 2>/dev/null; exit $status"#;

    /// Where this program is put in the sandbox, read-only, to set the syscall filter in place.
    const FILTER_IN_SANDBOX: &str = "/run/bashrs";
    /// What this program is started with in the sandbox to set the syscall filter in place and run
    /// the rest of its arguments ([`_filtered_exec`]): a marker no command is named.
    const FILTERED_MARKER: &str = "--gm-run-exe-filtered";
    /// Where each game's own folder is made, in the home: in `~/Games`, beside the user's other
    /// games, not in `~/.bashrs` (the developer's call).
    const PREFIXES_DIR: &str = "Games/manual_run_proton_prefixes";
    /// The Wine prefix proper — the C: drive and the registry — in a game's own folder, as Proton
    /// keeps it in the folder it's given.
    const PFX_DIR: &str = "pfx";
    /// The home the game sees, in its own folder.
    const HOME_DIR: &str = "home";
    /// The Steam client folder Proton is told of, in the game's own folder: empty, as there's no
    /// Steam in the sandbox.
    const STEAM_CLIENT_DIR: &str = "steam";
    /// Where Proton's script finds python3, inside the sandbox.
    const SYSTEM_PYTHONS: &[&str] = &["/usr/bin/python3", "/bin/python3"];
    /// The system folders shown read-only (where they exist): links are kept as links.
    const SYSTEM_DIRS: &[&str] = &["/usr", "/etc", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/sys"];
    /// The device folders and nodes shown: GPUs, controllers, sound, and ntsync.
    const DEVICES: &[&str] = &["/dev/dri", "/dev/input", "/dev/snd", "/dev/ntsync"];
    /// The devices in `/dev` shown by their name's start: NVIDIA's GPU nodes, and the raw HID nodes
    /// controllers are read through.
    const NUMBERED_DEVICES: &[&str] = &["nvidia", "hidraw"];
    /// The sound servers' sockets in the runtime folder: PipeWire's, and PulseAudio's (which
    /// PipeWire serves as well).
    const SOUND_SOCKETS: &[&str] = &["pipewire-0", "pulse/native"];
    /// The X server's sockets.
    const X11_SOCKETS: &str = "/tmp/.X11-unix";
    /// The variables passed into the sandbox by name.
    const PASSED_ALONG: &[&str] = &["LANG", "LANGUAGE", "TZ", "USER", "LOGNAME"];
    /// The variables passed into the sandbox by their name's start: the locale's, and those that
    /// tune Proton, Wine, DXVK and VKD3D, the GPU drivers and Vulkan, SDL, and MangoHud.
    const PASSED_PREFIXES: &[&str] = &["LC_", "PROTON_", "WINE", "DXVK_", "VKD3D_", "__GL_", "MESA_", "RADV_", "VK_", "SDL_", "MANGOHUD"];
    /// The variables the run sets itself, which a passed-along one mustn't override.
    const NOT_PASSED: &[&str] = &["WINEPREFIX", "PROTON_ENABLE_WAYLAND", "PROTON_USE_WAYLAND"];
    /// The shader caches' settings, set where the user hasn't: NVIDIA's cache isn't trimmed to its
    /// size limit, and Mesa's may grow to 10 GB.
    const SHADER_CACHES: &[(&str, &str)] = &[("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1"), ("MESA_SHADER_CACHE_MAX_SIZE", "10G")];
    /// The performance wrappers, the first one installed taken.
    const PERFORMANCE_WRAPPERS: &[&str] = &["game-performance", "gamemoderun"];

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        /// A scratch directory for one test, gone afterwards.
        fn scratch(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("bashrs_gaming_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// A receipt the way the app writes one (gzipped JSON, the game's record in it), in a new
        /// install folder.
        fn install(folder: &Path, url: &str, id: u64) {
            std::fs::create_dir_all(folder.join(".itch")).unwrap();
            let json = format!(r#"{{"game":{{"id":{id},"url":"{url}","title":"x"}},"upload":{{"type":"html"}}}}"#);
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            gz.write_all(json.as_bytes()).unwrap();
            std::fs::write(folder.join(RECEIPT), gz.finish().unwrap()).unwrap();
        }

        /// A page reads the same however it was pasted, and an address that is no game page is
        /// refused rather than looked up.
        #[test]
        fn a_game_page_is_read_from_an_address_as_pasted() {
            let page = GamePage { creator: "quantumgames-studio".into(), game: "the-machinegg".into() };
            for address in [
                "https://quantumgames-studio.itch.io/the-machinegg",
                "http://quantumgames-studio.itch.io/the-machinegg/",
                "quantumgames-studio.itch.io/the-machinegg",
                "  https://QuantumGames-Studio.itch.io/The-MachinEGG  ",
                "https://quantumgames-studio.itch.io/the-machinegg?ref=x#top",
                "https://quantumgames-studio.itch.io/the-machinegg/devlog/1/update",
            ] {
                assert_eq!(GamePage::parse(address).as_ref(), Some(&page), "{address}");
            }
            assert_eq!(page.url(), "https://quantumgames-studio.itch.io/the-machinegg");
            for address in [
                "https://itch.io/games",
                "https://creator.itch.io",
                "https://creator.itch.io/",
                "https://html-classic.itch.zone/html/123/index.html",
                "https://www.creator.itch.io/game",
                "",
            ] {
                assert_eq!(GamePage::parse(address), None, "{address}");
            }
        }

        /// A game is named by its page, with or without the scheme, or by its name alone; what is
        /// neither is refused.
        #[test]
        fn a_game_is_named_by_its_page_or_by_its_name() {
            let page = || Query::Page(GamePage { creator: "c".into(), game: "the-machinegg".into() });
            assert_eq!(Query::parse("https://c.itch.io/the-machinegg"), Some(page()));
            assert_eq!(Query::parse("c.itch.io/the-machinegg"), Some(page()));
            assert_eq!(Query::parse("  The-MachinEGG "), Some(Query::Name("the-machinegg".into())));
            assert_eq!(Query::parse("Plant Daddy"), Some(Query::Name("plant daddy".into())), "a title");
            assert_eq!(Query::parse("Dr. Wörm's Game"), Some(Query::Name("dr. wörm's game".into())), "a title with a dot is no address");
            for neither in ["", "  ", "c.example.com/game", "https://itch.io/games", "c/game"] {
                assert_eq!(Query::parse(neither), None, "{neither:?}");
            }
        }

        /// Where the app's data is, by Electron's rule: an absolute `XDG_CONFIG_HOME`, else
        /// `~/.config`.
        #[test]
        fn the_data_folder_follows_xdg_config_home() {
            let home = Path::new("/home/u");
            assert_eq!(_itch_dir(None, home), Path::new("/home/u/.config/itch"));
            assert_eq!(_itch_dir(Some("/cfg".as_ref()), home), Path::new("/cfg/itch"));
            assert_eq!(_itch_dir(Some("relative".as_ref()), home), Path::new("/home/u/.config/itch"), "ignored, per the XDG spec");
            assert_eq!(_itch_dir(Some("".as_ref()), home), Path::new("/home/u/.config/itch"), "set but empty is unset");
        }

        /// A receipt gives the game's page and id; a record missing either gives nothing. So does
        /// itch.io's `data.json`, by its `id`.
        #[test]
        fn receipts_and_page_data_give_the_id() {
            let receipt = r#"{"game":{"id":4395686,"url":"https://c.itch.io/g","title":"G"},"files":[]}"#;
            let game = |title: &str| Receipt { id: 4395686, url: "https://c.itch.io/g".to_string(), title: title.to_string() };
            assert_eq!(_game_in(receipt), Some(game("G")));
            assert_eq!(_game_in(r#"{"game":{"id":4395686,"url":"https://c.itch.io/g"}}"#), Some(game("")), "the title is optional");
            assert_eq!(_game_in(r#"{"game":{"url":"https://c.itch.io/g"}}"#), None);
            assert_eq!(_game_in(r#"{"upload":{"id":1}}"#), None);
            assert_eq!(_game_in("not json"), None);
            assert_eq!(_id_in(r#"{"id":4395686,"title":"G","authors":[]}"#), Some(4395686));
            assert_eq!(_id_in(r#"{"errors":["invalid game"]}"#), None);
        }

        /// itch.io's answer tells three things apart: an id, no such game, and no way to reach
        /// itch.io at all (a failed curl), which is said as being offline.
        #[test]
        fn itch_ios_answer_tells_an_id_from_no_game_from_no_connection() {
            assert_eq!(_published_answer(0, "{\"id\":42}\n200"), Ok(42));
            assert_eq!(_published_answer(0, "<html>not found</html>\n404"), Err("itch.io has no game there".to_string()));
            assert_eq!(_published_answer(0, "busy\n503"), Err("itch.io answered 503".to_string()));
            assert_eq!(_published_answer(0, "{}\n200"), Err("itch.io's answer had no id in it".to_string()));
            for offline in [6, 7, 28, 35] {
                assert_eq!(_published_answer(offline, ""), Err("itch.io couldn't be reached (is this computer online?)".to_string()), "{offline}");
            }
        }

        /// The database's answer reads into its games, installs and locations, whatever else it
        /// holds; anything that isn't JSON reads as nothing.
        #[test]
        fn the_databases_answer_reads_into_games_installs_and_locations() {
            let json = r#"{"games": [[42, "https://c.itch.io/g", "G"], ["bad"], [7, null, "no page"], [9, "https://c.itch.io/h", null]], "installs": [[42, "/games/g"]], "locations": ["/games"], "other": 1}"#;
            assert_eq!(
                _database_from(json),
                Some(Database {
                    games: vec![(42, "https://c.itch.io/g".to_string(), "G".to_string()), (9, "https://c.itch.io/h".to_string(), String::new())],
                    installs: vec![(42, PathBuf::from("/games/g"))],
                    locations: vec![PathBuf::from("/games")],
                })
            );
            assert_eq!(_database_from("{}"), Some(Database::default()));
            assert_eq!(_database_from("Traceback (most recent call last)"), None);
        }

        /// The query reads a real database of the app's shape, made here through the same python:
        /// games with their pages, installs in their location's folder or in a custom one, and the
        /// locations. Skipped, saying so, where no python with sqlite3 is at hand.
        #[test]
        fn the_query_reads_the_apps_database() {
            let dir = scratch("database");
            let db = dir.join("butler.db");
            let make = r#"import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
db.executescript('''
create table games (id integer primary key, url text, title text);
create table install_locations (id text primary key, path text);
create table caves (id text primary key, game_id integer, install_location_id text, install_folder_name text, custom_install_folder text);
insert into games values (42, 'https://c.itch.io/g', 'G'), (7, 'https://d.itch.io/other', 'O'), (8, null, 'no page');
insert into install_locations values ('appdata', '/home/u/.config/itch/apps'), ('disk', '/mnt/games');
insert into caves values ('a', 42, 'disk', 'g', null), ('b', 7, 'appdata', 'other', '/elsewhere/other'), ('c', 9, 'gone', 'x', null);
''')
db.commit()
"#;
            let made = Command::new(tools::resolve("python3")).args(["-I", "-c", make]).arg(&db).status();
            if !matches!(made, Ok(status) if status.success()) {
                eprintln!("skipped: no python3 with sqlite3 to make the test database");
                return;
            }
            let read = _database(&db).expect("the database reads");
            let mut games = read.games.clone();
            games.sort();
            let row = |id, url: &str, title: &str| (id, url.to_string(), title.to_string());
            assert_eq!(games, [row(7, "https://d.itch.io/other", "O"), row(42, "https://c.itch.io/g", "G")], "a game with no page left out");
            assert_eq!(read.installs, [(42, PathBuf::from("/mnt/games/g")), (7, PathBuf::from("/elsewhere/other"))]);
            assert_eq!(read.locations, [PathBuf::from("/home/u/.config/itch/apps"), PathBuf::from("/mnt/games")]);
            assert_eq!(_database(&dir.join("missing.db")), None);
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// What's known is found by page, by name (a page's last part, or an install's folder), and
        /// a name two games share finds both; installs that are gone aren't given.
        #[test]
        fn a_game_is_found_by_page_or_by_name_and_a_shared_name_finds_both() {
            let base = scratch("find");
            let (machinegg, renamed) = (base.join("the-machinegg"), base.join("Some Folder"));
            std::fs::create_dir_all(&machinegg).unwrap();
            std::fs::create_dir_all(&renamed).unwrap();
            let mut known = Known::default();
            known.add_game(42, "https://quantumgames-studio.itch.io/the-machinegg", "The MachinEGG");
            known.add_game(42, "https://ignored.itch.io/second-page-for-42", "ignored");
            known.add_game(7, "https://a.itch.io/twin", "Twin");
            known.add_game(8, "https://b.itch.io/twin", "Twin");
            known.add_game(9, "https://c.itch.io/renamed-game", "  A Renamed Game ");
            known.add_install(42, machinegg.clone());
            known.add_install(42, base.join("deleted-since"));
            known.add_install(9, renamed.clone());
            known.add_install(9, renamed.clone());

            let one = |id, installs: &[&PathBuf]| Found::One { id, installs: installs.iter().map(|path| path.to_path_buf()).collect() };
            assert_eq!(known.find(&Query::parse("quantumgames-studio.itch.io/the-machinegg").unwrap()), one(42, &[&machinegg]));
            assert_eq!(known.find(&Query::parse("the-machinegg").unwrap()), one(42, &[&machinegg]));
            assert_eq!(known.find(&Query::parse("renamed-game").unwrap()), one(9, &[&renamed]));
            assert_eq!(known.find(&Query::parse("a renamed game").unwrap()), one(9, &[&renamed]), "by its title");
            assert_eq!(known.find(&Query::parse("THE MACHINEGG").unwrap()), one(42, &[&machinegg]), "by its title, any case");
            assert_eq!(known.find(&Query::parse("ignored").unwrap()), Found::Nothing, "a second record of a game is no title of it");
            assert_eq!(known.find(&Query::parse("some folder").unwrap()), one(9, &[&renamed]), "an install's folder, as named");
            assert_eq!(known.find(&Query::parse("Some-Folder").unwrap()), Found::Nothing);
            let by_folder = Known { games: vec![], installs: vec![(9, base.join("folder-name"))] };
            std::fs::create_dir_all(base.join("folder-name")).unwrap();
            assert_eq!(by_folder.find(&Query::parse("FOLDER-NAME").unwrap()), one(9, &[&base.join("folder-name")]));
            let Found::Several(pages) = known.find(&Query::parse("twin").unwrap()) else { panic!("two games are called twin") };
            assert_eq!(pages.iter().map(GamePage::url).collect::<Vec<_>>(), ["https://a.itch.io/twin", "https://b.itch.io/twin"]);
            assert_eq!(known.find(&Query::parse("nothing-like-it").unwrap()), Found::Nothing);
            assert_eq!(known.find(&Query::parse("x.itch.io/the-machinegg").unwrap()), Found::Nothing, "another creator's page");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// Gathering reads the receipts in the default install location (and in the database's,
        /// when there is one) — every install, whatever its folder is called — and passes over the
        /// app's own folders. The lines name the installs, then the saves' folders that exist (the
        /// storage partition, then each sandbox home), then the id.
        #[test]
        fn receipts_are_gathered_and_printed_with_the_saves_that_exist() {
            let itch = scratch("gather");
            let apps = itch.join(APPS_DIR);
            install(&apps.join("the-machinegg"), "https://c.itch.io/the-machinegg", 42);
            install(&apps.join("renamed-copy"), "https://C.itch.io/the-machinegg/", 42);
            install(&apps.join("another"), "https://c.itch.io/another", 7);
            std::fs::create_dir_all(apps.join("downloads")).unwrap(); // the app's own, no receipt
            let known = Known::gather(&itch);
            let Found::One { id, installs } = known.find(&Query::parse("c.itch.io/the-machinegg").unwrap()) else { panic!("found") };
            assert_eq!(id, 42);
            assert_eq!(installs, [apps.join("renamed-copy"), apps.join("the-machinegg")]);
            let shown = |path: PathBuf| path.display().to_string();
            assert_eq!(_lines(&itch, &installs, 42), [shown(apps.join("renamed-copy")), shown(apps.join("the-machinegg")), "42".to_string()]);

            let partition = itch.join(PARTITIONS_DIR).join("gamesession_42");
            std::fs::create_dir_all(&partition).unwrap();
            std::fs::create_dir_all(apps.join("the-machinegg").join(SANDBOX_HOME)).unwrap();
            assert_eq!(
                _lines(&itch, &installs, 42),
                [
                    shown(apps.join("renamed-copy")),
                    shown(apps.join("the-machinegg")),
                    shown(partition.clone()),
                    shown(apps.join("the-machinegg").join(SANDBOX_HOME)),
                    "42".to_string(),
                ]
            );
            assert_eq!(_lines(&itch, &[], 42), [shown(partition), "42".to_string()], "uninstalled, its storage kept");
            assert!(_receipts(&itch.join("no-such-location")).is_empty());
            let _ = std::fs::remove_dir_all(&itch);
        }

        /// A Steam game is named by its id — as such, or in a store or community page, or in a
        /// `steam://` link; a non-Steam game's 64-bit id read down to its own — or by its name; what
        /// is neither is refused.
        #[test]
        fn a_steam_game_is_named_by_its_id_in_any_form_or_by_its_name() {
            let long = ((3_640_112_877_u64 << 32) | SHORTCUT_GAME_ID).to_string();
            for (input, id) in [
                ("620".to_string(), 620),
                ("  620 ".to_string(), 620),
                ("https://store.steampowered.com/app/620/Portal_2/".to_string(), 620),
                ("store.steampowered.com/app/620".to_string(), 620),
                ("http://Store.SteamPowered.com/APP/620/Portal_2?snr=1_7_15#top".to_string(), 620),
                ("https://steamcommunity.com/app/620/discussions/".to_string(), 620),
                ("steam://rungameid/620".to_string(), 620),
                ("steam://run/620//-novid".to_string(), 620),
                ("3640112877".to_string(), 3_640_112_877),
                (long.clone(), 3_640_112_877),
                (format!("steam://rungameid/{long}"), 3_640_112_877),
            ] {
                assert_eq!(SteamQuery::parse(&input), Some(SteamQuery::Id(id)), "{input}");
            }
            assert_eq!(SteamQuery::parse(" Portal 2 "), Some(SteamQuery::Name("portal 2".into())));
            assert_eq!(SteamQuery::parse("Half-Life: Alyx"), Some(SteamQuery::Name("half-life: alyx".into())), "a colon makes no address");
            assert_eq!(SteamQuery::parse("123456789012"), Some(SteamQuery::Name("123456789012".into())), "no app's id, nor a non-Steam game's");
            for neither in [
                "",
                "  ",
                "https://store.steampowered.com/sub/123",
                "https://example.com/app/620",
                "steam://store/620",
                "store.steampowered.com/app/",
                "store.steampowered.com/app/x",
            ] {
                assert_eq!(SteamQuery::parse(neither), None, "{neither:?}");
            }
        }

        /// A manifest's pairs are read at any depth, their escapes undone; a section's name and a
        /// string left open are none. The app is its id, title and install folder; no id, no app.
        #[test]
        fn an_app_manifest_reads_into_its_id_title_and_folder() {
            let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"620\"\n\t\"name\"\t\t\"Portal \\\"2\\\" \\\\ more\"\n\t\"installdir\"\t\t\"Portal 2\"\n\t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"english\"\n\t}\n\t\"broken\"\t\t\"no end\n}\n";
            let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
            assert_eq!(_vdf_pairs(acf), [pair("appid", "620"), pair("name", "Portal \"2\" \\ more"), pair("installdir", "Portal 2"), pair("language", "english")]);
            let steamapps = Path::new("/library/steamapps");
            let portal = SteamGame { id: 620, title: "Portal \"2\" \\ more".into(), folder: Some(PathBuf::from("/library/steamapps/common/Portal 2")), exe: None };
            assert_eq!(_manifest_game(acf, steamapps), Some(portal));
            assert_eq!(_manifest_game("\"AppState\"\n{\n\t\"name\"\t\t\"no id\"\n}\n", steamapps), None);
        }

        /// A `shortcuts.vdf` the way Steam writes one: binary KeyValues, a `shortcuts` map holding a
        /// map per game (`0`, `1`, …) — its id (left out for `None`), its title and .exe under
        /// `keys` (Steam has written `AppName` and `Exe`, and both in lower case), a number and its
        /// tags.
        fn shortcuts_vdf(games: &[(Option<u32>, &str, &str)], keys: [&str; 2]) -> Vec<u8> {
            let mut vdf = b"\x00shortcuts\x00".to_vec();
            for (index, (id, title, exe)) in games.iter().enumerate() {
                vdf.extend(format!("\x00{index}\x00").bytes());
                if let Some(id) = id {
                    vdf.extend(b"\x02appid\x00");
                    vdf.extend(id.to_le_bytes());
                }
                vdf.extend(format!("\x01{}\x00{title}\x00\x01{}\x00{exe}\x00", keys[0], keys[1]).bytes());
                vdf.extend(b"\x02LastPlayTime\x00\x10\x00\x00\x00");
                vdf.extend(b"\x00tags\x00\x010\x00favorite\x00\x08");
                vdf.push(BINARY_MAP_END);
            }
            vdf.extend([BINARY_MAP_END, BINARY_MAP_END]);
            vdf
        }

        /// The non-Steam games come out of the binary file with the id Steam gave each, under
        /// either spelling of its keys, and their .exe's folder, quoted or not, named in any
        /// script; one with no id is passed over, one that is no file has no folder, and a file cut
        /// short, or holding a type this doesn't read, gives none.
        #[test]
        fn non_steam_games_are_read_from_the_accounts_shortcuts_vdf() {
            let games = [
                (Some(3_640_112_877), "itchio-Cirno-ice-shave", "\"/games/ice/チルノの最強かき氷大作戦！/Game.exe\""),
                (Some(3_860_769_506), "Tool.exe", "/tools/tool.exe"),
                (None, "No Id.exe", "/x/no-id.exe"),
                (Some(2_200_000_000), "Flatpak", "flatpak"),
            ];
            let game = |id, title: &str, exe: Option<&str>| SteamGame {
                id,
                title: title.into(),
                folder: exe.and_then(|exe| Path::new(exe).parent()).map(Path::to_path_buf),
                exe: exe.map(PathBuf::from),
            };
            let expected = [
                game(3_640_112_877, "itchio-Cirno-ice-shave", Some("/games/ice/チルノの最強かき氷大作戦！/Game.exe")),
                game(3_860_769_506, "Tool.exe", Some("/tools/tool.exe")),
                game(2_200_000_000, "Flatpak", None),
            ];
            let whole = shortcuts_vdf(&games, ["AppName", "Exe"]);
            assert_eq!(_shortcuts_in(&whole), expected);
            assert_eq!(_shortcuts_in(&shortcuts_vdf(&games, ["appname", "exe"])), expected, "the older spelling");
            assert!(_shortcuts_in(&whole[..whole.len() - 3]).is_empty(), "cut short");
            assert!(_shortcuts_in(b"").is_empty());
            assert!(_shortcuts_in(b"\x00shortcuts\x00\x09odd\x00\x08\x08").is_empty(), "a type it doesn't read");
        }

        /// A Steam home: a library besides Steam's own folder (which `libraryfolders.vdf` lists
        /// too), a game installed in each, one with its prefix and its Steam Cloud folder, and the
        /// non-Steam games an account added, one in a folder named in Japanese, with its prefix in
        /// Steam's own library. Each game is found by its id in any form, or by a name; a name two
        /// share finds both, and an id needs nothing known of it. The lines give the installs, the
        /// saves' folders that exist, the id.
        #[test]
        fn steam_games_are_found_by_id_or_name_and_printed_with_their_prefix_and_cloud_folder() {
            let home = scratch("steam");
            let steam = home.join(".local/share/Steam");
            let disk = home.join("disk");
            std::fs::create_dir_all(steam.join("steamapps")).unwrap();
            let listed = format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n",
                steam.display(),
                disk.display()
            );
            std::fs::write(steam.join("steamapps/libraryfolders.vdf"), listed).unwrap();
            let install = |library: &Path, id: u32, title: &str, folder: &str| {
                std::fs::create_dir_all(library.join("steamapps/common").join(folder)).unwrap();
                let acf = format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{id}\"\n\t\"name\"\t\t\"{title}\"\n\t\"installdir\"\t\t\"{folder}\"\n}}\n");
                std::fs::write(library.join(format!("steamapps/appmanifest_{id}.acf")), acf).unwrap();
            };
            install(&disk, 620, "Portal 2", "Portal 2");
            install(&steam, 400, "Portal", "Portal");
            let account = steam.join("userdata/76197245");
            std::fs::create_dir_all(account.join("config")).unwrap();
            std::fs::create_dir_all(account.join("620/remote")).unwrap();
            let ice = home.join("games/ice/チルノの最強かき氷大作戦！");
            std::fs::create_dir_all(&ice).unwrap();
            let exe = format!("\"{}\"", ice.join("Game.exe").display());
            let added = [(Some(3_640_112_877), "itchio-Cirno-ice-shave", exe.as_str()), (Some(3_000_000_000), "Portal 2.exe", "/gone/Portal 2.exe")];
            std::fs::write(account.join("config/shortcuts.vdf"), shortcuts_vdf(&added, ["AppName", "Exe"])).unwrap();
            for prefix in [disk.join("steamapps/compatdata/620"), steam.join("steamapps/compatdata/3640112877")] {
                std::fs::create_dir_all(prefix).unwrap();
            }

            let known = SteamKnown::gather(&home);
            assert_eq!(known.libraries, [steam.clone(), disk.clone()], "Steam's own library once, though listed again");
            let find = |input: &str| known.find(&SteamQuery::parse(input).unwrap());
            let portal_2 = SteamFound::One { id: 620, installs: vec![disk.join("steamapps/common/Portal 2")] };
            assert_eq!(find("620"), portal_2);
            assert_eq!(find("store.steampowered.com/app/620/Portal_2"), portal_2);
            assert_eq!(find("PORTAL"), SteamFound::One { id: 400, installs: vec![steam.join("steamapps/common/Portal")] });
            for name in ["itchio-Cirno-ice-shave", "ITCHIO-CIRNO-ICE-SHAVE", "チルノの最強かき氷大作戦！"] {
                assert_eq!(find(name), SteamFound::One { id: 3_640_112_877, installs: vec![ice.clone()] }, "{name}");
            }
            let twins = SteamFound::Several(vec![(620, "Portal 2".into()), (3_000_000_000, "Portal 2.exe".into())]);
            assert_eq!(find("portal 2"), twins, "a title, and a non-Steam game's without its .exe");
            assert_eq!(find("half-life"), SteamFound::Nothing);
            assert_eq!(find("999"), SteamFound::One { id: 999, installs: vec![] });

            let shown = |path: &Path| path.display().to_string();
            let portal_2_lines = [&disk.join("steamapps/common/Portal 2"), &disk.join("steamapps/compatdata/620"), &account.join("620")].map(|path| shown(path));
            assert_eq!(_steam_lines(&known, &[disk.join("steamapps/common/Portal 2")], 620), [&portal_2_lines[..], &["620".to_string()]].concat());
            assert_eq!(
                _steam_lines(&known, std::slice::from_ref(&ice), 3_640_112_877),
                [shown(&ice), shown(&steam.join("steamapps/compatdata/3640112877")), "3640112877".to_string()]
            );
            assert_eq!(_steam_lines(&known, &[], 999), ["999".to_string()]);
            let _ = std::fs::remove_dir_all(&home);
        }

        /// The prefixes Steam made for an .exe are found by the Steam entries that are its game — a
        /// non-Steam game added with that .exe, even through a link, and an app installed around it
        /// — each with the prefix Proton made under its id, in whichever library it is. An entry
        /// whose prefix Steam made but never filled, and another game's, are no match.
        #[test]
        fn the_prefixes_steam_made_for_an_exe_are_found_by_its_entries() {
            let home = scratch("past_prefixes");
            let steam = home.join(".local/share/Steam");
            let disk = home.join("disk");
            std::fs::create_dir_all(steam.join("steamapps")).unwrap();
            let listed = format!("\"libraryfolders\"\n{{\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n}}\n", disk.display());
            std::fs::write(steam.join("steamapps/libraryfolders.vdf"), listed).unwrap();
            let portal = disk.join("steamapps/common/Portal 2");
            let (ice, other) = (home.join("games/ice"), home.join("games/other"));
            for folder in [&portal.join("bin"), &ice, &other] {
                std::fs::create_dir_all(folder).unwrap();
            }
            for exe in [portal.join("bin/portal2.exe"), ice.join("Game.exe"), other.join("other.exe")] {
                std::fs::write(exe, "MZ").unwrap();
            }
            let manifest = "\"AppState\"\n{\n\t\"appid\"\t\t\"620\"\n\t\"name\"\t\t\"Portal 2\"\n\t\"installdir\"\t\t\"Portal 2\"\n}\n";
            std::fs::write(disk.join("steamapps/appmanifest_620.acf"), manifest).unwrap();
            std::os::unix::fs::symlink(home.join("games"), home.join("linked")).unwrap();
            let quoted = |exe: PathBuf| format!("\"{}\"", exe.display());
            let (through_link, ice_exe, other_exe) = (quoted(home.join("linked/ice/Game.exe")), quoted(ice.join("Game.exe")), quoted(other.join("other.exe")));
            let added = [
                (Some(3_640_112_877), "itchio-Cirno-ice-shave", through_link.as_str()),
                (Some(3_000_000_001), "Ice, again", ice_exe.as_str()),
                (Some(3_000_000_002), "Ice, never run", ice_exe.as_str()),
                (Some(3_000_000_003), "Other", other_exe.as_str()),
            ];
            std::fs::create_dir_all(steam.join("userdata/1/config")).unwrap();
            std::fs::write(steam.join("userdata/1/config/shortcuts.vdf"), shortcuts_vdf(&added, ["AppName", "Exe"])).unwrap();
            let compat = |library: &Path, id: &str| library.join("steamapps/compatdata").join(id);
            for (library, id) in [(&steam, "3640112877"), (&steam, "3000000001"), (&steam, "3000000003"), (&disk, "620")] {
                std::fs::create_dir_all(compat(library, id).join(PFX_DIR)).unwrap();
            }
            std::fs::create_dir_all(compat(&steam, "3000000002")).unwrap(); // made, never run

            assert_eq!(
                _steam_prefixes_of(&ice.join("Game.exe"), &home),
                [("itchio-Cirno-ice-shave".to_string(), compat(&steam, "3640112877")), ("Ice, again".to_string(), compat(&steam, "3000000001"))]
            );
            assert_eq!(_steam_prefixes_of(&portal.join("bin/portal2.exe"), &home), [("Portal 2".to_string(), compat(&disk, "620"))]);
            assert!(_steam_prefixes_of(&home.join("games/unknown/game.exe"), &home).is_empty());
            let past = _past_prefixes(&other.join("other.exe"), &home);
            assert_eq!(past, [PastPrefix { launcher: "Steam", game: "Other".into(), folder: compat(&steam, "3000000003") }]);
            assert_eq!(past[0].described(), format!("Steam's prefix for Other ({})", compat(&steam, "3000000003").display()));
            let _ = std::fs::remove_dir_all(&home);
        }

        /// A prefix is copied whole — its links as links, relative and absolute, its files' modes
        /// kept — and its source stays; a copy that fails leaves nothing behind, where it was to go
        /// or beside it.
        #[test]
        fn a_prefix_is_copied_whole_or_not_at_all() {
            use std::os::unix::fs::PermissionsExt;
            let base = scratch("copy_prefix");
            let from = base.join("compatdata/42");
            std::fs::create_dir_all(from.join("pfx/drive_c/users/steamuser")).unwrap();
            std::fs::create_dir_all(from.join("pfx/dosdevices")).unwrap();
            std::fs::write(from.join("pfx/user.reg"), "WINE REGISTRY").unwrap();
            std::fs::set_permissions(from.join("pfx/user.reg"), std::fs::Permissions::from_mode(0o600)).unwrap();
            std::os::unix::fs::symlink("../drive_c", from.join("pfx/dosdevices/c:")).unwrap();
            std::os::unix::fs::symlink("/", from.join("pfx/dosdevices/z:")).unwrap();

            let to = base.join("prefixes/game-1");
            assert_eq!(_copy_prefix(&from, &to), Ok(()));
            assert_eq!(std::fs::read_to_string(to.join("pfx/user.reg")).unwrap(), "WINE REGISTRY");
            assert_eq!(std::fs::metadata(to.join("pfx/user.reg")).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::read_link(to.join("pfx/dosdevices/c:")).unwrap(), Path::new("../drive_c"));
            assert_eq!(std::fs::read_link(to.join("pfx/dosdevices/z:")).unwrap(), Path::new("/"));
            assert!(to.join("pfx/drive_c/users/steamuser").is_dir());
            assert!(from.join("pfx/user.reg").is_file(), "the source stays");
            let names = |dir: &Path| std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
            assert_eq!(names(&base.join("prefixes")), ["game-1"], "nothing beside it");

            let failed = _copy_prefix(&base.join("no-such-prefix"), &base.join("prefixes/game-2"));
            assert!(failed.is_err(), "{failed:?}");
            assert_eq!(names(&base.join("prefixes")), ["game-1"], "nothing made, where it was to go or beside it");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// A build date is its day in UTC.
        #[test]
        fn a_build_time_reads_as_its_date() {
            assert_eq!(_date(0), "1970-01-01");
            assert_eq!(_date(951_782_400), "2000-02-29");
            assert_eq!(_date(1_792_530_497), "2026-10-20");
            assert_eq!(_date(1_794_497_090), "2026-11-12");
            assert_eq!(_date(4_107_542_399), "2100-02-28");
        }

        /// A stand-in for an installed Proton: just what choosing reads.
        fn proton(name: &str, built: u64, wayland: bool, bundled: bool) -> Proton {
            Proton { dir: PathBuf::from(format!("/protons/{name}")), name: name.to_string(), built, wayland, bundled }
        }

        /// The newest runs the game; on Wayland, the newest with a Wayland driver, saying the newest
        /// has none; where none has one, the newest on X11. With the bundle preferred, the bundle,
        /// however old. One asked for by name (or part of one) is taken as asked, on X11 when it
        /// has no Wayland driver; a name nothing has is refused.
        #[test]
        fn the_newest_proton_is_chosen_with_a_wayland_driver_where_wayland_is_wanted() {
            let protons = [proton("experimental-11.0", 300, false, false), proton("cachyos-11.0", 250, true, false), proton("GE-Proton9", 100, true, true)];
            let pick = |asked, wayland, bundle_first| {
                let choice = _choose(&protons, asked, wayland, bundle_first).unwrap();
                (choice.proton.name.clone(), choice.wayland, choice.note)
            };
            assert_eq!(pick(None, false, false), ("experimental-11.0".into(), false, None));
            let (name, wayland, note) = pick(None, true, false);
            assert_eq!((name.as_str(), wayland), ("cachyos-11.0", true));
            assert!(note.unwrap().contains("the newest Proton, experimental-11.0, has no Wayland driver"));
            assert_eq!(pick(None, true, true), ("GE-Proton9".into(), true, None), "the bundle first when bashrs's own is the pick");
            assert_eq!(pick(Some("Experimental"), true, false).0, "experimental-11.0");
            assert!(!pick(Some("experimental"), true, false).1, "no driver, so X11");
            assert_eq!(pick(Some("ge-proton"), true, false), ("GE-Proton9".into(), true, None));
            assert!(_choose(&protons, Some("proton-tkg"), true, false).is_err());

            let x11_only = [proton("experimental-11.0", 300, false, false), proton("proton-9", 100, false, false)];
            let choice = _choose(&x11_only, None, true, false).unwrap();
            assert_eq!((choice.proton.name.as_str(), choice.wayland), ("experimental-11.0", false));
            assert!(choice.note.unwrap().contains("no Proton here has a Wayland driver"));
            assert!(_choose(&[], None, true, false).is_err());
        }

        /// `--proton` takes a Proton's folder, found by where it is, even through a link to it.
        #[test]
        fn a_proton_is_also_asked_for_by_its_folder() {
            let base = scratch("asked_folder");
            crate::tools::proton::tests::fake_proton(&base.join("custom"), "5 custom-build", false, "");
            std::os::unix::fs::symlink(base.join("custom"), base.join("link")).unwrap();
            let protons = [Proton { dir: base.join("custom"), name: "custom-build".into(), built: 5, wayland: false, bundled: false }];
            for asked in [base.join("custom"), base.join("link")] {
                let choice = _choose(&protons, Some(asked.to_str().unwrap()), false, false).unwrap();
                assert_eq!(choice.proton.dir, base.join("custom"));
            }
            let _ = std::fs::remove_dir_all(&base);
        }

        /// The listing marks the default pick and, where it differs, the pick with `--xwayland`.
        #[test]
        fn the_listing_marks_what_each_way_of_running_takes() {
            let protons = [proton("experimental-11.0", 1_794_497_090, false, false), proton("cachyos-11.0", 1_792_530_497, true, false)];
            let rows = _proton_rows(&protons, false);
            assert!(rows[0].contains("Proton") && rows[0].contains("Folder"), "{rows:?}");
            assert_eq!(rows[1], "experimental-11.0\t2026-11-12\tX11\twith --xwayland\t/protons/experimental-11.0");
            assert_eq!(rows[2], "cachyos-11.0\t2026-10-20\tWayland\tyes\t/protons/cachyos-11.0");
        }

        /// A computer with every kind of thing to show, for the sandbox's arguments.
        fn host() -> Host {
            Host {
                system: vec![(PathBuf::from("/usr"), None), (PathBuf::from("/bin"), Some(PathBuf::from("usr/bin")))],
                devices: vec![PathBuf::from("/dev/dri"), PathBuf::from("/dev/ntsync")],
                runtime_dir: Some(PathBuf::from("/run/user/1000")),
                wayland: Some(("wayland-0".into(), PathBuf::from("/run/user/1000/wayland-0"))),
                x11: Some((":0".into(), Some(PathBuf::from("/run/user/1000/xauth_abc")))),
                sound: vec![PathBuf::from("/run/user/1000/pipewire-0")],
                udev: Some(PathBuf::from("/run/udev")),
                resolver: Some(PathBuf::from("/run/systemd/resolve")),
                env: vec![("LANG".into(), "en_US.UTF-8".into()), ("DXVK_HUD".into(), "fps".into()), ("MESA_SHADER_CACHE_MAX_SIZE".into(), "2G".into())],
                overlays: true,
                userns_locked: true,
                filter_exe: Some(PathBuf::from("/opt/bashrs/bashrs")),
            }
        }

        /// Throwaway layers and `--disable-userns` came with bubblewrap 0.8.
        #[test]
        fn overlays_and_the_userns_lock_come_with_bubblewrap_0_8() {
            assert!(_from_0_8("bubblewrap 0.13.1\n"));
            assert!(_from_0_8("bubblewrap 0.8.0"));
            assert!(_from_0_8("bubblewrap 1.0"));
            assert!(!_from_0_8("bubblewrap 0.6.1"));
            assert!(!_from_0_8("nonsense"));
        }

        /// The arguments as text, to search through.
        fn words(args: &[OsString]) -> Vec<String> {
            args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect()
        }

        /// Whether `seq` appears in `args`, in order and side by side.
        fn has(args: &[String], seq: &[&str]) -> bool {
            args.windows(seq.len()).any(|window| window.iter().zip(seq).all(|(arg, want)| arg == want))
        }

        fn plan<'a>(network: bool, wayland: bool, allowed: &'a [PathBuf], game_args: &'a [String]) -> Plan<'a> {
            Plan {
                proton: Path::new("/protons/cachy"),
                exe: Path::new("/games/G/bin/g.exe"),
                game_dir: Path::new("/games/G"),
                prefix: Path::new("/home/u/Games/manual_run_proton_prefixes/G-1"),
                allowed,
                game_args,
                network,
                wayland,
            }
        }

        /// The sandbox by default: no network, its own IPC, a user namespace that can make no more,
        /// the system read-only with its links as links, devices, the Wayland socket (no X11),
        /// sound, Proton read-only, the game's folders read-write, run from the .exe's folder with a
        /// clean environment by this program, which sets the syscall filter in place first, the
        /// game's arguments last, untouched.
        #[test]
        fn the_sandbox_shows_the_system_read_only_and_lets_the_game_write_only_its_folders() {
            let game_args = ["-windowed".to_string(), "--level 2".to_string()];
            let args = words(&_bwrap_args(&plan(false, true, &[], &game_args), &host()));
            for seq in [
                &["--unshare-user", "--disable-userns"][..],
                &["--ro-bind", "/opt/bashrs/bashrs", FILTER_IN_SANDBOX],
                &["--unshare-net"],
                &["--unshare-ipc"],
                &["--new-session"],
                &["--die-with-parent"],
                &["--ro-bind", "/usr", "/usr"],
                &["--symlink", "usr/bin", "/bin"],
                &["--tmpfs", "/tmp"],
                &["--dev-bind", "/dev/dri", "/dev/dri"],
                &["--dev-bind", "/dev/ntsync", "/dev/ntsync"],
                &["--ro-bind", "/run/udev", "/run/udev"],
                &["--perms", "0700", "--dir", "/run/user/1000"],
                &["--ro-bind", "/run/user/1000/wayland-0", "/run/user/1000/wayland-0"],
                &["--ro-bind", "/run/user/1000/pipewire-0", "/run/user/1000/pipewire-0"],
                &["--overlay-src", "/protons/cachy", "--tmp-overlay", "/protons/cachy"],
                &["--bind", "/games/G", "/games/G"],
                &["--bind", "/home/u/Games/manual_run_proton_prefixes/G-1", "/home/u/Games/manual_run_proton_prefixes/G-1"],
                &["--chdir", "/games/G/bin", "--clearenv"],
                &["--setenv", "WAYLAND_DISPLAY", "wayland-0"],
                &["--setenv", "PROTON_ENABLE_WAYLAND", "1"],
                &["--", FILTER_IN_SANDBOX, FILTERED_MARKER, "/bin/sh", "-c", RUN_SCRIPT, "sh", "/protons/cachy", "/games/G/bin/g.exe", "-windowed", "--level 2"],
            ] {
                assert!(has(&args, seq), "{seq:?} in {args:?}");
            }
            assert!(args.last().is_some_and(|last| last == "--level 2"));
            for absent in [X11_SOCKETS, "/run/user/1000/xauth_abc", "DISPLAY", "/run/systemd/resolve", "--share-net"] {
                assert!(!args.iter().any(|arg| arg == absent), "{absent} has no place here: {args:?}");
            }
            let at = |seq: &[&str]| args.windows(seq.len()).position(|window| window.iter().zip(seq).all(|(arg, want)| arg == want)).unwrap();
            assert!(at(&["--tmpfs", "/tmp"]) < at(&["--perms"]), "the runtime folder after the fresh /tmp");
            assert!(at(&["--clearenv"]) < at(&["--setenv", "HOME"]), "the environment cleared before it's set");
            assert!(!has(&args, &["--ro-bind", "/protons/cachy", "/protons/cachy"]) && !has(&args, &["--bind", "/protons/cachy", "/protons/cachy"]));

            let older = Host { overlays: false, userns_locked: false, filter_exe: None, ..host() };
            let args = words(&_bwrap_args(&plan(false, true, &[], &game_args), &older));
            assert!(has(&args, &["--ro-bind", "/protons/cachy", "/protons/cachy"]), "read-only where there are no throwaway layers");
            assert!(has(&args, &["--", "/bin/sh", "-c", RUN_SCRIPT]), "no filter to set where there's none for the architecture");
            for absent in ["--tmp-overlay", "--disable-userns", "--unshare-user", FILTER_IN_SANDBOX, FILTERED_MARKER] {
                assert!(!args.iter().any(|arg| arg == absent), "{absent} has no place here: {args:?}");
            }
        }

        /// A folder the game may write to mayn't be, or hold, the user's own folders — the home;
        /// the folders `user-dirs.dirs` names, wherever they are, else their usual ones; the
        /// programs' settings, data, state and caches, where the environment says or usually; the
        /// folder every game's own is made in — or where a filesystem is mounted. A folder inside
        /// one of them is a game's own, and fine.
        #[test]
        fn a_folder_the_game_writes_to_mustnt_be_or_hold_the_users_own() {
            let home = _canonical(&scratch("broad"));
            std::fs::create_dir_all(home.join(".config")).unwrap();
            let user_dirs = "# xdg-user-dirs-update\nXDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\nXDG_MUSIC_DIR=\"/mnt/music\"\nXDG_DESKTOP_DIR=\"$HOME/\"\n";
            std::fs::write(home.join(".config/user-dirs.dirs"), user_dirs).unwrap();
            let env = |key: &str| match key {
                "XDG_CACHE_HOME" => Some(OsString::from("/var/cache/u")),
                "XDG_DATA_HOME" => Some(OsString::from("relative, so not taken")),
                _ => None,
            };
            let own = _users_folders(&home, &env);
            let mounts = [PathBuf::from("/"), PathBuf::from("/mnt/games")];
            let why = |folder: &Path| _too_broad(folder, &own, &mounts);
            let said = |text: &str| Some(text.to_string());
            assert_eq!(why(&home), said("it is your home folder"));
            assert_eq!(why(home.parent().unwrap()), said("it holds your home folder"));
            assert_eq!(why(&home.join("Téléchargements")), said("it is ~/Téléchargements, one of your own folders"));
            assert_eq!(why(Path::new("/mnt/music")), said("it is /mnt/music, one of your own folders"));
            assert_eq!(why(&home.join(".config")), said("it is ~/.config, one of your own folders"));
            assert_eq!(why(&home.join(".local")), said("it holds ~/.local/share, one of your own folders"));
            assert_eq!(why(Path::new("/var/cache/u")), said("it is /var/cache/u, one of your own folders"));
            assert_eq!(why(&home.join("Games")), said("it holds ~/Games/manual_run_proton_prefixes, where every game's own folder is made"));
            assert_eq!(why(Path::new("/mnt/games")), said("it is /mnt/games, where a filesystem is mounted"));
            assert_eq!(why(Path::new("/mnt")), said("it holds /mnt/music, one of your own folders"));
            for fine in [
                home.join("Downloads"),
                home.join("Téléchargements/Some Game"),
                home.join(".config/itch/apps/ice"),
                home.join("Games/Some Game"),
                home.join("Games/manual_run_proton_prefixes/Some Game-1"),
                PathBuf::from("/mnt/games/steamapps/common/G"),
            ] {
                assert_eq!(why(&fine), None, "{}", fine.display());
            }
            let _ = std::fs::remove_dir_all(&home);
        }

        /// With no `user-dirs.dirs`, or none named in it, the usual folders are the user's.
        #[test]
        fn the_usual_folders_are_the_users_where_none_are_named() {
            let home = Path::new("/home/u");
            let usual: Vec<PathBuf> = USUAL_USER_DIRS.iter().map(|name| home.join(name)).collect();
            assert_eq!(_user_dirs("", home), usual);
            assert_eq!(_user_dirs("# nothing but comments\nOTHER=1\n", home), usual);
            assert_eq!(_user_dirs("XDG_VIDEOS_DIR=\"$HOME/Clips\"\n", home), [PathBuf::from("/home/u/Clips")]);
        }

        /// Mount points are each line's fifth field, a space and the like unescaped.
        #[test]
        fn mount_points_are_read_from_mountinfo() {
            let info = b"21 1 0:19 / / rw - btrfs /dev/x rw\n36 21 0:30 / /run/media/u/My\\040Disk rw - ext4 /dev/y rw\n\n";
            assert_eq!(_mount_points_in(info), [PathBuf::from("/"), PathBuf::from("/run/media/u/My Disk")]);
            assert!(_mount_points_in(b"").is_empty());
            assert!(_mount_points().iter().any(|point| point == Path::new("/")), "this process sees its root mounted");
        }

        /// With the network, the resolver's folder is shown too; on X11, the X sockets (after the
        /// fresh `/tmp`) and the authority file, the host's IPC kept, and no Wayland at all; and the
        /// folders allowed are writable.
        #[test]
        fn the_network_and_x11_add_what_they_need_and_nothing_more() {
            let allowed = [PathBuf::from("/mnt/mods")];
            let args = words(&_bwrap_args(&plan(true, false, &allowed, &[]), &host()));
            assert!(has(&args, &["--ro-bind", "/run/systemd/resolve", "/run/systemd/resolve"]));
            assert!(has(&args, &["--ro-bind", X11_SOCKETS, X11_SOCKETS]));
            assert!(has(&args, &["--ro-bind", "/run/user/1000/xauth_abc", "/run/user/1000/xauth_abc"]));
            assert!(has(&args, &["--setenv", "DISPLAY", ":0"]));
            assert!(has(&args, &["--bind", "/mnt/mods", "/mnt/mods"]));
            for absent in ["--unshare-net", "--unshare-ipc", "/run/user/1000/wayland-0", "WAYLAND_DISPLAY", "PROTON_ENABLE_WAYLAND"] {
                assert!(!args.iter().any(|arg| arg == absent), "{absent} has no place here: {args:?}");
            }
            let at = |word: &str| args.iter().position(|arg| arg == word).unwrap();
            assert!(at("/tmp") < at(X11_SOCKETS));
        }

        /// The environment inside: Proton's folders, the home the game sees, the user's tuning
        /// passed along (their shader-cache size winning over the default), and the sync and
        /// cache settings.
        #[test]
        fn the_sandbox_environment_carries_protons_folders_and_the_users_tuning() {
            let env = _sandbox_env(&plan(false, true, &[], &[]), &host());
            let get = |key: &str| env.iter().find(|(known, _)| known == key).map(|(_, value)| value.to_string_lossy().into_owned());
            let prefix = "/home/u/Games/manual_run_proton_prefixes/G-1";
            assert_eq!(get("HOME"), Some(format!("{prefix}/home")));
            assert_eq!(get("STEAM_COMPAT_DATA_PATH").as_deref(), Some(prefix));
            assert_eq!(get("STEAM_COMPAT_CLIENT_INSTALL_PATH"), Some(format!("{prefix}/steam")));
            assert_eq!(get("STEAM_COMPAT_INSTALL_PATH").as_deref(), Some("/games/G"));
            assert_eq!(get("WINEPREFIX"), Some(format!("{prefix}/pfx")));
            assert_eq!(get("XDG_RUNTIME_DIR").as_deref(), Some("/run/user/1000"));
            assert_eq!(get("DXVK_HUD").as_deref(), Some("fps"));
            assert_eq!(get("LANG").as_deref(), Some("en_US.UTF-8"));
            assert_eq!(get("MESA_SHADER_CACHE_MAX_SIZE").as_deref(), Some("2G"), "the user's own wins");
            assert_eq!(get("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP").as_deref(), Some("1"));
            assert_eq!(env.iter().filter(|(key, _)| key == "HOME").count(), 1);
        }

        /// What goes in from the user's environment: locale, timezone, user name and tuning — not
        /// what the run sets itself, nor anything else of the session.
        #[test]
        fn only_locale_and_tuning_variables_are_passed_along() {
            for passed in ["LANG", "LC_ALL", "TZ", "PROTON_LOG", "DXVK_HUD", "VKD3D_CONFIG", "WINEDEBUG", "MANGOHUD", "MANGOHUD_CONFIG", "__GL_SYNC_TO_VBLANK", "SDL_GAMECONTROLLERCONFIG"] {
                assert!(_passed_along(passed), "{passed}");
            }
            for kept_out in ["WINEPREFIX", "PROTON_ENABLE_WAYLAND", "PROTON_USE_WAYLAND", "SSH_AUTH_SOCK", "DBUS_SESSION_BUS_ADDRESS", "HOME", "PATH", "GNUPGHOME", "DISPLAY"] {
                assert!(!_passed_along(kept_out), "{kept_out}");
            }
        }

        /// A game's own folder is named for its folder, with a checksum of where that is: the same
        /// game always gets the same one, and same-named games in different places get their own.
        #[test]
        fn each_game_gets_a_folder_of_its_own_named_for_it() {
            let one = _prefix_name(Path::new("/games/Hollow Knight"));
            assert!(one.starts_with("Hollow Knight-") && one.len() == "Hollow Knight-".len() + 8, "{one}");
            assert_eq!(one, _prefix_name(Path::new("/games/Hollow Knight")));
            assert_ne!(one, _prefix_name(Path::new("/other/Hollow Knight")));
        }

        /// A game named by its folder is the one `.exe` in it, Unity's crash handler and an
        /// uninstaller aside; a folder with several is answered with their names, as is one with
        /// none, or only helpers.
        #[test]
        fn a_game_named_by_its_folder_is_its_one_exe() {
            let base = scratch("exe_in");
            let folder = |name: &str, files: &[&str]| {
                let dir = base.join(name);
                std::fs::create_dir_all(dir.join("Data.exe")).unwrap(); // a folder named like one is none
                for file in files {
                    std::fs::write(dir.join(file), "MZ").unwrap();
                }
                dir
            };
            let unity = folder("unity", &["Some Game.exe", "UnityCrashHandler64.exe", "unins000.exe", "readme.txt"]);
            assert_eq!(_exe_in(&unity), Ok(unity.join("Some Game.exe")));
            let upper = folder("upper", &["GAME.EXE"]);
            assert_eq!(_exe_in(&upper), Ok(upper.join("GAME.EXE")));
            let two = folder("two", &["game.exe", "launcher.exe", "UnityCrashHandler64.exe"]);
            let several = _exe_in(&two).unwrap_err();
            assert!(several.ends_with("holds several .exe files — name the one to run: game.exe, launcher.exe"), "{several}");
            assert!(_exe_in(&folder("none", &["readme.txt"])).unwrap_err().ends_with("holds no .exe"));
            let helpers = _exe_in(&folder("helpers", &["unins000.exe", "vc_redist.x64.exe"])).unwrap_err();
            assert!(helpers.ends_with("holds only helpers (unins000.exe, vc_redist.x64.exe) — name the .exe to run"), "{helpers}");
            let _ = std::fs::remove_dir_all(&base);
        }

        /// The shims leave PATH for the performance wrapper; the rest stays, in order.
        #[test]
        fn the_wrappers_path_leaves_out_the_tool_shims() {
            let path = _path_without_shims();
            assert!(!std::env::split_paths(&path).any(|dir| dir == tools::bin_dir()));
            let kept: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|dir| *dir != tools::bin_dir()).collect();
            assert_eq!(std::env::split_paths(&path).collect::<Vec<_>>(), kept);
        }

        /// The run script runs Proton's verb with the game and its arguments as given, waits for
        /// Wine's server, and exits with the game's status. Run here by `sh` against a stand-in.
        #[test]
        fn the_run_script_passes_everything_and_keeps_the_games_status() {
            let base = scratch("script");
            let proton = base.join("proton-dir");
            std::fs::create_dir_all(proton.join("files/bin")).unwrap();
            let write = |path: PathBuf, body: &str| {
                std::fs::write(&path, body).unwrap();
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            };
            let log = base.join("log");
            write(proton.join("proton"), &format!("#!/bin/sh\nprintf '%s|' \"$@\" >> '{}'; exit 3\n", log.display()));
            write(proton.join("files/bin/wineserver"), &format!("#!/bin/sh\necho \" wineserver $*\" >> '{}'\n", log.display()));
            let status = Command::new("/bin/sh").args(["-c", RUN_SCRIPT, "sh"]).arg(&proton).args(["/g/game.exe", "-a b", "--c"]).status().unwrap();
            assert_eq!(status.code(), Some(3), "the game's status");
            assert_eq!(std::fs::read_to_string(&log).unwrap(), "waitforexitandrun|/g/game.exe|-a b|--c| wineserver -w\n");
            let _ = std::fs::remove_dir_all(&base);
        }
    }
}
