//! Binary-format decoders: pull searchable text out of files `gg` would otherwise skip as binary,
//! by understanding their structure rather than scanning raw bytes. A child of [`super`]
//! (treegrep), whose `scan_file` searches what they decode; `pub(crate)` because the AI-fingerprint
//! scan (`detect_ai_textual_fingerprint`) reads files through [`extract`] too. One decoder per
//! format; [`extract`] dispatches on the [`Format`] a file's extension names, so adding a format is
//! a `Format` row, a decoder and a dispatch arm. Files with no decoder return `None` and stay
//! skipped (no raw `strings`-style fallback — that's the noisy, slow approach `gg` deliberately
//! avoids).
//!
//! Documents are read by every search, since their text is what a search is for; media only
//! under `gg --delve` (developer's call, 2026-11-05):
//!
//! - `.pdf` — the text of each page, labelled `page N` ([`pdf`], its own child module).
//! - `.torrent` (`--delve`) — bencode: the human-readable string values (name, file paths,
//!   tracker URLs), skipping the binary `pieces` SHA-1 blob.
//! - `.mkv`/`.mka`/`.mks`/`.webm` (`--delve`) — Matroska: the text subtitle tracks (`S_TEXT/*`,
//!   and WebM's WebVTT, `D_WEBVTT/*`). Extracted by walking the EBML tree and *seeking past*
//!   video/audio payloads — so a multi-GB file is read in milliseconds (only the tiny subtitle
//!   blocks are actually read), not in full.
//! - `.mp4`/`.m4v`/`.mov` (`--delve`) — ISO-BMFF: text subtitle tracks (tx3g/QuickTime text,
//!   WebVTT, TTML), located via the `moov` sample tables (progressive) or `moof`/`traf`/`trun`
//!   fragments (fragmented) and read from `mdat`.
//!
//! A video's subtitle lines are placed as its player lists them: by their track — its title, else
//! its language — and the second their cue starts (`English (SDH) 0:01:23`). A file whose text
//! can't be read comes back saying why ([`Unread`]: encrypted, a scan, picture subtitles, too big,
//! damaged), for the search to name it rather than pass over it in silence.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::Path;

// The PDF reader is large enough to be a file of its own.
mod pdf;

/// A format [`extract`] decodes, told by the file's extension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Format {
    Pdf,
    Torrent,
    /// `.mkv`/`.mka`/`.mks`/`.webm`.
    Matroska,
    /// `.mp4`/`.m4v`/`.mov`.
    Mp4,
}

impl Format {
    /// The format of `path`, if it is one decoded here.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        Some(match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "pdf" => Self::Pdf,
            "torrent" => Self::Torrent,
            "mkv" | "mka" | "mks" | "webm" => Self::Matroska,
            "mp4" | "m4v" | "mov" => Self::Mp4,
            _ => return None,
        })
    }

    /// A document — its text is what a search is for, so every search reads it; media only under
    /// `--delve`.
    fn is_document(self) -> bool {
        self == Self::Pdf
    }

    /// Decoded from the whole file, held in memory — up to [`MAX_WHOLE`]. The media formats are
    /// read piecemeal instead: only their subtitle text, however big the file.
    pub(crate) fn is_read_whole(self) -> bool {
        matches!(self, Self::Pdf | Self::Torrent)
    }
}

/// What [`extract`] made of a file it could read.
pub(crate) enum Decoded {
    /// Its text, in the parts it comes in.
    Text(Vec<Section>),
    /// Text it couldn't get at, and why.
    Unread(Unread),
}

impl From<Result<Vec<Section>, Unread>> for Decoded {
    fn from(decoded: Result<Vec<Section>, Unread>) -> Self {
        decoded.map_or_else(Self::Unread, Self::Text)
    }
}

/// Why a decodable file's text couldn't be read — each a reason to name the file, never to pass
/// over it in silence. A file left alone by choice (a binary one, or media without `--delve`) has
/// none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Unread {
    /// An encrypted PDF: reading it takes decrypting it, which is left to the user — even one
    /// encrypted only to restrict printing, whose password is empty.
    Encrypted,
    /// A PDF with no text at all: a scan, most likely, its words pictures.
    NoText,
    /// A video whose subtitles are all pictures (PGS, VobSub).
    PictureSubtitles,
    /// Bigger than [`MAX_WHOLE`], so not decoded.
    TooLarge,
    /// Not readable as the format its name says: damaged, cut short, or misnamed.
    Damaged,
}

impl Unread {
    /// Why, in a few words — what a search names the file with.
    pub(crate) fn reason(self) -> String {
        match self {
            Self::Encrypted => "encrypted".to_owned(),
            Self::NoText => "no text — a scan?".to_owned(),
            Self::PictureSubtitles => "subtitles are pictures".to_owned(),
            Self::TooLarge => format!("over {} MiB — too big to decode", MAX_WHOLE >> 20),
            Self::Damaged => "damaged, cut short, or not what its name says".to_owned(),
        }
    }
}

/// Text decoded from one file, in the parts it comes in: a PDF's pages, a video's subtitle tracks,
/// or a single part for a format without either.
pub(crate) struct Section {
    /// Where in the file the text is, shown where a line number would be.
    pub(crate) place: Place,
    /// The text, its lines joined by newlines.
    pub(crate) text: Vec<u8>,
}

/// Where a section's lines sit in the file they were decoded from.
pub(crate) enum Place {
    /// Nowhere worth naming: a torrent's names.
    Nowhere,
    /// One place for every line: a PDF's `page 3`.
    Section(String),
    /// A place per line of the section's text, in order: a subtitle track's `eng 0:01:23`.
    Lines(Vec<String>),
}

/// The largest file decoded whole in memory (a PDF, a torrent): bigger ones — scans, mostly, whose
/// pages are pictures with no text to find — are named as too big rather than held whole.
const MAX_WHOLE: u64 = 256 << 20;

/// Extract searchable UTF-8 text from `path` if its [`Format`] has a decoder: a document always,
/// media only when `delving`. Returns `None` when the extension isn't one decoded here (or is media
/// and this isn't a delve) — so callers keep skipping such files rather than raw-scanning them. A
/// decodable file comes back as its text, or as why that couldn't be read ([`Unread`]) — never as
/// a fall-through to a raw scan of (potentially huge) binary; an error reading the file itself is
/// the caller's to report.
pub(crate) fn extract(path: &Path, delving: bool) -> Option<io::Result<Decoded>> {
    let format = Format::of(path)?;
    (delving || format.is_document()).then(|| decode(path, format))
}

/// Decode `path` as `format`.
fn decode(path: &Path, format: Format) -> io::Result<Decoded> {
    if format.is_read_whole() && std::fs::metadata(path)?.len() > MAX_WHOLE {
        return Ok(Decoded::Unread(Unread::TooLarge));
    }
    Ok(match format {
        Format::Pdf => pdf_pages(&std::fs::read(path)?).into(),
        Format::Torrent => torrent_names(&std::fs::read(path)?).into(),
        Format::Matroska => video_text(matroska_subtitles(io::BufReader::with_capacity(128 * 1024, File::open(path)?)))?,
        Format::Mp4 => video_text(mp4_subtitles(File::open(path)?))?,
    })
}

/// A video's subtitles as decoded text — or as damaged, where its decoder met bytes that aren't the
/// format (or that end) before its track list. An error reading the bytes stays an error.
fn video_text(tracks: io::Result<Vec<SubtitleTrack>>) -> io::Result<Decoded> {
    match tracks {
        Ok(tracks) => Ok(subtitle_sections(&tracks).into()),
        Err(err) if is_damage(&err) => Ok(Decoded::Unread(Unread::Damaged)),
        Err(err) => Err(err),
    }
}

/// A PDF's pages, each placed `page N` by its place in the document; pages with no text are left
/// out, their numbers kept by the rest. A PDF with pages but no text on any is
/// [`Unread::NoText`]; one with no pages to find, [`Unread::Damaged`].
fn pdf_pages(data: &[u8]) -> Result<Vec<Section>, Unread> {
    let pages = pdf::pages(data).map_err(|pdf::Encrypted| Unread::Encrypted)?;
    if pages.is_empty() {
        return Err(Unread::Damaged);
    }
    let sections: Vec<Section> = (1..)
        .zip(pages)
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(number, text)| Section { place: Place::Section(format!("page {number}")), text: text.into_bytes() })
        .collect();
    if sections.is_empty() {
        return Err(Unread::NoText);
    }
    Ok(sections)
}

/// A torrent's names — its files', its trackers' — as one section with no place to name;
/// [`Unread::Damaged`] when its bencode breaks off.
fn torrent_names(data: &[u8]) -> Result<Vec<Section>, Unread> {
    let text = bencode_text(data).ok_or(Unread::Damaged)?;
    Ok(vec![Section { place: Place::Nowhere, text }])
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

// ---- bencode (.torrent) ---------------------------------------------------------------------

/// Walk the bencode value that is a torrent, collecting every UTF-8 byte-string as a line.
/// Non-UTF-8 byte-strings (the `pieces` hash blob) and integers are skipped; anything after the
/// value is ignored. `None` when the value breaks off — malformed or cut short.
fn bencode_text(data: &[u8]) -> Option<Vec<u8>> {
    let (mut out, mut pos) = (Vec::new(), 0);
    walk_bencode(data, &mut pos, &mut out)?;
    Some(out)
}

/// Parse one bencode value at `*pos`, advancing `*pos` past it and pushing any UTF-8 strings found.
/// Returns `None` on malformed input (parsing stops).
fn walk_bencode(data: &[u8], pos: &mut usize, out: &mut Vec<u8>) -> Option<()> {
    match data.get(*pos)? {
        b'i' => {
            // i<digits>e
            *pos += 1;
            while *data.get(*pos)? != b'e' {
                *pos += 1;
            }
            *pos += 1;
        }
        b'l' | b'd' => {
            // list/dict: values (dict = alternating key/value) until 'e'
            *pos += 1;
            while *data.get(*pos)? != b'e' {
                walk_bencode(data, pos, out)?;
            }
            *pos += 1;
        }
        b'0'..=b'9' => {
            // <len>:<bytes>
            let mut len = 0usize;
            while let Some(&c) = data.get(*pos) {
                if c == b':' {
                    break;
                }
                len = len.checked_mul(10)?.checked_add(c.checked_sub(b'0').filter(|d| *d < 10)? as usize)?;
                *pos += 1;
            }
            *pos += 1; // skip ':'
            let bytes = data.get(*pos..pos.checked_add(len)?)?;
            *pos += len;
            if let Ok(text) = std::str::from_utf8(bytes) {
                if !text.is_empty() && !text.chars().any(|c| c.is_control()) {
                    out.extend_from_slice(text.as_bytes());
                    out.push(b'\n');
                }
            }
        }
        _ => return None,
    }
    Some(())
}

// ---- subtitles, Matroska and MP4 alike ------------------------------------------------------

/// A video's subtitle track, as a player lists it.
struct SubtitleTrack {
    /// What a player calls it: its title, else its language.
    name: String,
    /// Its cues — `None` for picture subtitles (PGS, VobSub), which hold no text.
    cues: Option<Vec<Cue>>,
}

/// A subtitle cue: when it starts, in whole seconds, and its text.
struct Cue {
    start: u64,
    text: String,
}

/// One section per text track with any cue, in the video's track order: its cues' lines, blank
/// ones left out, each placed by the track's name and its cue's start — `English 0:01:23`. Where
/// two text tracks share a name, each is numbered by its place among the video's subtitle tracks,
/// as a player's menu counts them (`English #2`). A video whose only subtitles are pictures is
/// [`Unread::PictureSubtitles`]; one with none has nothing to read.
fn subtitle_sections(tracks: &[SubtitleTrack]) -> Result<Vec<Section>, Unread> {
    if !tracks.is_empty() && tracks.iter().all(|track| track.cues.is_none()) {
        return Err(Unread::PictureSubtitles);
    }
    let shared = |name: &str| tracks.iter().filter(|track| track.cues.is_some() && track.name == name).count() > 1;
    Ok(tracks
        .iter()
        .enumerate()
        .filter_map(|(index, track)| {
            let cues = track.cues.as_ref()?;
            let name = if shared(&track.name) { format!("{} #{}", track.name, index + 1) } else { track.name.clone() };
            let (mut text, mut places) = (Vec::new(), Vec::new());
            for cue in cues {
                let place = format!("{name} {}", clock(cue.start));
                for line in cue.text.lines().filter(|line| !line.trim().is_empty()) {
                    text.extend_from_slice(line.as_bytes());
                    text.push(b'\n');
                    places.push(place.clone());
                }
            }
            (!places.is_empty()).then_some(Section { place: Place::Lines(places), text })
        })
        .collect())
}

/// `seconds` as the clock time a player seeks to, `H:MM:SS`.
fn clock(seconds: u64) -> String {
    format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)
}

