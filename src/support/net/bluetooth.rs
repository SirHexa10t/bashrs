//! Bluetooth adapters and the devices they can see — the engine behind `net_bluetooth`.
//!
//! # Why this one shells out
//!
//! Every other probe in this crate speaks its protocol directly, because it can: TCP and DNS are
//! sockets. Bluetooth is not. Enumeration goes through BlueZ, whose supported interface is D-Bus
//! (`org.bluez`), and reaching that from Rust means a D-Bus client — a substantial dependency, and
//! an async one, for a single command in a crate that has neither. The alternative interface is
//! `bluetoothctl`, BlueZ's own CLI, which ships with BlueZ and is therefore present on exactly the
//! machines where Bluetooth works at all. That is the trade taken here, deliberately and narrowly:
//! the subprocess boundary is [`run`], everything above it is parsing.
//!
//! # Structure, for something that cannot be tested end to end here
//!
//! The machine this was written on has no Bluetooth adapter, no BlueZ, and no
//! `/var/lib/bluetooth`, so the live path is unverified by construction. Everything that could be
//! made a pure function of captured `bluetoothctl` output therefore is — [`parse_devices`],
//! [`parse_info`], [`parse_adapter`] — and those carry the tests. What remains untested is the
//! subprocess call itself and the shape of a future BlueZ's output.
//!
//! # What each source gives
//!
//! - `/sys/class/bluetooth/` — the adapters, free and always readable. The one part that needs no
//!   BlueZ at all, so "you have no Bluetooth hardware" is distinguishable from "you have no BlueZ".
//! - `bluetoothctl devices` — everything BlueZ currently knows: paired, and anything seen recently.
//! - `bluetoothctl info <mac>` — per-device detail, including the `Icon` that says what it is.
//! - `bluetoothctl --timeout N scan on` — a timed discovery, which adds to what `devices` returns.

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::support::exec;

/// Where the kernel exposes Bluetooth adapters, BlueZ or no BlueZ.
const SYS_BLUETOOTH: &str = "/sys/class/bluetooth";

/// Where the kernel exposes radio kill-switches. A blocked adapter still appears under
/// [`SYS_BLUETOOTH`], so without consulting this, "the radio is switched off" and "nothing is
/// nearby" produce identical output — which is the single most confusing thing this command could
/// do to someone whose device is sitting right there, switched on.
const SYS_RFKILL: &str = "/sys/class/rfkill";

/// BlueZ's CLI — the interface this module speaks (see the module header).
const BLUETOOTHCTL: &str = "bluetoothctl";

/// A Bluetooth adapter on this machine.
#[derive(Debug, PartialEq, Eq, Clone)]
pub(crate) struct Adapter {
    /// The interface name, `hci0` and friends.
    pub name: String,
    /// Its own hardware address, when the kernel exposes one.
    pub address: Option<String>,
}

/// A device BlueZ knows about.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub(crate) struct BtDevice {
    pub address: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
    /// BlueZ's own icon name (`audio-headset`, `phone`, `input-keyboard`, …) — the closest thing
    /// to a device type it will tell us, and what [`kind`] renders.
    pub icon: Option<String>,
    /// Signal strength in dBm, present only while a device is actually in range.
    pub rssi: Option<i32>,
}

impl BtDevice {
    /// What the device is, in a word — BlueZ's icon translated out of freedesktop naming, since
    /// `audio-headset` is an icon name rather than something to show a person.
    pub(crate) fn kind(&self) -> &str {
        match self.icon.as_deref() {
            Some("audio-headset") | Some("audio-headphones") => "headphones",
            Some("audio-card") | Some("audio-speakers") => "speaker",
            Some("phone") => "phone",
            Some("computer") => "computer",
            Some("input-keyboard") => "keyboard",
            Some("input-mouse") => "mouse",
            Some("input-gaming") => "controller",
            Some("input-tablet") => "tablet",
            Some("camera-photo") | Some("camera-video") => "camera",
            Some("printer") => "printer",
            Some("multimedia-player") => "media player",
            Some("video-display") => "display",
            Some("network-wireless") => "network",
            Some("watch") => "watch",
            Some(other) => other,
            None => "—",
        }
    }
}

