//! Binary-format decoders behind `gg --delve`: pull searchable text out of files `gg` would
//! otherwise skip as binary, by understanding their structure rather than scanning raw bytes. One
//! decoder per format; [`extract`] dispatches by extension, so adding a format is a decoder plus a
//! dispatch arm. Files with no decoder return `None` and stay skipped (no raw `strings`-style
//! fallback — that's the noisy, slow approach `gg` deliberately avoids).
//!
//! - `.torrent` — bencode: the human-readable string values (name, file paths, tracker URLs),
//!   skipping the binary `pieces` SHA-1 blob.
//! - `.mkv`/`.mka`/`.webm` — Matroska: the text subtitle tracks (`S_TEXT/*`), extracted by walking
//!   the EBML tree and *seeking past* video/audio payloads — so a multi-GB file is read in
//!   milliseconds (only the tiny subtitle blocks are actually read), not in full.

use std::fs::File;
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::Path;

/// Extract searchable UTF-8 text from `path` if its format has a decoder, as newline-joined lines
/// (each becomes one searchable "line"). Returns `None` only when the extension isn't one we decode
/// — so callers keep skipping such files rather than raw-scanning them. A decodable file that fails
/// to parse yields `Some(empty)`, never a fall-through to a raw scan of (potentially huge) binary.
pub(crate) fn extract(path: &Path) -> Option<Vec<u8>> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "torrent" => Some(std::fs::read(path).map(|bytes| bencode_text(&bytes)).unwrap_or_default()),
        "mkv" | "mka" | "webm" => Some(
            File::open(path)
                .map(|f| io::BufReader::with_capacity(128 * 1024, f))
                .ok()
                .and_then(|r| matroska_subtitles(r).ok())
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

// ---- bencode (.torrent) ---------------------------------------------------------------------

/// Walk the bencode tree in `data`, collecting every UTF-8 byte-string as a line. Non-UTF-8
/// byte-strings (the `pieces` hash blob) and integers are skipped. Malformed input just stops early.
fn bencode_text(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0;
    while walk_bencode(data, &mut pos, &mut out).is_some() && pos < data.len() {}
    out
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

// ---- Matroska/EBML (.mkv/.mka/.webm) --------------------------------------------------------

const ID_SEGMENT: u64 = 0x1853_8067;
const ID_SEEK_HEAD: u64 = 0x114D_9B74;
const ID_INFO: u64 = 0x1549_A966;
const ID_TRACKS: u64 = 0x1654_AE6B;
const ID_CUES: u64 = 0x1C53_BB6B;
const ID_CHAPTERS: u64 = 0x1043_A770;
const ID_TAGS: u64 = 0x1254_C367;
const ID_ATTACHMENTS: u64 = 0x1941_A469;
const ID_CLUSTER: u64 = 0x1F43_B675;
const ID_TRACK_ENTRY: u64 = 0xAE;
const ID_TRACK_NUMBER: u64 = 0xD7;
const ID_TRACK_TYPE: u64 = 0x83;
const ID_CODEC_ID: u64 = 0x86;
const ID_SIMPLE_BLOCK: u64 = 0xA3;
const ID_BLOCK_GROUP: u64 = 0xA0;
const ID_BLOCK: u64 = 0xA1;
const TRACK_TYPE_SUBTITLE: u64 = 0x11;

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

/// Extract the text subtitle tracks from a Matroska/WebM stream. Walks the EBML tree reading only
/// subtitle blocks — small payloads streamed through, large ones seeked past (see [`skip`]) — so
/// cost tracks the subtitle track's size, not the file's. Assumes the standard layout where
/// `Tracks` precedes the `Cluster`s. Give it a *buffered* reader: the traversal does many small reads.
fn matroska_subtitles<R: BufRead + Seek>(mut r: R) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    // Top level: find the Segment, skipping the EBML header (and anything else).
    let seg_size = loop {
        match read_element(&mut r)? {
            None => return Ok(out),
            Some((ID_SEGMENT, size)) => break size,
            Some((_, Some(size))) => skip(&mut r, size)?,
            Some((_, None)) => return Ok(out), // unknown-size element before the Segment: give up
        }
    };
    let seg_end = seg_size.map_or(u64::MAX, |s| r.stream_position().unwrap_or(0) + s);

    let mut subs: Vec<u64> = Vec::new();
    while r.stream_position()? < seg_end {
        let Some((id, size)) = read_element(&mut r)? else { break };
        match id {
            ID_TRACKS => {
                let Some(size) = size else { break };
                let mut buf = vec![0u8; size as usize];
                r.read_exact(&mut buf)?;
                subs = subtitle_track_numbers(&buf);
                if subs.is_empty() {
                    break; // no text subtitle track → nothing to extract
                }
            }
            ID_CLUSTER => read_container(&mut r, size, &subs, &mut out)?,
            _ => match size {
                Some(s) => skip(&mut r, s)?,
                None => break,
            },
        }
    }
    Ok(out)
}

/// The track numbers of the text-subtitle tracks (`TrackType` = subtitle, `CodecID` = `S_TEXT/*`)
/// in a `Tracks` element's body. Image subtitles (PGS/VobSub) carry no text and are ignored.
fn subtitle_track_numbers(tracks: &[u8]) -> Vec<u64> {
    let mut subs = Vec::new();
    let mut cursor = io::Cursor::new(tracks);
    while let Ok(Some((id, size))) = read_element(&mut cursor) {
        let start = cursor.position() as usize;
        let end = start.saturating_add(size.unwrap_or(0) as usize).min(tracks.len());
        if id == ID_TRACK_ENTRY {
            if let Some(number) = subtitle_track(&tracks[start..end]) {
                subs.push(number);
            }
        }
        cursor.set_position(end as u64);
    }
    subs
}

/// The track number of a `TrackEntry`, if it's a text subtitle track.
fn subtitle_track(entry: &[u8]) -> Option<u64> {
    let (mut number, mut is_subtitle, mut is_text) = (None, false, false);
    let mut cursor = io::Cursor::new(entry);
    while let Ok(Some((id, size))) = read_element(&mut cursor) {
        let start = cursor.position() as usize;
        let end = start.saturating_add(size.unwrap_or(0) as usize).min(entry.len());
        let data = entry.get(start..end)?;
        match id {
            ID_TRACK_NUMBER => number = Some(read_uint(data)),
            ID_TRACK_TYPE => is_subtitle = read_uint(data) == TRACK_TYPE_SUBTITLE,
            ID_CODEC_ID => is_text = std::str::from_utf8(data).is_ok_and(|c| c.starts_with("S_TEXT")),
            _ => {}
        }
        cursor.set_position(end as u64);
    }
    (is_subtitle && is_text).then_some(number?)
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

/// Walk the children of a Cluster (or BlockGroup), extracting subtitle blocks and advancing past
/// everything else via [`skip`]. Buffered reads keep the per-block header/track parsing cheap.
/// `size` `None` = an unknown-size Cluster: stop when a Segment-level sibling appears (rewind so the
/// caller re-reads it) or at EOF.
fn read_container<R: BufRead + Seek>(
    r: &mut R,
    size: Option<u64>,
    subs: &[u64],
    out: &mut Vec<u8>,
) -> io::Result<()> {
    let end = size.map(|s| r.stream_position().unwrap_or(0) + s);
    loop {
        let before = r.stream_position()?;
        if end.is_some_and(|e| before >= e) {
            break;
        }
        let Some((id, esize)) = read_element(r)? else { break };
        match id {
            ID_SIMPLE_BLOCK | ID_BLOCK => {
                let Some(block_size) = esize else { break };
                let Some((_, track, track_len)) = read_vint(r)? else { break };
                let rest = block_size.saturating_sub(u64::from(track_len));
                if subs.contains(&track) {
                    read_subtitle_frame(r, rest, out)?;
                } else {
                    skip(r, rest)?;
                }
            }
            ID_BLOCK_GROUP => read_container(r, esize, subs, out)?,
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

/// Read a subtitle block's remaining bytes (`[int16 ts][flags][frame text]`, the track vint already
/// consumed) and append the text. Only called for subtitle blocks, so reading them in full is fine.
fn read_subtitle_frame<R: Read>(r: &mut R, rest: u64, out: &mut Vec<u8>) -> io::Result<()> {
    let mut buf = vec![0u8; rest as usize];
    r.read_exact(&mut buf)?;
    // lacing bits (flags & 0x06) == 0 means a single plain frame; buf[3..] is the text.
    if buf.len() > 3 && buf[2] & 0x06 == 0 {
        if let Ok(text) = std::str::from_utf8(&buf[3..]) {
            out.extend_from_slice(text.as_bytes());
            out.push(b'\n');
        }
    }
    Ok(())
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
        let out = bencode_text(b"d4:name5:hello4:spamli42e3:fooee");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("name") && text.contains("hello") && text.contains("foo"));
        assert!(!text.contains("42")); // integers aren't strings
    }

    #[test]
    fn bencode_skips_the_binary_pieces_blob() {
        // A byte-string whose bytes aren't UTF-8 text (NUL etc.) is dropped.
        let out = bencode_text(b"d6:pieces4:\x00\x01\x02\x03e");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("pieces")); // the key is text
        assert!(!text.contains('\u{0}')); // the blob value is not emitted
    }

    #[test]
    fn matroska_extracts_a_text_subtitle_block() {
        // A minimal Matroska: Segment { Tracks { TrackEntry(#1, subtitle, S_TEXT/UTF8) },
        // Cluster { SimpleBlock(track 1, "hi there") } }. All sizes < 128 → 1-byte size vints.
        fn elem(id: &[u8], data: &[u8]) -> Vec<u8> {
            let mut v = id.to_vec();
            v.push(0x80 | u8::try_from(data.len()).unwrap()); // 1-byte EBML size vint
            v.extend_from_slice(data);
            v
        }
        let entry =
            [elem(&[0xD7], &[0x01]), elem(&[0x83], &[0x11]), elem(&[0x86], b"S_TEXT/UTF8")].concat();
        let tracks = elem(&[0x16, 0x54, 0xAE, 0x6B], &elem(&[0xAE], &entry));
        let mut block = vec![0x81, 0x00, 0x00, 0x00]; // track-1 vint, int16 timestamp, flags
        block.extend_from_slice(b"hi there");
        let cluster = elem(&[0x1F, 0x43, 0xB6, 0x75], &elem(&[0xA3], &block));
        let segment = elem(&[0x18, 0x53, 0x80, 0x67], &[tracks, cluster].concat());

        let out = matroska_subtitles(io::Cursor::new(segment)).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "hi there\n");
    }

    #[test]
    #[ignore = "reads the local multi-GB videofile.mkv test fixture; run explicitly"]
    fn matroska_extracts_subtitles_from_the_test_file() {
        let t0 = std::time::Instant::now();
        let text = matroska_subtitles(io::BufReader::new(File::open("videofile.mkv").unwrap())).unwrap();
        let text = String::from_utf8_lossy(&text);
        eprintln!("extracted {} bytes of subtitles in {:?}", text.len(), t0.elapsed());
        assert!(text.contains("So don't take it personally"));
    }
}