/// Whether a decoder's error says the bytes aren't the format, or end too soon — damage — rather
/// than that the system couldn't read them.
fn is_damage(err: &io::Error) -> bool {
    matches!(err.kind(), io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof)
}

/// A cue's bytes as text — and, where they end partway into a character (a cue cut short with its
/// file), the text before that character. `None` for bytes that aren't UTF-8.
fn utf8_text(bytes: &[u8]) -> Option<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(err) if err.error_len().is_none() => std::str::from_utf8(&bytes[..err.valid_up_to()]).ok(),
        Err(_) => None,
    }
}

/// Read up to `buf.len()` bytes (fewer only at EOF); returns the count read.
fn fill<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// A text field — a track's title, its language — as its container stores it: UTF-8, maybe padded
/// with NULs. `None` when empty.
fn text_field(data: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(data).ok()?.trim_end_matches('\0');
    (!text.is_empty()).then(|| text.to_owned())
}

/// The largest subtitle sample (a Matroska block, an MP4 sample) read to decode. A cue's text is a
/// few hundred bytes; a size past this is corrupt, or not a subtitle.
const MAX_SUBTITLE_SAMPLE: u64 = 256 * 1024;

// ---- Matroska/EBML (.mkv/.mka/.webm) --------------------------------------------------------

const ID_SEGMENT: u64 = 0x1853_8067;
const ID_SEEK_HEAD: u64 = 0x114D_9B74;
const ID_INFO: u64 = 0x1549_A966;
const ID_TIMESTAMP_SCALE: u64 = 0x2A_D7B1;
const ID_TRACKS: u64 = 0x1654_AE6B;
const ID_CUES: u64 = 0x1C53_BB6B;
const ID_CHAPTERS: u64 = 0x1043_A770;
const ID_TAGS: u64 = 0x1254_C367;
const ID_ATTACHMENTS: u64 = 0x1941_A469;
const ID_CLUSTER: u64 = 0x1F43_B675;
const ID_CLUSTER_TIMESTAMP: u64 = 0xE7;
const ID_TRACK_ENTRY: u64 = 0xAE;
const ID_TRACK_NUMBER: u64 = 0xD7;
const ID_TRACK_TYPE: u64 = 0x83;
const ID_CODEC_ID: u64 = 0x86;
const ID_NAME: u64 = 0x536E;
const ID_LANGUAGE: u64 = 0x22_B59C;
const ID_LANGUAGE_BCP47: u64 = 0x22_B59D;
const ID_SIMPLE_BLOCK: u64 = 0xA3;
const ID_BLOCK_GROUP: u64 = 0xA0;
const ID_BLOCK: u64 = 0xA1;
const TRACK_TYPE_SUBTITLE: u64 = 0x11;

/// Nanoseconds per tick of a Segment's timestamps when its `Info` names no `TimestampScale` — the
/// Matroska spec's default, a millisecond.
const DEFAULT_TIMESTAMP_SCALE: u64 = 1_000_000;

/// The largest `Info` or `Tracks` element read into memory. Real ones hold a few KB of track
/// descriptions; past this, a size is corrupt, and believing it would allocate whatever it claims.
const MAX_HEADER: u64 = 16 << 20;

/// The Segment-level elements. Used to detect the end of an *unknown-size* Cluster: seeing one of
/// these means the Cluster is over and this is its sibling.
fn is_segment_level(id: u64) -> bool {
    matches!(
        id,
        ID_SEEK_HEAD | ID_INFO | ID_TRACKS | ID_CUES | ID_CHAPTERS | ID_TAGS | ID_ATTACHMENTS | ID_CLUSTER
    )
}

/// Read one EBML variable-length integer. Returns `(raw, value, len)`: `raw` keeps the length-marker
/// bit (that's the element ID), `value` clears it (that's a size/uint), `len` is the byte count.
/// `Ok(None)` at clean EOF.
fn read_vint<R: Read>(r: &mut R) -> io::Result<Option<(u64, u64, u8)>> {
    let mut b = [0u8; 1];
    match r.read(&mut b) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(e) => return Err(e),
    }
    let first = b[0];
    if first == 0 {
        return Err(bad("EBML vint longer than 8 bytes"));
    }
    let len = first.leading_zeros() as u8 + 1; // 1..=8
    let marker = 0x80u8 >> (len - 1);
    let (mut raw, mut value) = (u64::from(first), u64::from(first & !marker));
    for _ in 1..len {
        r.read_exact(&mut b)?;
        raw = (raw << 8) | u64::from(b[0]);
        value = (value << 8) | u64::from(b[0]);
    }
    Ok(Some((raw, value, len)))
}

/// Read an element header: its ID and its size (`None` = the EBML "unknown size" marker). `Ok(None)`
/// at clean EOF (no more elements).
fn read_element<R: Read>(r: &mut R) -> io::Result<Option<(u64, Option<u64>)>> {
    let Some((id, _, _)) = read_vint(r)? else { return Ok(None) };
    let Some((_, size, len)) = read_vint(r)? else { return Err(bad("truncated element size")) };
    let unknown = size == (1u64 << (7 * u32::from(len))) - 1;
    Ok(Some((id, (!unknown).then_some(size))))
}

/// An EBML unsigned integer stored big-endian in `data`.
fn read_uint(data: &[u8]) -> u64 {
    data.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
}

/// Extract the subtitle tracks of a Matroska/WebM stream, as a player lists them. Walks the EBML
/// tree reading only subtitle blocks — small payloads streamed through, large ones seeked past (see
/// [`skip`]) — so cost tracks the subtitle track's size, not the file's. Assumes the standard layout
/// where `Tracks` precedes the `Cluster`s. Give it a *buffered* reader: the traversal does many
/// small reads.
fn matroska_subtitles<R: BufRead + Seek>(mut r: R) -> io::Result<Vec<SubtitleTrack>> {
    let mut read = MatroskaRead { scale: DEFAULT_TIMESTAMP_SCALE, listed: false, tracks: Vec::new(), frames: Vec::new() };
    match read_segment(&mut r, &mut read) {
        // Once its track list is read, a video is read as it is: cut short or broken further on,
        // the cues before the break are searched like any others.
        Err(err) if !(read.listed && is_damage(&err)) => Err(err),
        _ => Ok(read.into_tracks()),
    }
}

/// What a walk through a Matroska Segment has found so far.
struct MatroskaRead {
    /// Nanoseconds per tick of its timestamps (`TimestampScale`).
    scale: u64,
    /// Whether its track list (`Tracks`) has been read.
    listed: bool,
    /// Its subtitle tracks, in file order.
    tracks: Vec<MatroskaTrack>,
    /// The subtitle frames read, in file order.
    frames: Vec<Frame>,
}

impl MatroskaRead {
    /// The subtitle tracks with their cues, each frame's start in seconds.
    fn into_tracks(self) -> Vec<SubtitleTrack> {
        let mut cues: Vec<Vec<Cue>> = self.tracks.iter().map(|_| Vec::new()).collect();
        for frame in self.frames {
            let nanoseconds = u128::try_from(frame.start).unwrap_or(0) * u128::from(self.scale);
            let start = u64::try_from(nanoseconds / 1_000_000_000).unwrap_or(u64::MAX);
            if let Some(cues) = cues.get_mut(frame.track) {
                cues.push(Cue { start, text: frame.text });
            }
        }
        self.tracks
            .into_iter()
            .zip(cues)
            .map(|(track, cues)| SubtitleTrack { name: track.title.unwrap_or(track.language), cues: track.codec.map(|_| cues) })
            .collect()
    }
}

/// Walk a Matroska stream's Segment into `read`: its timestamp scale, its subtitle tracks, and their
/// frames.
fn read_segment<R: BufRead + Seek>(r: &mut R, read: &mut MatroskaRead) -> io::Result<()> {
    // Top level: find the Segment, skipping the EBML header (and anything else).
    let seg_size = loop {
        match read_element(r)? {
            Some((ID_SEGMENT, size)) => break size,
            Some((_, Some(size))) => skip(r, size)?,
            // The file ends, or an unknown-size element comes, before any Segment: not Matroska.
            None | Some((_, None)) => return Err(bad("no Matroska Segment")),
        }
    };
    let seg_end = seg_size.map_or(u64::MAX, |s| r.stream_position().unwrap_or(0) + s);

    while r.stream_position()? < seg_end {
        let Some((id, size)) = read_element(r)? else { break };
        match id {
            ID_INFO | ID_TRACKS => {
                let Some(size) = size.filter(|&size| size <= MAX_HEADER) else { return Err(bad("an implausible header size")) };
                let mut buf = vec![0u8; size as usize];
                r.read_exact(&mut buf)?;
                if id == ID_INFO {
                    read.scale = timestamp_scale(&buf);
                    continue;
                }
                read.tracks = subtitle_tracks(&buf);
                read.listed = true;
                if read.tracks.iter().all(|track| track.codec.is_none()) {
                    break; // no text subtitle track → nothing more to read
                }
            }
            ID_CLUSTER => read_container(r, size, &read.tracks, 0, &mut read.frames)?,
            _ => match size {
                Some(s) => skip(r, s)?,
                None => break,
            },
        }
    }
    Ok(())
}

/// A subtitle track, as its `TrackEntry` declares it.
struct MatroskaTrack {
    number: u64,
    /// How its frames hold their text — `None` for picture subtitles (PGS, VobSub), which hold none.
    codec: Option<TextCodec>,
    /// Its `Name`: the title a player's menu shows.
    title: Option<String>,
    /// `LanguageBCP47` where given (`en`, `pt-BR`), else `Language` (`eng` — the spec's default
    /// when an entry names neither).
    language: String,
}

/// One subtitle frame's text: the track it belongs to (an index into the tracks) and when it
/// starts, in the Segment's ticks.
struct Frame {
    track: usize,
    start: i64,
    text: String,
}

/// The `TimestampScale` in an `Info` element's body: nanoseconds per tick of every timestamp in the
/// Segment.
fn timestamp_scale(info: &[u8]) -> u64 {
    children(info)
        .find(|&(id, _)| id == ID_TIMESTAMP_SCALE)
        .map(|(_, data)| read_uint(data))
        .filter(|&scale| scale > 0)
        .unwrap_or(DEFAULT_TIMESTAMP_SCALE)
}