/// Whether the Bluetooth radio is switched off, and by what.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Radio {
    /// Not blocked — the radio is free to work.
    Live,
    /// Software-blocked: something turned it off (a desktop toggle, `rfkill block bluetooth`,
    /// aeroplane mode). Reversible from the keyboard.
    SoftBlocked,
    /// Hardware-blocked: a physical switch or a firmware setting. No software can undo it.
    HardBlocked,
    /// No Bluetooth kill-switch is registered — nothing to report either way.
    Unknown,
}

/// The state of this machine's Bluetooth radio, read from the kernel's kill-switch list.
pub(crate) fn radio() -> Radio {
    let Ok(entries) = std::fs::read_dir(SYS_RFKILL) else { return Radio::Unknown };
    let switches: Vec<Radio> =
        entries.flatten().map(|entry| entry.path()).filter_map(|path| read_switch(&path)).collect();
    // Hardest block wins: reporting "soft-blocked, run rfkill unblock" to someone whose laptop
    // switch is off would send them after a fix that cannot work.
    for state in [Radio::HardBlocked, Radio::SoftBlocked, Radio::Live] {
        if switches.contains(&state) {
            return state;
        }
    }
    Radio::Unknown
}

/// One rfkill switch, if it governs Bluetooth. `soft`/`hard` are `0`/`1`; anything unreadable
/// (a dangling symlink into a device this container cannot see, say) yields `None`.
fn read_switch(path: &Path) -> Option<Radio> {
    let field = |name: &str| std::fs::read_to_string(path.join(name)).ok();
    if field("type")?.trim() != "bluetooth" {
        return None;
    }
    let blocked = |name: &str| field(name).is_some_and(|value| value.trim() == "1");
    Some(match (blocked("hard"), blocked("soft")) {
        (true, _) => Radio::HardBlocked,
        (false, true) => Radio::SoftBlocked,
        (false, false) => Radio::Live,
    })
}

/// What `bluetoothctl show` reports about the default controller: whether one exists at all, and
/// whether it is powered. `None` when the command could not be run.
pub(crate) fn controller_powered() -> Option<Option<bool>> {
    Some(parse_show(&run(&["show"])?))
}

/// Parse `bluetoothctl show`. `None` means it named no controller — BlueZ is installed but its
/// service isn't running, or no adapter is bound. `Some(powered)` is the controller's state.
pub(crate) fn parse_show(text: &str) -> Option<bool> {
    // A running service prints `Controller <mac> (public)` and then indented `Key: value` lines.
    text.lines().any(|line| line.trim_start().starts_with("Controller ")).then_some(())?;
    let powered = text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key.trim() == "Powered").then(|| value.trim() == "yes")
    });
    Some(powered.unwrap_or(false))
}

/// The adapters this machine has. Empty means no Bluetooth hardware (or none the kernel bound),
/// which is a different problem from BlueZ being absent — and worth saying differently.
pub(crate) fn adapters() -> Vec<Adapter> {
    let Ok(entries) = std::fs::read_dir(SYS_BLUETOOTH) else { return Vec::new() };
    let mut found: Vec<Adapter> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter_map(|path| parse_adapter(&path))
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// One adapter, from its sysfs directory. Split out so the naming and address rules are testable
/// against a fabricated tree rather than requiring hardware.
fn parse_adapter(path: &Path) -> Option<Adapter> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    // Only the controllers themselves (`hci0`), not the per-connection children (`hci0:42`) the
    // same directory carries once something is connected.
    if !name.starts_with("hci") || name.contains(':') {
        return None;
    }
    let address = std::fs::read_to_string(path.join("address"))
        .ok()
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty());
    Some(Adapter { name, address })
}

/// Whether BlueZ's CLI is installed — the difference between "nothing is paired" and "nothing can
/// be asked".
pub(crate) fn tooling_present() -> bool {
    exec::on_path(BLUETOOTHCTL)
}

/// Run `bluetoothctl` with `args` and hand back its stdout. The single subprocess boundary of this
/// module; everything else parses what it returns.
fn run(args: &[&str]) -> Option<String> {
    exec::capture_stdout(BLUETOOTHCTL, args)
}

/// Every device BlueZ currently knows: paired ones, and anything seen recently enough to still be
/// cached. `None` when `bluetoothctl` could not be run at all.
pub(crate) fn known_devices() -> Option<Vec<BtDevice>> {
    Some(parse_devices(&run(&["devices"])?))
}

/// Run a timed discovery, then report everything known afterwards — the newly-seen devices
/// included, since discovery adds them to the same cache `devices` reads.
///
/// Parsing the scan's own live output is deliberately avoided: it is a stream of `[NEW]`/`[CHG]`
/// lines designed for a human watching it, and re-reading the cache once it finishes gets the same
/// devices in the one format that is actually specified.
pub(crate) fn scan(seconds: u32) -> Option<Vec<BtDevice>> {
    let timeout = seconds.to_string();
    let _ = run(&["--timeout", &timeout, "scan", "on"]);
    known_devices()
}

/// Parse `bluetoothctl devices` — one `Device <address> <name>` per line.
///
/// Anything else in the stream is ignored rather than guessed at: BlueZ prints controller banners
/// and agent chatter into the same output, and a line that doesn't start with `Device` is not one.
pub(crate) fn parse_devices(text: &str) -> Vec<BtDevice> {
    let mut found: Vec<BtDevice> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("Device ")?;
            // A device BlueZ has no name for still prints, as the address alone — so the split is
            // optional, not required. Requiring it silently dropped exactly the devices whose
            // identity is least obvious, which is where this command is most wanted.
            let (address, name) = rest.split_once(' ').unwrap_or((rest, ""));
            is_address(address).then(|| BtDevice {
                address: address.to_lowercase(),
                name: name.trim().to_string(),
                ..Default::default()
            })
        })
        .collect();
    found.sort_by(|a, b| a.address.cmp(&b.address));
    found.dedup_by(|a, b| a.address == b.address);
    found
}

/// Fill in one device's detail from `bluetoothctl info <address>`.
pub(crate) fn describe(device: &mut BtDevice) {
    if let Some(text) = run(&["info", &device.address]) {
        parse_info(text.as_str(), device);
    }
}

/// Parse `bluetoothctl info` into an existing device. The body is `Key: value` lines, indented;
/// unknown keys are skipped, and a key BlueZ stops emitting simply leaves its field as it was.
pub(crate) fn parse_info(text: &str, device: &mut BtDevice) {
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            // `Alias` is the user-facing name and overrides `Name` when both appear, which is
            // what BlueZ itself displays — a renamed device should read as the user renamed it.
            "Name" if device.name.is_empty() => device.name = value.to_string(),
            "Alias" => device.name = value.to_string(),
            "Paired" => device.paired = value == "yes",
            "Connected" => device.connected = value == "yes",
            "Icon" => device.icon = Some(value.to_string()),
            "RSSI" => device.rssi = value.parse().ok(),
            _ => {}
        }
    }
}

/// Whether `text` looks like a Bluetooth address (`AA:BB:CC:DD:EE:FF`) — the guard that keeps
/// BlueZ's prose out of the device list.
fn is_address(text: &str) -> bool {
    let octets: Vec<&str> = text.split(':').collect();
    octets.len() == 6
        && octets.iter().all(|octet| {
            octet.len() == 2 && octet.chars().all(|character| character.is_ascii_hexdigit())
        })
}