/// The elements in a parent element's `body`, each one's ID and data, in order — stopping at a
/// header that can't be read, and cutting a size that runs past the body at its end.
fn children(body: &[u8]) -> impl Iterator<Item = (u64, &[u8])> {
    let mut cursor = io::Cursor::new(body);
    std::iter::from_fn(move || {
        let (id, size) = read_element(&mut cursor).ok()??;
        let start = cursor.position() as usize;
        let end = start.saturating_add(size.unwrap_or(0) as usize).min(body.len());
        cursor.set_position(end as u64);
        Some((id, body.get(start..end)?))
    })
}

/// How a Matroska text subtitle track lays its text out in a block's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TextCodec {
    /// `S_TEXT/*` (SRT's `UTF8`, ASS/SSA, `WEBVTT`): the frame is the text.
    SText,
    /// `D_WEBVTT/*`, WebM's WebVTT — which ffmpeg writes into `.mkv` too, so it is what YouTube
    /// subtitles embedded by yt-dlp arrive as. The frame starts with the cue's identifier line and
    /// settings line (`align:start position:0%`), both always present even when empty; the cue
    /// text follows. Its `SUBTITLES`/`CAPTIONS` kinds are subtitle-type tracks; `DESCRIPTIONS`
    /// and `METADATA` are metadata-type (0x21), so the subtitle check leaves them out.
    WebmVtt,
}

impl TextCodec {
    /// The family of a track's `CodecID`, if it is a text one.
    fn of(codec_id: &str) -> Option<Self> {
        if codec_id.starts_with("S_TEXT") {
            Some(Self::SText)
        } else if codec_id.starts_with("D_WEBVTT/") {
            Some(Self::WebmVtt)
        } else {
            None
        }
    }

    /// The text a block's frame carries.
    fn text(self, frame: &[u8]) -> &[u8] {
        match self {
            Self::SText => frame,
            Self::WebmVtt => after_two_lines(frame),
        }
    }
}

/// What follows `frame`'s first two lines (a WebVTT line ends at CRLF, LF or CR) — nothing when
/// it has fewer, rather than a cue's settings passed off as its text.
fn after_two_lines(mut frame: &[u8]) -> &[u8] {
    for _ in 0..2 {
        let Some(end) = frame.iter().position(|&byte| byte == b'\n' || byte == b'\r') else { return &[] };
        let crlf = frame[end] == b'\r' && frame.get(end + 1) == Some(&b'\n');
        frame = &frame[end + 1 + usize::from(crlf)..];
    }
    frame
}

/// The subtitle tracks (`TrackType` = subtitle) in a `Tracks` element's body, in file order — so a
/// track's position is the number a player's menu gives it. Picture subtitles (PGS/VobSub) are
/// among them, with no [`TextCodec`]: they carry no text, but they count.
fn subtitle_tracks(tracks: &[u8]) -> Vec<MatroskaTrack> {
    children(tracks).filter(|&(id, _)| id == ID_TRACK_ENTRY).filter_map(|(_, entry)| subtitle_track(entry)).collect()
}

/// A `TrackEntry`, if it's a subtitle track.
fn subtitle_track(entry: &[u8]) -> Option<MatroskaTrack> {
    let (mut number, mut is_subtitle, mut codec) = (None, false, None);
    let (mut title, mut language, mut bcp47) = (None, None, None);
    for (id, data) in children(entry) {
        match id {
            ID_TRACK_NUMBER => number = Some(read_uint(data)),
            ID_TRACK_TYPE => is_subtitle = read_uint(data) == TRACK_TYPE_SUBTITLE,
            ID_CODEC_ID => codec = std::str::from_utf8(data).ok().and_then(TextCodec::of),
            ID_NAME => title = text_field(data),
            ID_LANGUAGE => language = text_field(data),
            ID_LANGUAGE_BCP47 => bcp47 = text_field(data),
            _ => {}
        }
    }
    let language = bcp47.or(language).unwrap_or_else(|| "eng".to_owned());
    is_subtitle.then_some(MatroskaTrack { number: number?, codec, title, language })
}

/// Payloads at or below this read straight through (cheap and sequential — kind to spinning disks);
/// larger ones are seeked past (avoids reading a big video frame). This trades seek count against
/// bytes read; there's no single optimum across SSD and HDD, so it's a deliberate middle ground.
const SKIP_THRESHOLD: u64 = 64 * 1024;

/// Advance past `size` payload bytes: stream straight through if small, else seek past.
fn skip<R: BufRead + Seek>(r: &mut R, size: u64) -> io::Result<()> {
    if size <= SKIP_THRESHOLD {
        io::copy(&mut (&mut *r).take(size), &mut io::sink())?;
    } else {
        r.seek(SeekFrom::Current(size as i64))?;
    }
    Ok(())
}

/// Walk the children of a Cluster (or BlockGroup), collecting subtitle blocks as [`Frame`]s and
/// advancing past everything else via [`skip`]. `time` is the Cluster's timestamp, which its blocks
/// count from — its first child, so read before them. Buffered reads keep the per-block
/// header/track parsing cheap. `size` `None` = an unknown-size Cluster: stop when a Segment-level
/// sibling appears (rewind so the caller re-reads it) or at EOF.
fn read_container<R: BufRead + Seek>(
    r: &mut R,
    size: Option<u64>,
    tracks: &[MatroskaTrack],
    mut time: i64,
    frames: &mut Vec<Frame>,
) -> io::Result<()> {
    let end = size.map(|s| r.stream_position().unwrap_or(0) + s);
    loop {
        let before = r.stream_position()?;
        if end.is_some_and(|e| before >= e) {
            break;
        }
        let Some((id, esize)) = read_element(r)? else { break };
        match id {
            ID_CLUSTER_TIMESTAMP => {
                let Some(size) = esize.filter(|&size| size <= 8) else { break };
                let mut buf = [0u8; 8];
                r.read_exact(&mut buf[..size as usize])?;
                time = i64::try_from(read_uint(&buf[..size as usize])).unwrap_or(i64::MAX);
            }
            ID_SIMPLE_BLOCK | ID_BLOCK => {
                let Some(block_size) = esize else { break };
                let Some((_, number, number_len)) = read_vint(r)? else { break };
                let rest = block_size.saturating_sub(u64::from(number_len));
                let text_track = tracks
                    .iter()
                    .position(|track| track.number == number)
                    .and_then(|index| Some((index, tracks[index].codec?)));
                match text_track {
                    Some((index, codec)) => {
                        if let Some((offset, text)) = read_subtitle_frame(r, rest, codec)? {
                            frames.push(Frame { track: index, start: time.saturating_add(i64::from(offset)), text });
                        }
                    }
                    None => skip(r, rest)?,
                }
            }
            ID_BLOCK_GROUP => read_container(r, esize, tracks, time, frames)?,
            _ if end.is_none() && is_segment_level(id) => {
                r.seek(SeekFrom::Start(before))?; // Cluster ended; the caller re-reads this sibling
                return Ok(());
            }
            _ => match esize {
                Some(s) => skip(r, s)?,
                None => break,
            },
        }
    }
    Ok(())
}

/// Read a subtitle block's remaining bytes (`[int16 ts][flags][frame]`, the track vint already
/// consumed): its timestamp, counted from its Cluster's, and the text its `codec` puts in the
/// frame. Only called for subtitle blocks, so reading them in full is fine — up to
/// [`MAX_SUBTITLE_SAMPLE`], past which the block is skipped. A block the file ends inside gives the
/// text as far as it goes.
fn read_subtitle_frame<R: BufRead + Seek>(r: &mut R, rest: u64, codec: TextCodec) -> io::Result<Option<(i16, String)>> {
    if rest > MAX_SUBTITLE_SAMPLE {
        skip(r, rest)?;
        return Ok(None);
    }
    let mut buf = vec![0u8; rest as usize];
    let read = fill(r, &mut buf)?;
    buf.truncate(read);
    // lacing bits (flags & 0x06) == 0 means a single plain frame; buf[3..] is the frame.
    if buf.len() > 3 && buf[2] & 0x06 == 0 {
        if let Some(text) = utf8_text(codec.text(&buf[3..])) {
            return Ok(Some((i16::from_be_bytes([buf[0], buf[1]]), text.to_owned())));
        }
    }
    Ok(None)
}

// ---- MP4 / ISO-BMFF (.mp4/.m4v/.mov) --------------------------------------------------------

/// The `N` bytes at `pos`, or `None` when the stream ends first — the shared seek-and-read-exactly
/// opening of the box parsers below. Leaves the reader positioned just past the bytes.
fn read_at<const N: usize, R: Read + Seek>(r: &mut R, pos: u64) -> io::Result<Option<[u8; N]>> {
    r.seek(SeekFrom::Start(pos))?;
    let mut buf = [0u8; N];
    Ok((fill(r, &mut buf)? == N).then_some(buf))
}

/// A box header: `(fourcc, content_start, content_end, next_box_pos)`.
type BoxHeader = ([u8; 4], u64, u64, u64);

/// The box at `pos` within a container ending at `end`, or `None` when there's no room for another
/// box. Handles 32-bit sizes, 64-bit (`size == 1`), and extends-to-end (`size == 0`).
fn next_box<R: Read + Seek>(r: &mut R, pos: u64, end: u64) -> io::Result<Option<BoxHeader>> {
    if pos + 8 > end {
        return Ok(None);
    }
    let Some(hdr) = read_at::<8, _>(r, pos)? else { return Ok(None) };
    let fourcc = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let (total, header) = match u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) {
        1 => {
            let mut ext = [0u8; 8];
            r.read_exact(&mut ext)?;
            (u64::from_be_bytes(ext), 16)
        }
        0 => (end - pos, 8),
        s => (u64::from(s), 8),
    };
    if total < header {
        return Ok(None);
    }
    let content_start = pos + header;
    Ok(Some((fourcc, content_start, (pos + total).min(end), pos + total)))
}