/// The sysfs path for an adapter — exposed so a test can build a fake tree at a known shape.
#[cfg(test)]
fn adapter_path(root: &Path, name: &str) -> PathBuf {
    root.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bashrs_bt_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `bluetoothctl devices` as BlueZ 5 prints it, with the banner noise it mixes in.
    const DEVICES: &str = "\
Device AA:BB:CC:DD:EE:FF Pixel 8
Device 11:22:33:44:55:66 WH-1000XM4
Agent registered
Device 11:22:33:44:55:66 WH-1000XM4
[bluetooth]#
Device DE:AD:BE:EF:00:01
";

    #[test]
    fn the_device_list_keeps_devices_and_ignores_bluez_chatter() {
        let found = parse_devices(DEVICES);
        assert_eq!(found.len(), 3, "the agent banner and prompt are not devices: {found:?}");
        assert_eq!(found[0].address, "11:22:33:44:55:66", "sorted by address");
        assert_eq!(found[0].name, "WH-1000XM4");
        assert_eq!(found[1].address, "aa:bb:cc:dd:ee:ff", "addresses normalise to lowercase");
        assert_eq!(found[1].name, "Pixel 8", "a name with a space survives whole");
        // A device with no name at all is still a device — BlueZ prints the line regardless.
        assert_eq!(found[2].address, "de:ad:be:ef:00:01");
        assert_eq!(found[2].name, "");
        // The duplicate line collapses: BlueZ repeats a device once per controller that saw it.
        assert_eq!(found.iter().filter(|d| d.address == "11:22:33:44:55:66").count(), 1);
    }

    #[test]
    fn a_line_that_is_not_a_device_is_never_mistaken_for_one() {
        assert!(parse_devices("").is_empty());
        assert!(parse_devices("Agent registered\n[bluetooth]# \n").is_empty());
        // `Device` followed by something that isn't an address — BlueZ prose, not a device.
        assert!(parse_devices("Device not-an-address Foo\n").is_empty());
        assert!(parse_devices("Device AA:BB:CC:DD:EE Foo\n").is_empty(), "five octets is not one");
        assert!(parse_devices("Device ZZ:BB:CC:DD:EE:FF Foo\n").is_empty(), "non-hex is not one");
    }

    /// `bluetoothctl info` for a connected, paired headset.
    const INFO: &str = "\
Device 11:22:33:44:55:66 (public)
\tName: WH-1000XM4
\tAlias: Studio Headphones
\tClass: 0x00240404
\tIcon: audio-headset
\tPaired: yes
\tBonded: yes
\tTrusted: yes
\tBlocked: no
\tConnected: yes
\tLegacyPairing: no
\tRSSI: -47
\tUUID: Audio Sink                (0000110b-0000-1000-8000-00805f9b34fb)
";

    #[test]
    fn info_fills_in_the_detail_and_prefers_the_users_own_alias() {
        let mut device = BtDevice { address: "11:22:33:44:55:66".into(), ..Default::default() };
        parse_info(INFO, &mut device);
        assert_eq!(device.name, "Studio Headphones", "the alias is what BlueZ itself displays");
        assert!(device.paired && device.connected);
        assert_eq!(device.icon.as_deref(), Some("audio-headset"));
        assert_eq!(device.rssi, Some(-47), "a negative dBm parses");
        assert_eq!(device.kind(), "headphones", "the icon name is translated for a person");
    }

    #[test]
    fn info_reads_the_negative_answers_as_negative_rather_than_missing() {
        let text = "Device X\n\tPaired: no\n\tConnected: no\n\tIcon: phone\n";
        let mut device = BtDevice { paired: true, connected: true, ..Default::default() };
        parse_info(text, &mut device);
        assert!(!device.paired, "`no` must clear a previously-true field, not leave it");
        assert!(!device.connected);
        assert_eq!(device.kind(), "phone");
        // An out-of-range device reports no RSSI at all; the field stays empty rather than 0.
        assert_eq!(device.rssi, None);
    }

    #[test]
    fn info_that_is_empty_or_unfamiliar_leaves_the_device_as_it_was() {
        let mut device = BtDevice {
            address: "aa:bb:cc:dd:ee:ff".into(),
            name: "Known".into(),
            paired: true,
            ..Default::default()
        };
        let before = device.clone();
        parse_info("", &mut device);
        parse_info("Some unrelated banner\nNoColonHere\n", &mut device);
        assert_eq!(device, before, "nothing recognised means nothing changed");
    }

    /// Every icon BlueZ commonly emits should read as a word, and an unknown one should pass
    /// through rather than being flattened to nothing — a new icon name is still information.
    #[test]
    fn device_kinds_translate_known_icons_and_pass_unknown_ones_through() {
        let kind = |icon: Option<&str>| {
            BtDevice { icon: icon.map(str::to_string), ..Default::default() }.kind().to_string()
        };
        assert_eq!(kind(Some("audio-headset")), "headphones");
        assert_eq!(kind(Some("input-gaming")), "controller");
        assert_eq!(kind(Some("computer")), "computer");
        assert_eq!(kind(Some("some-future-icon")), "some-future-icon", "unknown, but still shown");
        assert_eq!(kind(None), "—", "no icon is no answer, not a guess");
    }

    /// The radio state that makes "no devices" a lie — checked against a fabricated rfkill tree,
    /// since a real one needs hardware this machine hasn't got.
    #[test]
    fn a_bluetooth_kill_switch_is_read_and_the_hardest_block_wins() {
        let root = scratch("rfkill");
        let switch = |name: &str, kind: &str, soft: &str, hard: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("type"), format!("{kind}\n")).unwrap();
            std::fs::write(dir.join("soft"), format!("{soft}\n")).unwrap();
            std::fs::write(dir.join("hard"), format!("{hard}\n")).unwrap();
            dir
        };
        assert_eq!(read_switch(&switch("rfkill0", "bluetooth", "0", "0")), Some(Radio::Live));
        assert_eq!(read_switch(&switch("rfkill1", "bluetooth", "1", "0")), Some(Radio::SoftBlocked));
        assert_eq!(read_switch(&switch("rfkill2", "bluetooth", "0", "1")), Some(Radio::HardBlocked));
        // A hardware block outranks a soft one — telling someone to run `rfkill unblock` when a
        // physical switch is off sends them after a fix that cannot work.
        assert_eq!(read_switch(&switch("rfkill3", "bluetooth", "1", "1")), Some(Radio::HardBlocked));
        // Wi-Fi's switch is not Bluetooth's, however blocked it is.
        assert_eq!(read_switch(&switch("rfkill4", "wlan", "1", "1")), None);
        // A dangling entry (the container case) reads as nothing rather than panicking.
        assert_eq!(read_switch(&root.join("nonexistent")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `bluetoothctl show` distinguishes three states that otherwise look identical from outside:
    /// no controller (service down), a controller that is off, and one that is ready.
    #[test]
    fn show_tells_a_stopped_service_from_a_powered_down_adapter() {
        let ready = "Controller AA:BB:CC:DD:EE:FF (public)\n\tName: box\n\tPowered: yes\n\tPairable: yes\n";
        assert_eq!(parse_show(ready), Some(true));

        let off = "Controller AA:BB:CC:DD:EE:FF (public)\n\tName: box\n\tPowered: no\n";
        assert_eq!(parse_show(off), Some(false));

        // BlueZ installed, service not running: it names no controller at all.
        assert_eq!(parse_show(""), None);
        assert_eq!(parse_show("No default controller available\n"), None);
        // A controller with no Powered line at all is not assumed to be on.
        assert_eq!(parse_show("Controller AA:BB:CC:DD:EE:FF (public)\n\tName: box\n"), Some(false));
    }

    #[test]
    fn adapters_are_read_from_sysfs_and_connection_children_are_skipped() {
        let root = scratch("adapters");
        // A controller, with its address.
        let hci0 = adapter_path(&root, "hci0");
        std::fs::create_dir_all(&hci0).unwrap();
        std::fs::write(hci0.join("address"), "AA:BB:CC:DD:EE:FF\n").unwrap();
        assert_eq!(
            parse_adapter(&hci0),
            Some(Adapter { name: "hci0".into(), address: Some("aa:bb:cc:dd:ee:ff".into()) })
        );

        // A per-connection child (`hci0:42`) sits in the same directory and is not an adapter.
        let child = adapter_path(&root, "hci0:42");
        std::fs::create_dir_all(&child).unwrap();
        assert_eq!(parse_adapter(&child), None);

        // A controller whose address the kernel doesn't expose is still a controller.
        let hci1 = adapter_path(&root, "hci1");
        std::fs::create_dir_all(&hci1).unwrap();
        assert_eq!(parse_adapter(&hci1), Some(Adapter { name: "hci1".into(), address: None }));

        // Anything not named `hci*` is something else entirely.
        let other = adapter_path(&root, "rfkill0");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(parse_adapter(&other), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// On a machine with no Bluetooth at all — like the one this was written on — every entry
    /// point must answer emptily rather than fail.
    #[test]
    fn a_machine_without_bluetooth_reports_nothing_rather_than_erroring() {
        // `adapters()` reads a directory that may not exist; that is the normal case here.
        let _ = adapters();
        assert!(parse_devices("").is_empty());
    }
}