/// The content range of the first child box of type `want` within `[start, end)`.
fn find_box<R: Read + Seek>(r: &mut R, start: u64, end: u64, want: &[u8; 4]) -> io::Result<Option<(u64, u64)>> {
    let mut pos = start;
    while let Some((fourcc, cs, ce, next)) = next_box(r, pos, end)? {
        if &fourcc == want {
            return Ok(Some((cs, ce)));
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    Ok(None)
}

/// Extract the subtitle tracks of an ISO-BMFF (MP4/MOV) file, as a player lists them. Reads each
/// `trak`'s name, timing and sample tables and pulls its samples from `mdat` (progressive files);
/// then, for *fragmented* files (where `moov`'s tables are empty), also picks up samples from the
/// `moof` fragments across the file. Only the (tiny, few) subtitle samples are read — never the
/// media data.
fn mp4_subtitles<R: Read + Seek>(mut r: R) -> io::Result<Vec<SubtitleTrack>> {
    let file_end = r.seek(SeekFrom::End(0))?;
    let Some(moov) = find_box(&mut r, 0, file_end, b"moov")? else {
        // Nothing says what subtitles it has, or where: damaged, cut short before its index, or
        // not an MP4.
        return Err(bad("no `moov` box"));
    };
    let mut tracks = Vec::new();
    // Once its track list is found, a video is read as it is: cut short or broken further on, the
    // cues before the break are searched like any others.
    if let Err(err) = read_mp4(&mut r, moov, file_end, &mut tracks) {
        if !is_damage(&err) {
            return Err(err);
        }
    }
    Ok(tracks.into_iter().map(|track| SubtitleTrack { name: track.name, cues: track.codec.map(|_| track.cues) }).collect())
}

/// A subtitle `trak`, as read so far.
struct Mp4Track {
    id: u32,
    /// Its sample entry's fourCC — `None` for picture subtitles (a `subp` handler: VobSub).
    codec: Option<[u8; 4]>,
    /// What a player calls it: its title, else its language.
    name: String,
    /// Ticks per second of its sample times (`mdhd`).
    timescale: u32,
    /// Ticks to add to a sample's time for the movie's, from its edit list.
    shift: i64,
    /// Each fragment sample's duration and size, where its fragment gives neither (`trex`).
    defaults: (u32, u32),
    /// Where its next fragment starts, in ticks, for one with no `tfdt` to say.
    next_fragment: u64,
    cues: Vec<Cue>,
}

impl Mp4Track {
    /// Decode one sample, starting `time` ticks into the track, as a cue.
    fn push_cue(&mut self, time: u64, codec: &[u8; 4], sample: &[u8]) {
        let mut text = Vec::new();
        decode_sample(codec, sample, &mut text);
        let Ok(text) = String::from_utf8(text) else { return };
        if text.trim().is_empty() {
            return;
        }
        let ticks = i64::try_from(time).unwrap_or(i64::MAX).saturating_add(self.shift).max(0);
        let start = u64::try_from(ticks).unwrap_or(0) / u64::from(self.timescale.max(1));
        self.cues.push(Cue { start, text });
    }
}

/// Walk an MP4's boxes into `tracks`: the subtitle `trak`s in its `moov` with their progressive
/// samples, then any fragments' samples.
fn read_mp4<R: Read + Seek>(r: &mut R, moov: (u64, u64), file_end: u64, tracks: &mut Vec<Mp4Track>) -> io::Result<()> {
    let (moov_start, moov_end) = moov;
    let movie_timescale = match find_box(r, moov_start, moov_end, b"mvhd")? {
        Some((s, _)) => timescale_at(r, s)?,
        None => 0,
    };
    let mut pos = moov_start;
    while let Some((fourcc, cs, ce, next)) = next_box(r, pos, moov_end)? {
        if &fourcc == b"trak" {
            if let Some(track) = read_trak(r, cs, ce, movie_timescale)? {
                tracks.push(track);
            }
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    for (id, defaults) in read_trex(r, moov_start, moov_end)? {
        if let Some(track) = tracks.iter_mut().find(|track| track.id == id) {
            track.defaults = defaults;
        }
    }
    // Fragmented MP4: samples live in `moof` fragments across the file, not the `moov` tables. This
    // top-level scan seeks past `mdat` (never reads it), so it costs nothing for progressive files.
    if tracks.iter().any(|track| track.codec.is_some()) {
        let mut pos = 0;
        while let Some((fourcc, cs, ce, next)) = next_box(r, pos, file_end)? {
            if &fourcc == b"moof" {
                read_moof(r, pos, cs, ce, tracks)?;
            }
            if next <= pos {
                break;
            }
            pos = next;
        }
    }
    Ok(())
}

/// A `trak`, if it's a subtitle track: what it's called and how it's timed — and, if it holds text,
/// its codec and its progressive samples' cues.
fn read_trak<R: Read + Seek>(r: &mut R, start: u64, end: u64, movie_timescale: u32) -> io::Result<Option<Mp4Track>> {
    let Some((mdia_s, mdia_e)) = find_box(r, start, end, b"mdia")? else { return Ok(None) };
    let Some(handler) = subtitle_handler(r, mdia_s, mdia_e)? else { return Ok(None) };
    let (timescale, language) = read_mdhd(r, mdia_s, mdia_e)?;
    let mut track = Mp4Track {
        id: read_track_id(r, start, end)?,
        codec: None,
        name: read_track_title(r, start, end)?.unwrap_or(language),
        timescale,
        shift: read_edit_shift(r, start, end, movie_timescale, timescale)?,
        defaults: (0, 0),
        next_fragment: 0,
        cues: Vec::new(),
    };
    if handler == Handler::Pictures {
        return Ok(Some(track));
    }
    let Some((minf_s, minf_e)) = find_box(r, mdia_s, mdia_e, b"minf")? else { return Ok(None) };
    let Some((stbl_s, stbl_e)) = find_box(r, minf_s, minf_e, b"stbl")? else { return Ok(None) };
    let codec = stsd_codec(r, stbl_s, stbl_e)?;
    track.codec = Some(codec);
    let sizes = read_sample_sizes(r, stbl_s, stbl_e)?;
    let table = SampleTable {
        times: read_sample_times(r, stbl_s, stbl_e, sizes.len())?,
        sizes,
        chunks: read_chunk_offsets(r, stbl_s, stbl_e)?,
        per_chunk: read_sample_to_chunk(r, stbl_s, stbl_e)?,
    };
    extract_samples(r, &mut track, &codec, &table)?;
    Ok(Some(track))
}

/// The track's numeric ID, from `tkhd`.
fn read_track_id<R: Read + Seek>(r: &mut R, trak_s: u64, trak_e: u64) -> io::Result<u32> {
    let Some((s, _)) = find_box(r, trak_s, trak_e, b"tkhd")? else { return Ok(0) };
    let Some(vf) = read_at::<4, _>(r, s)? else { return Ok(0) };
    // track_ID follows [version+flags] and the creation/modification times (8 bytes at v0, 16 at v1).
    let Some(id) = read_at::<4, _>(r, s + 4 + if vf[0] == 1 { 16 } else { 8 })? else { return Ok(0) };
    Ok(u32::from_be_bytes(id))
}

/// The timescale of the `mvhd` or `mdhd` whose content starts at `s`: past its version and flags,
/// and its creation and modification times (4 bytes each at version 0, 8 at version 1).
fn timescale_at<R: Read + Seek>(r: &mut R, s: u64) -> io::Result<u32> {
    let Some([version]) = read_at::<1, _>(r, s)? else { return Ok(0) };
    Ok(read_at::<4, _>(r, s + if version == 1 { 20 } else { 12 })?.map_or(0, u32::from_be_bytes))
}

/// A track's `mdhd`: ticks per second of its sample times, and its language.
fn read_mdhd<R: Read + Seek>(r: &mut R, mdia_s: u64, mdia_e: u64) -> io::Result<(u32, String)> {
    let Some((s, _)) = find_box(r, mdia_s, mdia_e, b"mdhd")? else { return Ok((0, unpack_language(0))) };
    let Some([version]) = read_at::<1, _>(r, s)? else { return Ok((0, unpack_language(0))) };
    // The language follows the timescale and the duration (4 bytes at version 0, 8 at version 1).
    let packed = read_at::<2, _>(r, s + if version == 1 { 32 } else { 20 })?.map_or(0, u16::from_be_bytes);
    Ok((timescale_at(r, s)?, unpack_language(packed)))
}

/// An ISO 639-2 language code, packed as three 5-bit letters (`a` = 1) — `und` for anything else,
/// QuickTime's old language numbers among them.
fn unpack_language(packed: u16) -> String {
    let letters = [packed >> 10 & 0x1F, packed >> 5 & 0x1F, packed & 0x1F];
    if letters.iter().all(|letter| (1..=26).contains(letter)) {
        letters.iter().map(|&letter| char::from(b'a' - 1 + letter as u8)).collect()
    } else {
        "und".to_owned()
    }
}

/// The longest track title read: a title is a few words.
const MAX_TITLE: u64 = 1024;

/// A track's title: the `name` box in its `udta`, where ffmpeg writes a stream's title (and reads
/// it back from).
fn read_track_title<R: Read + Seek>(r: &mut R, trak_s: u64, trak_e: u64) -> io::Result<Option<String>> {
    let Some((udta_s, udta_e)) = find_box(r, trak_s, trak_e, b"udta")? else { return Ok(None) };
    let Some((s, e)) = find_box(r, udta_s, udta_e, b"name")? else { return Ok(None) };
    let mut title = vec![0u8; usize::try_from(e.saturating_sub(s).min(MAX_TITLE)).unwrap_or(0)];
    r.seek(SeekFrom::Start(s))?;
    let read = fill(r, &mut title)?;
    title.truncate(read);
    Ok(text_field(&title))
}

/// The most edit-list entries read: a subtitle track's has one or two.
const MAX_EDITS: u64 = 16;

/// Ticks of the track's own timescale to add to its samples' times for the movie's, from the start
/// of its edit list, as a player applies it: empty edits delay the track (their durations in the
/// movie's timescale), then the first real edit's `media_time` skips into it.
fn read_edit_shift<R: Read + Seek>(r: &mut R, trak_s: u64, trak_e: u64, movie_timescale: u32, timescale: u32) -> io::Result<i64> {
    let Some((edts_s, edts_e)) = find_box(r, trak_s, trak_e, b"edts")? else { return Ok(0) };
    let Some((s, e)) = find_box(r, edts_s, edts_e, b"elst")? else { return Ok(0) };
    // head = [version+flags][entry_count]; an entry is [segment_duration][media_time][media_rate],
    // its first two 4 bytes each at version 0, 8 at version 1.
    let Some(head) = read_at::<8, _>(r, s)? else { return Ok(0) };
    let wide = head[0] == 1;
    let entry = if wide { 20 } else { 12 };
    let count = u64::from(u32::from_be_bytes([head[4], head[5], head[6], head[7]])).min(MAX_EDITS);
    let mut shift = 0i64;
    for index in 0..count {
        let at = s + 8 + index * entry;
        if at + entry > e {
            break;
        }
        let (duration, media_time) = if wide {
            let Some(b) = read_at::<16, _>(r, at)? else { break };
            (u64::from_be_bytes(b[..8].try_into().unwrap_or_default()), i64::from_be_bytes(b[8..].try_into().unwrap_or_default()))
        } else {
            let Some(b) = read_at::<8, _>(r, at)? else { break };
            (u64::from(u32::from_be_bytes([b[0], b[1], b[2], b[3]])), i64::from(i32::from_be_bytes([b[4], b[5], b[6], b[7]])))
        };
        if media_time != -1 {
            return Ok(shift.saturating_sub(media_time));
        }
        let delay = u128::from(duration) * u128::from(timescale) / u128::from(movie_timescale.max(1));
        shift = shift.saturating_add(i64::try_from(delay).unwrap_or(i64::MAX));
    }
    Ok(shift)
}

/// Each track's fragment defaults (`trex`, in `mvex`): `(track_id, (sample duration, sample size))`.
fn read_trex<R: Read + Seek>(r: &mut R, moov_s: u64, moov_e: u64) -> io::Result<Vec<(u32, (u32, u32))>> {
    let Some((mvex_s, mvex_e)) = find_box(r, moov_s, moov_e, b"mvex")? else { return Ok(Vec::new()) };
    let mut defaults = Vec::new();
    let mut pos = mvex_s;
    while let Some((fourcc, cs, _, next)) = next_box(r, pos, mvex_e)? {
        // [version+flags][track_ID][default_sample_description_index][default_sample_duration]
        // [default_sample_size]
        if &fourcc == b"trex" {
            if let Some(b) = read_at::<20, _>(r, cs)? {
                let word = |at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
                defaults.push((word(4), (word(12), word(16))));
            }
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    Ok(defaults)
}

/// Walk a `moof`'s `traf` boxes, reading the samples of known text tracks.
fn read_moof<R: Read + Seek>(r: &mut R, moof_start: u64, cs: u64, ce: u64, tracks: &mut [Mp4Track]) -> io::Result<()> {
    let mut pos = cs;
    while let Some((fourcc, tcs, tce, next)) = next_box(r, pos, ce)? {
        if &fourcc == b"traf" {
            read_traf(r, moof_start, tcs, tce, tracks)?;
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    Ok(())
}

/// A `traf`: its `tfhd` (whose samples, and their defaults), `tfdt` (when they start) and `trun`
/// (the samples). If they belong to a text track, read them from where the fragment's data lives
/// and decode them as its cues.
fn read_traf<R: Read + Seek>(r: &mut R, moof_start: u64, tcs: u64, tce: u64, tracks: &mut [Mp4Track]) -> io::Result<()> {
    let Some((tfhd_s, _)) = find_box(r, tcs, tce, b"tfhd")? else { return Ok(()) };
    let fragment = read_tfhd(r, moof_start, tfhd_s)?;
    let Some(track) = tracks.iter_mut().find(|track| track.id == fragment.track_id) else { return Ok(()) };
    let Some(codec) = track.codec else { return Ok(()) };
    let Some((trun_s, trun_e)) = find_box(r, tcs, tce, b"trun")? else { return Ok(()) };
    let defaults = (fragment.duration.unwrap_or(track.defaults.0), fragment.size.unwrap_or(track.defaults.1));
    let (data_offset, samples) = read_trun(r, trun_s, trun_e, defaults)?;
    let mut time = match find_box(r, tcs, tce, b"tfdt")? {
        Some((s, _)) => read_tfdt(r, s)?,
        None => track.next_fragment,
    };
    let mut off = fragment.base.wrapping_add(data_offset as u64);
    for (size, duration) in samples {
        if size <= MAX_SUBTITLE_SAMPLE {
            r.seek(SeekFrom::Start(off))?;
            let mut buf = vec![0u8; size as usize];
            let read = fill(r, &mut buf)?;
            track.push_cue(time, &codec, &buf[..read]); // the file may end inside it: as far as it goes
            if read < buf.len() {
                break;
            }
        }
        off = off.saturating_add(size);
        time = time.saturating_add(duration);
    }
    track.next_fragment = time;
    Ok(())
}

/// What a `tfhd` says of its fragment: whose samples they are, where its data is measured from,
/// and the samples' duration and size where its `trun` gives neither.
struct FragmentHeader {
    track_id: u32,
    base: u64,
    duration: Option<u32>,
    size: Option<u32>,
}

/// Parse a `tfhd`. The base defaults to the enclosing `moof` (the common "default-base-is-moof"
/// case) unless an explicit base is present.
fn read_tfhd<R: Read + Seek>(r: &mut R, moof_start: u64, s: u64) -> io::Result<FragmentHeader> {
    let mut fragment = FragmentHeader { track_id: 0, base: moof_start, duration: None, size: None };
    // head = [version+flags][track_id]
    let Some(head) = read_at::<8, _>(r, s)? else { return Ok(fragment) };
    let flags = u32::from_be_bytes([0, head[1], head[2], head[3]]);
    fragment.track_id = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    if flags & 0x00_0001 != 0 {
        let mut b = [0u8; 8]; // base_data_offset
        r.read_exact(&mut b)?;
        fragment.base = u64::from_be_bytes(b);
    }
    if flags & 0x00_0002 != 0 {
        read_u32(r)?; // sample_description_index
    }
    if flags & 0x00_0008 != 0 {
        fragment.duration = Some(read_u32(r)?); // default_sample_duration
    }
    if flags & 0x00_0010 != 0 {
        fragment.size = Some(read_u32(r)?); // default_sample_size
    }
    Ok(fragment)
}

/// A `tfdt`: when its fragment's first sample starts, in the track's ticks.
fn read_tfdt<R: Read + Seek>(r: &mut R, s: u64) -> io::Result<u64> {
    // [version+flags][base_media_decode_time: 4 bytes at version 0, 8 at version 1]
    let Some([version]) = read_at::<1, _>(r, s)? else { return Ok(0) };
    Ok(if version == 1 {
        read_at::<8, _>(r, s + 4)?.map_or(0, u64::from_be_bytes)
    } else {
        read_at::<4, _>(r, s + 4)?.map_or(0, |b| u64::from(u32::from_be_bytes(b)))
    })
}

/// Parse a `trun`: `(data_offset, each sample's (size, duration))` — from the run, or from
/// `defaults` (duration, size) where it gives none. `data_offset` is relative to the fragment's base.
fn read_trun<R: Read + Seek>(r: &mut R, s: u64, e: u64, defaults: (u32, u32)) -> io::Result<(i64, Vec<(u64, u64)>)> {
    let (default_duration, default_size) = defaults;
    // head = [version+flags][sample_count]
    let Some(head) = read_at::<8, _>(r, s)? else { return Ok((0, Vec::new())) };
    let flags = u32::from_be_bytes([0, head[1], head[2], head[3]]);
    let sample_count = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let mut data_offset = 0i64;
    if flags & 0x00_0001 != 0 {
        data_offset = i64::from(read_u32(r)? as i32); // data_offset (i32)
    }
    if flags & 0x00_0004 != 0 {
        read_u32(r)?; // first_sample_flags
    }
    let (has_dur, has_size) = (flags & 0x00_0100 != 0, flags & 0x00_0200 != 0);
    let (has_flags, has_ctime) = (flags & 0x00_0400 != 0, flags & 0x00_0800 != 0);
    let per_sample = 4 * (u64::from(has_dur) + u64::from(has_size) + u64::from(has_flags) + u64::from(has_ctime));
    // Bound the count by what fits in the box (and a ceiling) so a bogus count can't run away.
    let avail = e.saturating_sub(r.stream_position()?);
    let room = avail.checked_div(per_sample).map_or(sample_count, |n| n as usize);
    let count = sample_count.min(room).min(1 << 16);
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let duration = if has_dur { read_u32(r)? } else { default_duration };
        let size = if has_size { read_u32(r)? } else { default_size };
        if has_flags {
            read_u32(r)?;
        }
        if has_ctime {
            read_u32(r)?;
        }
        samples.push((u64::from(size), u64::from(duration)));
    }
    Ok((data_offset, samples))
}

/// Read a big-endian `u32`.
fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

/// What a subtitle track's `hdlr` says it holds.
#[derive(PartialEq)]
enum Handler {
    /// Text: `text` (QuickTime), `sbtl` (tx3g, WebVTT), `subt` (TTML).
    Text,
    /// Pictures: `subp` (VobSub).
    Pictures,
}

/// What a track's `hdlr` names it, if subtitles.
fn subtitle_handler<R: Read + Seek>(r: &mut R, mdia_s: u64, mdia_e: u64) -> io::Result<Option<Handler>> {
    let Some((s, e)) = find_box(r, mdia_s, mdia_e, b"hdlr")? else { return Ok(None) };
    if e.saturating_sub(s) < 12 {
        return Ok(None);
    }
    // b = [version+flags][pre_defined][handler_type]
    let Some(b) = read_at::<12, _>(r, s)? else { return Ok(None) };
    Ok(match &b[8..12] {
        b"text" | b"sbtl" | b"subt" => Some(Handler::Text),
        b"subp" => Some(Handler::Pictures),
        _ => None,
    })
}

/// The codec fourCC of the first sample entry in `stsd`.
fn stsd_codec<R: Read + Seek>(r: &mut R, stbl_s: u64, stbl_e: u64) -> io::Result<[u8; 4]> {
    let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"stsd")? else { return Ok([0; 4]) };
    if e - s < 16 {
        return Ok([0; 4]);
    }
    // s + 12 skips [version+flags][entry_count][entry size].
    Ok(read_at::<4, _>(r, s + 12)?.unwrap_or([0; 4]))
}

/// Per-sample byte sizes, from `stsz` (a single shared size, or one per sample).
fn read_sample_sizes<R: Read + Seek>(r: &mut R, stbl_s: u64, stbl_e: u64) -> io::Result<Vec<u64>> {
    let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"stsz")? else { return Ok(Vec::new()) };
    // head = [version+flags][sample_size][sample_count]
    let Some(head) = read_at::<12, _>(r, s)? else { return Ok(Vec::new()) };
    let count = u32::from_be_bytes([head[8], head[9], head[10], head[11]]) as usize;
    match u32::from_be_bytes([head[4], head[5], head[6], head[7]]) {
        0 => read_u32s(r, e.saturating_sub(s + 12), count).map(|v| v.into_iter().map(u64::from).collect()),
        shared => Ok(vec![u64::from(shared); count]),
    }
}

/// Chunk file offsets, from `stco` (32-bit) or `co64` (64-bit).
fn read_chunk_offsets<R: Read + Seek>(r: &mut R, stbl_s: u64, stbl_e: u64) -> io::Result<Vec<u64>> {
    if let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"stco")? {
        r.seek(SeekFrom::Start(s + 4))?;
        let count = read_u32s(r, 4, 1)?.first().copied().unwrap_or(0) as usize;
        Ok(read_u32s(r, e.saturating_sub(s + 8), count)?.into_iter().map(u64::from).collect())
    } else if let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"co64")? {
        r.seek(SeekFrom::Start(s + 4))?;
        let count = read_u32s(r, 4, 1)?.first().copied().unwrap_or(0) as usize;
        let count = count.min((e.saturating_sub(s + 8) / 8) as usize);
        let mut offs = Vec::with_capacity(count);
        for _ in 0..count {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            offs.push(u64::from_be_bytes(b));
        }
        Ok(offs)
    } else {
        Ok(Vec::new())
    }
}

/// `stsc` entries reduced to `(first_chunk, samples_per_chunk)`.
fn read_sample_to_chunk<R: Read + Seek>(r: &mut R, stbl_s: u64, stbl_e: u64) -> io::Result<Vec<(u32, u32)>> {
    let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"stsc")? else { return Ok(Vec::new()) };
    r.seek(SeekFrom::Start(s + 4))?;
    let count = read_u32s(r, 4, 1)?.first().copied().unwrap_or(0) as usize;
    let count = count.min((e.saturating_sub(s + 8) / 12) as usize);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let f = read_u32s(r, 12, 3)?;
        entries.push((*f.first().unwrap_or(&0), *f.get(1).unwrap_or(&0)));
    }
    Ok(entries)
}

/// Read up to `count` big-endian `u32`s, bounded by `avail` bytes so a bogus count can't over-read.
fn read_u32s<R: Read>(r: &mut R, avail: u64, count: usize) -> io::Result<Vec<u32>> {
    let count = count.min((avail / 4) as usize);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut b = [0u8; 4];
        r.read_exact(&mut b)?;
        out.push(u32::from_be_bytes(b));
    }
    Ok(out)
}

/// Each sample's start, in the track's ticks, from `stts`'s runs of equal durations — `count` of
/// them, the samples there are; any the table doesn't reach start where it ends.
fn read_sample_times<R: Read + Seek>(r: &mut R, stbl_s: u64, stbl_e: u64, count: usize) -> io::Result<Vec<u64>> {
    let (mut times, mut time) = (Vec::with_capacity(count), 0u64);
    if let Some((s, e)) = find_box(r, stbl_s, stbl_e, b"stts")? {
        r.seek(SeekFrom::Start(s + 4))?;
        let runs = read_u32s(r, 4, 1)?.first().copied().unwrap_or(0) as usize;
        let entries = read_u32s(r, e.saturating_sub(s + 8), runs.saturating_mul(2))?;
        'runs: for &[samples, duration] in entries.as_chunks::<2>().0 {
            for _ in 0..samples {
                if times.len() == count {
                    break 'runs;
                }
                times.push(time);
                time = time.saturating_add(u64::from(duration));
            }
        }
    }
    times.resize(count, time);
    Ok(times)
}

/// Where a progressive track's samples are, and when each starts: from `stsz`, `stts`,
/// `stco`/`co64` and `stsc`.
struct SampleTable {
    sizes: Vec<u64>,
    /// Each sample's start, in the track's ticks.
    times: Vec<u64>,
    /// Each chunk's file offset.
    chunks: Vec<u64>,
    /// `(first_chunk, samples_per_chunk)` runs.
    per_chunk: Vec<(u32, u32)>,
}

/// Reconstruct each sample's file offset (its chunk's offset plus the preceding samples in that
/// chunk), read it, and decode it as a cue of `track`. Skips implausibly large samples (not
/// subtitle text).
fn extract_samples<R: Read + Seek>(r: &mut R, track: &mut Mp4Track, codec: &[u8; 4], table: &SampleTable) -> io::Result<()> {
    let mut sample = 0usize;
    for (i, &chunk_off) in table.chunks.iter().enumerate() {
        let chunk = i as u32 + 1; // chunks are 1-based in `stsc`
        let per_chunk = table.per_chunk.iter().rev().find(|(first, _)| *first <= chunk).map_or(0, |&(_, n)| n);
        let mut off = chunk_off;
        for _ in 0..per_chunk {
            let Some(&size) = table.sizes.get(sample) else { return Ok(()) };
            let time = table.times.get(sample).copied().unwrap_or(0);
            sample += 1;
            if size <= MAX_SUBTITLE_SAMPLE {
                r.seek(SeekFrom::Start(off))?;
                let mut buf = vec![0u8; size as usize];
                let read = fill(r, &mut buf)?;
                track.push_cue(time, codec, &buf[..read]); // the file may end inside it: as far as it goes
                if read < buf.len() {
                    return Ok(());
                }
            }
            off = off.saturating_add(size);
        }
    }
    Ok(())
}

/// Decode one subtitle sample's text, per codec.
fn decode_sample(codec: &[u8; 4], sample: &[u8], out: &mut Vec<u8>) {
    match codec {
        // tx3g / QuickTime text: `[u16 text length][UTF-8 text][optional style boxes]` — the text as
        // far as the sample goes, should the file end inside it.
        b"tx3g" | b"text" => {
            let len = sample.get(..2).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
            if let Some(text) = sample.get(2..(2 + len).min(sample.len())) {
                push_text(out, text);
            }
        }
        // WebVTT: cue boxes; the visible text lives in `payl` boxes.
        b"wvtt" => push_wvtt_payloads(sample, out),
        // TTML (`stpp`) and anything else text-ish: take the whole (UTF-8) sample.
        _ => push_text(out, sample),
    }
}

/// Append `bytes` as a trimmed line, if it's non-empty UTF-8 (see [`utf8_text`]).
fn push_text(out: &mut Vec<u8>, bytes: &[u8]) {
    if let Some(text) = utf8_text(bytes) {
        let text = text.trim();
        if !text.is_empty() {
            out.extend_from_slice(text.as_bytes());
            out.push(b'\n');
        }
    }
}

/// Walk a WebVTT sample's boxes, appending each `payl` (cue payload); descends into `vttc` cues.
fn push_wvtt_payloads(sample: &[u8], out: &mut Vec<u8>) {
    let mut pos = 0;
    while pos + 8 <= sample.len() {
        let size = u32::from_be_bytes([sample[pos], sample[pos + 1], sample[pos + 2], sample[pos + 3]]) as usize;
        if size < 8 {
            break;
        }
        let end = (pos + size).min(sample.len());
        match &sample[pos + 4..pos + 8] {
            b"vttc" => push_wvtt_payloads(&sample[pos + 8..end], out),
            b"payl" => push_text(out, &sample[pos + 8..end]),
            _ => {}
        }
        pos += size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_decodes_ids_and_sizes() {
        // 1-byte size 0x82 → value 2 (marker 0x80 cleared); raw keeps it.
        assert_eq!(read_vint(&mut &[0x82u8][..]).unwrap(), Some((0x82, 2, 1)));
        // 2-byte 0x4002 → value 2, raw 0x4002.
        assert_eq!(read_vint(&mut &[0x40u8, 0x02][..]).unwrap(), Some((0x4002, 2, 2)));
        // 4-byte EBML header id 0x1A45DFA3 → raw is the element id.
        assert_eq!(read_vint(&mut &[0x1Au8, 0x45, 0xDF, 0xA3][..]).unwrap().map(|v| v.0), Some(0x1A45_DFA3));
        // Clean EOF.
        assert_eq!(read_vint(&mut &[][..]).unwrap(), None);
    }

    #[test]
    fn unknown_size_is_reported_as_none() {
        // 0x01 FF..FF (8-byte all-ones value) is the "unknown size" marker.
        let mut bytes: &[u8] = &[0xEC, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        assert_eq!(read_element(&mut bytes).unwrap(), Some((0xEC, None)));
    }

    #[test]
    fn bencode_collects_text_strings_and_skips_binary_and_ints() {
        let out = bencode_text(b"d4:name5:hello4:spamli42e3:fooee").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("name") && text.contains("hello") && text.contains("foo"));
        assert!(!text.contains("42")); // integers aren't strings
    }

    #[test]
    fn bencode_skips_the_binary_pieces_blob() {
        // A byte-string whose bytes aren't UTF-8 text (NUL etc.) is dropped.
        let out = bencode_text(b"d6:pieces4:\x00\x01\x02\x03e").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("pieces")); // the key is text
        assert!(!text.contains('\u{0}')); // the blob value is not emitted
    }

    #[test]
    fn matroska_extracts_a_text_subtitle_block() {
        // A minimal Matroska: Segment { Tracks { TrackEntry(#1, subtitle, S_TEXT/UTF8, no
        // language) }, Cluster { SimpleBlock(track 1, "hi there") } } — no timestamps anywhere, and
        // no language, which the spec reads as English.
        let entry = element(ID_TRACK_ENTRY, &[element(ID_TRACK_NUMBER, &[1]), element(ID_TRACK_TYPE, &[0x11]), element(ID_CODEC_ID, b"S_TEXT/UTF8")].concat());
        let file = matroska(&[element(ID_TRACKS, &entry), cluster(0, &[block(1, 0, "hi there")])]);
        assert_eq!(lines(matroska_subtitles(io::Cursor::new(file))), ["eng 0:00:00:hi there"]);
    }

    /// An EBML element: `id`, its data's size (as an 8-byte vint, which fits any), then the data.
    fn element(id: u64, data: &[u8]) -> Vec<u8> {
        let id = id.to_be_bytes();
        let mut bytes = id[id.iter().position(|&byte| byte != 0).unwrap()..].to_vec();
        bytes.push(0x01);
        bytes.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
        bytes.extend_from_slice(data);
        bytes
    }

    /// A Matroska file: an (empty) EBML header, then a Segment of `children`.
    fn matroska(children: &[Vec<u8>]) -> Vec<u8> {
        [element(0x1A45_DFA3, &[]), element(ID_SEGMENT, &children.concat())].concat()
    }

    /// A subtitle track's `TrackEntry`: its number, `CodecID` and `Language`.
    fn subtitle_entry(number: u8, codec: &str, language: &str) -> Vec<u8> {
        let fields = [
            element(ID_TRACK_NUMBER, &[number]),
            element(ID_TRACK_TYPE, &[0x11]),
            element(ID_CODEC_ID, codec.as_bytes()),
            element(ID_LANGUAGE, language.as_bytes()),
        ];
        element(ID_TRACK_ENTRY, &fields.concat())
    }

    /// A Cluster at `time` ticks, holding `blocks`.
    fn cluster(time: u16, blocks: &[Vec<u8>]) -> Vec<u8> {
        element(ID_CLUSTER, &[&element(ID_CLUSTER_TIMESTAMP, &time.to_be_bytes())[..], &blocks.concat()].concat())
    }

    /// A SimpleBlock of track `track`, `offset` ticks into its Cluster, its frame `text`.
    fn block(track: u8, offset: i16, text: &str) -> Vec<u8> {
        let header = [&[0x80 | track][..], &offset.to_be_bytes(), &[0x80]].concat(); // flags: keyframe, unlaced
        element(ID_SIMPLE_BLOCK, &[&header[..], text.as_bytes()].concat())
    }

    /// A video's subtitles as `gg` prints them, behind the path: each line's place, then the line.
    fn lines(tracks: io::Result<Vec<SubtitleTrack>>) -> Vec<String> {
        placed(&subtitle_sections(&tracks.unwrap()).unwrap_or_else(|unread| panic!("{unread:?}")))
    }

    /// Decoded text as `gg` prints it, behind the path: each line's place, then the line.
    fn placed(sections: &[Section]) -> Vec<String> {
        let mut lines = Vec::new();
        for section in sections {
            let Place::Lines(places) = &section.place else { panic!("a subtitle line is placed on its own") };
            let text = std::str::from_utf8(&section.text).unwrap();
            assert_eq!(places.len(), text.lines().count(), "a place for every line: {text:?}");
            lines.extend(places.iter().zip(text.lines()).map(|(place, line)| format!("{place}:{line}")));
        }
        lines
    }

    #[test]
    fn subtitle_lines_are_placed_by_track_and_cue_start() {
        // A tick here is a tenth of a second, so the Cluster at 36,000 ticks starts an hour in.
        // Each track's lines come together, in the file's track order; a cue of two lines places
        // both, and a blank one places none.
        let info = element(ID_INFO, &element(ID_TIMESTAMP_SCALE, &100_000_000u32.to_be_bytes()));
        let tracks = element(ID_TRACKS, &[subtitle_entry(1, "S_TEXT/UTF8", "eng"), subtitle_entry(2, "S_TEXT/UTF8", "jpn")].concat());
        let blocks = [block(1, 0, "Welcome in"), block(2, 10, "いらっしゃいませ"), block(1, 25, "Two lines\nof cue"), block(1, 40, " ")];
        let file = matroska(&[info, tracks, cluster(36_000, &blocks)]);
        assert_eq!(
            lines(matroska_subtitles(io::Cursor::new(file))),
            ["eng 1:00:00:Welcome in", "eng 1:00:02:Two lines", "eng 1:00:02:of cue", "jpn 1:00:01:いらっしゃいませ"],
        );
    }

    #[test]
    fn tracks_sharing_a_language_are_told_apart_by_their_number() {
        // Two English text tracks, with an English image track between them that a player's menu
        // counts too; the French one is the only French, so its language is enough.
        let tracks = [
            subtitle_entry(1, "S_TEXT/UTF8", "eng"),
            subtitle_entry(2, "S_HDMV/PGS", "eng"),
            subtitle_entry(3, "S_TEXT/ASS", "eng"),
            subtitle_entry(4, "S_TEXT/UTF8", "fre"),
        ];
        let blocks = [block(1, 0, "full"), block(3, 0, "signs"), block(4, 0, "complet")];
        let file = matroska(&[element(ID_TRACKS, &tracks.concat()), cluster(0, &blocks)]);
        assert_eq!(
            lines(matroska_subtitles(io::Cursor::new(file))),
            ["eng #1 0:00:00:full", "eng #3 0:00:00:signs", "fre 0:00:00:complet"],
        );
    }

    #[test]
    fn a_language_tag_wins_over_the_older_language_code() {
        let fields = [
            element(ID_TRACK_NUMBER, &[1]),
            element(ID_TRACK_TYPE, &[0x11]),
            element(ID_CODEC_ID, b"S_TEXT/UTF8"),
            element(ID_LANGUAGE, b"por"),
            element(ID_LANGUAGE_BCP47, b"pt-BR"),
        ];
        let file = matroska(&[element(ID_TRACKS, &element(ID_TRACK_ENTRY, &fields.concat())), cluster(0, &[block(1, 0, "Oi")])]);
        assert_eq!(lines(matroska_subtitles(io::Cursor::new(file))), ["pt-BR 0:00:00:Oi"]);
    }

    #[test]
    fn sizes_no_real_file_has_are_not_believed() {
        // A `Tracks` claiming a terabyte, in a Segment of unknown size: believed, it would be
        // allocated. It marks the file damaged instead.
        let mut file = element(0x1A45_DFA3, &[]);
        file.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]); // Segment, size unknown
        file.extend_from_slice(&[0x16, 0x54, 0xAE, 0x6B, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]); // Tracks, 1 << 40 bytes
        let read = matroska_subtitles(io::Cursor::new(file));
        assert_eq!(read.err().map(|err| err.kind()), Some(io::ErrorKind::InvalidData));
        // A subtitle block past the sample cap is skipped, and the cue after it still read.
        let tracks = element(ID_TRACKS, &subtitle_entry(1, "S_TEXT/UTF8", "eng"));
        let huge = "x".repeat(usize::try_from(MAX_SUBTITLE_SAMPLE).unwrap());
        let file = matroska(&[tracks, cluster(0, &[block(1, 0, &huge), block(1, 1000, "after")])]);
        assert_eq!(lines(matroska_subtitles(io::Cursor::new(file))), ["eng 0:00:01:after"]);
    }

    #[test]
    fn formats_are_told_by_extension_in_any_case() {
        for (name, format) in [
            ("receipt.pdf", Some(Format::Pdf)),
            ("SCAN.PDF", Some(Format::Pdf)),
            ("ubuntu.torrent", Some(Format::Torrent)),
            ("show.MKV", Some(Format::Matroska)),
            ("talk.webm", Some(Format::Matroska)),
            ("clip.m4v", Some(Format::Mp4)),
            ("notes.txt", None),
            ("pdf", None),
        ] {
            assert_eq!(Format::of(Path::new(name)), format, "{name}");
        }
    }

    /// An MP4 box: its size, its fourCC, then `content`.
    fn mp4box(fourcc: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut v = u32::try_from(8 + content.len()).unwrap().to_be_bytes().to_vec();
        v.extend_from_slice(fourcc);
        v.extend_from_slice(content);
        v
    }

    #[test]
    fn mp4_extracts_a_tx3g_sample() {
        // moov { trak { mdia { hdlr[handler=text] minf { stbl { stsd[tx3g] stsz stco stsc } } } } },
        // and an mdat whose single tx3g sample `[u16 len=6]["hi mp4"]` sits at file offset 8 (mdat
        // is written first, so `stco` can point straight at it).
        let hdlr = mp4box(b"hdlr", &[&[0u8; 8][..], b"text", &[0u8; 13]].concat()); // handler_type at [8..12]
        let stsd = mp4box(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &mp4box(b"tx3g", &[0u8; 8])].concat());
        let stsz = mp4box(b"stsz", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8]); // shared=0, count=1, size=8
        let stco = mp4box(b"stco", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8]); // count=1, offset=8
        let stsc = mp4box(b"stsc", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]);
        let stbl = mp4box(b"stbl", &[stsd, stsz, stco, stsc].concat());
        let mdia = mp4box(b"mdia", &[hdlr, mp4box(b"minf", &stbl)].concat());
        let moov = mp4box(b"moov", &mp4box(b"trak", &mdia));
        let mdat = mp4box(b"mdat", &[&[0u8, 6][..], b"hi mp4"].concat());
        // No `mdhd` to give it a language or a timescale: `und`, at the start.
        assert_eq!(lines(mp4_subtitles(io::Cursor::new([mdat, moov].concat()))), ["und 0:00:00:hi mp4"]);
    }

    #[test]
    fn mp4_extracts_a_fragmented_tx3g_sample() {
        // Fragmented MP4: `moov` defines track 1 (text/tx3g) with EMPTY sample tables; the sample
        // lives in a `moof` fragment. `mdat` is written first, so `tfhd`'s explicit base_data_offset
        // can point straight at it (offset 8) without a chicken-and-egg size computation.
        let tkhd = mp4box(b"tkhd", &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]); // v0; track_id=1 at [12]
        let hdlr = mp4box(b"hdlr", &[&[0u8; 8][..], b"text", &[0u8; 13]].concat());
        let stsd = mp4box(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &mp4box(b"tx3g", &[0u8; 8])].concat());
        let empty = mp4box(b"stsz", &[0u8; 12]); // count=0 (fragmented → no progressive samples)
        let stbl = mp4box(
            b"stbl",
            &[stsd, empty, mp4box(b"stco", &[0u8; 8]), mp4box(b"stsc", &[0u8; 8])].concat(),
        );
        let mdia = mp4box(b"mdia", &[hdlr, mp4box(b"minf", &stbl)].concat());
        let moov = mp4box(b"moov", &mp4box(b"trak", &[tkhd, mdia].concat()));
        // tfhd: flags=0x000001 (base_data_offset present), track_id=1, base=8.
        let tfhd = mp4box(b"tfhd", &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 8]);
        // trun: flags=0x000200 (sample_size present), sample_count=1, sample_size=9.
        let trun = mp4box(b"trun", &[0, 0, 2, 0, 0, 0, 0, 1, 0, 0, 0, 9]);
        let moof = mp4box(b"moof", &mp4box(b"traf", &[tfhd, trun].concat()));
        let mdat = mp4box(b"mdat", &[&[0u8, 7][..], b"hi frag"].concat()); // 9-byte sample at offset 8
        assert_eq!(lines(mp4_subtitles(io::Cursor::new([mdat, moov, moof].concat()))), ["und 0:00:00:hi frag"]);
    }

    #[test]
    fn wvtt_payload_text_is_extracted() {
        // A WebVTT sample is boxes: a `vttc` cue wrapping a `payl` payload holding the cue text.
        let mut payl = (8u32 + 9).to_be_bytes().to_vec();
        payl.extend_from_slice(b"payl");
        payl.extend_from_slice(b"hello cue");
        let mut vttc = u32::try_from(8 + payl.len()).unwrap().to_be_bytes().to_vec();
        vttc.extend_from_slice(b"vttc");
        vttc.extend_from_slice(&payl);
        let mut out = Vec::new();
        push_wvtt_payloads(&vttc, &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "hello cue\n");
    }

    #[test]
    fn mp4_extracts_fragmented_wvtt_cues() {
        // A real fragmented WebVTT stream (Sintel), trimmed to its `moov` init plus the first two
        // `moof`/`mdat` fragments — so this covers the multi-fragment scan on real muxer output.
        // Each cue is placed where ffmpeg's demuxer times its sample (107.25 s and 111.8 s), the
        // track named by its language: it has no title.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sintel-wvtt.mp4"));
        let lines = lines(mp4_subtitles(io::Cursor::new(&bytes[..])));
        assert!(lines.iter().any(|line| line == "eng 0:01:47:This blade has a dark past."), "fragment 1: {lines:?}");
        assert!(lines.iter().any(|line| line == "eng 0:01:51:It has shed much innocent blood."), "fragment 2: {lines:?}");
    }

    #[test]
    fn mp4_extracts_fragmented_stpp_ttml() {
        // A real fragmented TTML stream (Tears of Steel). The source segment's sample was an empty
        // `<tt/>`, so a real cue was spliced into its `mdat` for a meaningful assertion; `stpp`
        // decoding returns the whole TTML document, so we match the cue text within it.
        // Its one sample starts at 0 s, where ffmpeg's demuxer times it too.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tears-of-steel-stpp.mp4"));
        let lines = lines(mp4_subtitles(io::Cursor::new(&bytes[..])));
        let cue = lines.iter().find(|line| line.contains("A storm approaches."));
        assert!(cue.is_some_and(|line| line.starts_with("eng 0:00:00:")), "stpp cue: {lines:?}");
    }

    #[test]
    fn matroska_extracts_a_text_subtitle_from_a_real_file() {
        // A ~120-byte slice of a real Matroska file — its EBML header, `S_TEXT/UTF8` subtitle track,
        // and one real cue — reconstructed from a 9.77 GB source with the bulk A/V dropped.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phm.mkv"));
        let lines = lines(matroska_subtitles(io::Cursor::new(&bytes[..])));
        assert!(lines.iter().any(|line| line.contains("So don't take it personally")), "{lines:?}");
    }

    #[test]
    fn matroska_reads_a_webvtt_cue_s_text_and_not_its_identifier_or_settings() {
        // Two English cues ffmpeg muxed into Matroska, as `D_WEBVTT/SUBTITLES` (ffmpeg's codec ID
        // for WebVTT in `.mkv` and `.webm` alike): the first, at 0 s, with an identifier
        // (`first-cue`) and settings (`align:start position:0%`), the second, at 2 s, with neither.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/d-webvtt.mkv"));
        assert_eq!(
            lines(matroska_subtitles(io::Cursor::new(&bytes[..]))),
            ["eng 0:00:00:The kitchen ran out of salt", "eng 0:00:02:Second helping is on the house"],
        );
    }

    #[test]
    fn a_track_s_title_names_it_where_it_has_one() {
        let titled = [
            element(ID_TRACK_NUMBER, &[1]),
            element(ID_TRACK_TYPE, &[0x11]),
            element(ID_CODEC_ID, b"S_TEXT/UTF8"),
            element(ID_NAME, b"English (auto-generated)"),
            element(ID_LANGUAGE, b"eng"),
        ];
        let tracks = [element(ID_TRACK_ENTRY, &titled.concat()), subtitle_entry(2, "S_TEXT/UTF8", "jpn")];
        let file = matroska(&[element(ID_TRACKS, &tracks.concat()), cluster(0, &[block(1, 0, "Welcome in"), block(2, 0, "いらっしゃい")])]);
        assert_eq!(
            lines(matroska_subtitles(io::Cursor::new(file))),
            ["English (auto-generated) 0:00:00:Welcome in", "jpn 0:00:00:いらっしゃい"],
        );
    }

    #[test]
    fn a_video_whose_subtitles_are_all_pictures_says_so() {
        let pictures = element(ID_TRACKS, &subtitle_entry(1, "S_HDMV/PGS", "eng"));
        let read = matroska_subtitles(io::Cursor::new(matroska(&[pictures, cluster(0, &[])])));
        assert!(matches!(subtitle_sections(&read.unwrap()), Err(Unread::PictureSubtitles)));
        // Beside a text track, picture subtitles are only left out; with none at all, there is
        // simply nothing to read.
        let mixed = element(ID_TRACKS, &[subtitle_entry(1, "S_HDMV/PGS", "eng"), subtitle_entry(2, "S_TEXT/UTF8", "eng")].concat());
        let file = matroska(&[mixed, cluster(0, &[block(2, 0, "text")])]);
        assert_eq!(lines(matroska_subtitles(io::Cursor::new(file))), ["eng 0:00:00:text"]);
        assert!(subtitle_sections(&[]).is_ok_and(|sections| sections.is_empty()));
    }

    #[test]
    fn mp4_tracks_are_named_and_timed_as_ffmpeg_reads_them() {
        // Muxed by ffmpeg from two subtitle files: an English track titled `English (SDH)`, and an
        // untitled French one — read back by ffmpeg with these very cue times.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tx3g-tracks.mp4"));
        assert_eq!(
            lines(mp4_subtitles(io::Cursor::new(&bytes[..]))),
            [
                "English (SDH) 0:00:00:The kitchen ran out of salt",
                "English (SDH) 0:00:05:Second helping is on the house",
                "fre 0:00:01:Bonjour",
                "fre 0:20:05:La cuisine est fermée",
            ],
        );
    }

    #[test]
    fn an_mp4_track_s_edit_list_moves_its_cues_as_a_player_does() {
        // A movie ticking in milliseconds whose subtitle track (also in milliseconds) waits 3 s
        // behind an empty edit, then plays from its media's start; its one sample is at 1 s.
        let full = |version_flags: [u8; 4], body: &[u8]| [&version_flags[..], body].concat();
        let mvhd = mp4box(b"mvhd", &full([0; 4], &[&[0u8; 8][..], &1000u32.to_be_bytes()].concat()));
        let elst = mp4box(b"elst", &full([0; 4], &[&2u32.to_be_bytes()[..], &3000u32.to_be_bytes(), &(-1i32).to_be_bytes(), &[0, 1, 0, 0], &5000u32.to_be_bytes(), &0i32.to_be_bytes(), &[0, 1, 0, 0]].concat()));
        let mdhd = mp4box(b"mdhd", &full([0; 4], &[&[0u8; 8][..], &1000u32.to_be_bytes(), &[0, 0, 0, 0], &0x15C7u16.to_be_bytes(), &[0, 0]].concat()));
        let hdlr = mp4box(b"hdlr", &[&[0u8; 8][..], b"sbtl", &[0u8; 13]].concat());
        let stsd = mp4box(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &mp4box(b"tx3g", &[0u8; 8])].concat());
        let stts = mp4box(b"stts", &full([0; 4], &[&2u32.to_be_bytes()[..], &1u32.to_be_bytes(), &1000u32.to_be_bytes(), &1u32.to_be_bytes(), &2000u32.to_be_bytes()].concat()));
        let stsz = mp4box(b"stsz", &full([0; 4], &[&0u32.to_be_bytes()[..], &2u32.to_be_bytes(), &2u32.to_be_bytes(), &7u32.to_be_bytes()].concat()));
        let stco = mp4box(b"stco", &full([0; 4], &[&1u32.to_be_bytes()[..], &8u32.to_be_bytes()].concat()));
        let stsc = mp4box(b"stsc", &full([0; 4], &[&1u32.to_be_bytes()[..], &1u32.to_be_bytes(), &2u32.to_be_bytes(), &1u32.to_be_bytes()].concat()));
        let stbl = mp4box(b"stbl", &[stsd, stts, stsz, stco, stsc].concat());
        let mdia = mp4box(b"mdia", &[mdhd, hdlr, mp4box(b"minf", &stbl)].concat());
        let trak = mp4box(b"trak", &[mp4box(b"edts", &elst), mdia].concat());
        let mdat = mp4box(b"mdat", &[&[0u8, 0][..], &[0, 5], b"hello"].concat()); // an empty sample, then "hello"
        let file = [mdat, mp4box(b"moov", &[mvhd, trak].concat())].concat();
        assert_eq!(lines(mp4_subtitles(io::Cursor::new(file))), ["eng 0:00:04:hello"]);
    }

    #[test]
    fn an_mp4_whose_subtitles_are_all_pictures_says_so() {
        let hdlr = mp4box(b"hdlr", &[&[0u8; 8][..], b"subp", &[0u8; 13]].concat());
        let moov = mp4box(b"moov", &mp4box(b"trak", &mp4box(b"mdia", &hdlr)));
        let read = mp4_subtitles(io::Cursor::new(moov));
        assert!(matches!(subtitle_sections(&read.unwrap()), Err(Unread::PictureSubtitles)));
    }

    /// Every way a decodable file can keep its text from a search, as [`extract`] reports it.
    #[test]
    fn a_file_whose_text_can_t_be_read_says_why() {
        let dir = std::env::temp_dir().join(format!("bashrs_unread_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf = |content: &str, trailer: &str| {
            format!(
                "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
                 2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
                 3 0 obj << /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj\n\
                 4 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
                 5 0 obj << /Length {} >> stream\n{content}\nendstream endobj\ntrailer {trailer}\n%%EOF\n",
                content.len()
            )
        };
        let unread = |name: &str, contents: &[u8], delving: bool| {
            let path = dir.join(name);
            std::fs::write(&path, contents).unwrap();
            match extract(&path, delving) {
                Some(Ok(Decoded::Unread(unread))) => Some(unread),
                _ => None,
            }
        };
        let open = "<< /Root 1 0 R >>";
        assert_eq!(unread("receipt.pdf", pdf("BT /F1 12 Tf (Paid) Tj ET", open).as_bytes(), false), None, "it has text");
        assert_eq!(unread("locked.pdf", pdf("BT /F1 12 Tf (Paid) Tj ET", "<< /Root 1 0 R /Encrypt << /Filter /Standard >> >>").as_bytes(), false), Some(Unread::Encrypted));
        assert_eq!(unread("scan.pdf", pdf("0 0 m 100 100 l S", open).as_bytes(), false), Some(Unread::NoText), "a line drawn, no text");
        assert_eq!(unread("error.pdf", b"<html>404 Not Found</html>", false), Some(Unread::Damaged), "a web page saved as a PDF");
        assert_eq!(unread("cut.torrent", b"d4:name5:hel", true), Some(Unread::Damaged), "cut short");
        assert_eq!(unread("movie.mkv", b"not a video", true), Some(Unread::Damaged));
        assert_eq!(unread("movie.mp4", b"not a video", true), Some(Unread::Damaged));
        // Too big to hold, told from its size alone: a sparse file, its bytes never read.
        let huge = dir.join("scan-of-a-book.pdf");
        std::fs::File::create(&huge).unwrap().set_len(MAX_WHOLE + 1).unwrap();
        assert!(matches!(extract(&huge, false), Some(Ok(Decoded::Unread(Unread::TooLarge)))));
        // Media without `--delve` is left alone by choice — nothing to name.
        assert!(extract(&dir.join("movie.mkv"), false).is_none());
        // A file that can't be read at all is the caller's to report.
        assert!(matches!(extract(&dir.join("gone.pdf"), false), Some(Err(err)) if err.kind() == io::ErrorKind::NotFound));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once its track list is read, a video is read as it is: cut short — even inside a cue — its
    /// cues are searched as far as they go, and it isn't damaged. Cut before its track list, nothing
    /// says what subtitles it has: that is damage.
    #[test]
    fn a_video_cut_short_is_read_as_it_is() {
        let tracks = element(ID_TRACKS, &subtitle_entry(1, "S_TEXT/UTF8", "eng"));
        let whole = matroska(&[tracks, cluster(0, &[block(1, 0, "first cue"), block(1, 1000, "second cue")])]);
        let inside_the_second = memchr::memmem::find(&whole, b"second cue").unwrap() + "second c".len();
        assert_eq!(lines(matroska_subtitles(io::Cursor::new(&whole[..inside_the_second]))), ["eng 0:00:00:first cue", "eng 0:00:01:second c"]);
        // Cut inside the first Cluster's header, before any cue: read as it is, there's no cue in it.
        let before_any_cue = memchr::memmem::find(&whole, &[0x1F, 0x43, 0xB6, 0x75]).unwrap() + 6;
        assert!(lines(matroska_subtitles(io::Cursor::new(&whole[..before_any_cue]))).is_empty());
        let inside_the_track_list = memchr::memmem::find(&whole, b"S_TEXT").unwrap();
        let cut = matroska_subtitles(io::Cursor::new(&whole[..inside_the_track_list]));
        assert!(cut.as_ref().is_err_and(is_damage), "cut before its track list");
    }

    #[test]
    fn an_mp4_cut_inside_a_sample_keeps_its_text_as_far_as_it_goes() {
        // `moov` first, then the `mdat` holding one tx3g sample `[u16 len=11]["hello world"]` —
        // `stco` points past `moov`, whose size doesn't depend on the offset it holds.
        let moov = |offset: u32| {
            let hdlr = mp4box(b"hdlr", &[&[0u8; 8][..], b"text", &[0u8; 13]].concat());
            let stsd = mp4box(b"stsd", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &mp4box(b"tx3g", &[0u8; 8])].concat());
            let stsz = mp4box(b"stsz", &[&[0u8; 8][..], &1u32.to_be_bytes(), &13u32.to_be_bytes()].concat());
            let stco = mp4box(b"stco", &[&[0, 0, 0, 0, 0, 0, 0, 1][..], &offset.to_be_bytes()].concat());
            let stsc = mp4box(b"stsc", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1]);
            let stbl = mp4box(b"stbl", &[stsd, stsz, stco, stsc].concat());
            mp4box(b"moov", &mp4box(b"trak", &mp4box(b"mdia", &[hdlr, mp4box(b"minf", &stbl)].concat())))
        };
        let at = u32::try_from(moov(0).len() + 8).unwrap();
        let whole = [moov(at), mp4box(b"mdat", &[&[0u8, 11][..], b"hello world"].concat())].concat();
        assert_eq!(lines(mp4_subtitles(io::Cursor::new(&whole[..]))), ["und 0:00:00:hello world"]);
        assert_eq!(lines(mp4_subtitles(io::Cursor::new(&whole[..whole.len() - 3]))), ["und 0:00:00:hello wo"]);
        // With its index (`moov`) cut off, nothing says where its cues are: damage.
        let mdat_only = mp4box(b"mdat", &[&[0u8, 11][..], b"hello world"].concat());
        assert!(mp4_subtitles(io::Cursor::new(mdat_only)).as_ref().is_err_and(is_damage));
    }

    #[test]
    fn a_cue_cut_inside_a_character_keeps_the_text_before_it() {
        let greeting = "いらっしゃい".as_bytes();
        assert_eq!(utf8_text(&greeting[..4]), Some("い"), "cut one byte into the second character");
        assert_eq!(utf8_text(greeting), Some("いらっしゃい"));
        assert_eq!(utf8_text(b"ab\xffcd"), None, "a broken byte mid-text is not a cut");
    }

    #[test]
    fn a_webm_webvtt_frame_s_text_follows_two_lines_ended_any_webvtt_way() {
        assert_eq!(after_two_lines(b"id\nalign:start\ncue text"), b"cue text");
        assert_eq!(after_two_lines(b"\n\ncue text"), b"cue text"); // both lines are there even empty
        assert_eq!(after_two_lines(b"id\r\nline:0\r\ntwo\r\nlines"), b"two\r\nlines");
        assert_eq!(after_two_lines(b"id\rline:0\rcue"), b"cue");
        // Short of two lines, the frame holds no cue text — never its settings.
        assert_eq!(after_two_lines(b"align:start position:0%\n"), b"");
        assert_eq!(after_two_lines(b""), b"");
    }

    #[test]
    fn subtitle_codecs_are_read_only_when_they_carry_text() {
        assert_eq!(TextCodec::of("S_TEXT/UTF8"), Some(TextCodec::SText));
        assert_eq!(TextCodec::of("S_TEXT/ASS"), Some(TextCodec::SText));
        assert_eq!(TextCodec::of("S_TEXT/WEBVTT"), Some(TextCodec::SText)); // its cue settings live outside the frame
        assert_eq!(TextCodec::of("D_WEBVTT/SUBTITLES"), Some(TextCodec::WebmVtt));
        assert_eq!(TextCodec::of("D_WEBVTT/CAPTIONS"), Some(TextCodec::WebmVtt));
        for image in ["S_HDMV/PGS", "S_VOBSUB", "S_DVBSUB", "S_IMAGE/BMP"] {
            assert_eq!(TextCodec::of(image), None, "{image}");
        }
    }
}
