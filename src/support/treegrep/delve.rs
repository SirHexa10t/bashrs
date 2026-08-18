//! Binary-format decoders behind `gg --delve`: pull searchable text out of files `gg` would
//! otherwise skip as binary, by understanding their structure rather than scanning raw bytes. A
//! private child of [`super`] (treegrep) — the one module that calls it, from `scan_file`'s delve
//! branch; `pub(super)` keeps it that way by construction. One decoder per format; [`extract`]
//! dispatches by extension, so adding a format is a decoder plus a dispatch arm. Files with no
//! decoder return `None` and stay skipped (no raw `strings`-style fallback — that's the noisy,
//! slow approach `gg` deliberately avoids).
//!
//! - `.torrent` — bencode: the human-readable string values (name, file paths, tracker URLs),
//!   skipping the binary `pieces` SHA-1 blob.
//! - `.mkv`/`.mka`/`.mks`/`.webm` — Matroska: the text subtitle tracks (`S_TEXT/*`), extracted by
//!   walking the EBML tree and *seeking past* video/audio payloads — so a multi-GB file is read in
//!   milliseconds (only the tiny subtitle blocks are actually read), not in full.
//! - `.mp4`/`.m4v`/`.mov` — ISO-BMFF: text subtitle tracks (tx3g/QuickTime text, WebVTT, TTML),
//!   located via the `moov` sample tables (progressive) or `moof`/`traf`/`trun` fragments
//!   (fragmented) and read from `mdat`.

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
        "mkv" | "mka" | "mks" | "webm" => Some(
            File::open(path)
                .map(|f| io::BufReader::with_capacity(128 * 1024, f))
                .ok()
                .and_then(|r| matroska_subtitles(r).ok())
                .unwrap_or_default(),
        ),
        "mp4" | "m4v" | "mov" => {
            Some(File::open(path).ok().and_then(|f| mp4_subtitles(f).ok()).unwrap_or_default())
        }
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

// ---- MP4 / ISO-BMFF (.mp4/.m4v/.mov) --------------------------------------------------------

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

/// Extract text from every text/subtitle track of an ISO-BMFF (MP4/MOV) file. Reads each `trak`'s
/// codec + sample tables and pulls its samples from `mdat` (progressive files); then, for
/// *fragmented* files (where `moov`'s tables are empty), also picks up samples from the `moof`
/// fragments across the file. Only the (tiny, few) subtitle samples are read — never the media data.
fn mp4_subtitles<R: Read + Seek>(mut r: R) -> io::Result<Vec<u8>> {
    let file_end = r.seek(SeekFrom::End(0))?;
    let mut out = Vec::new();
    let Some((moov_start, moov_end)) = find_box(&mut r, 0, file_end, b"moov")? else {
        return Ok(out);
    };
    // Read each text track: extract its progressive samples now, and remember its `(track_id,
    // codec)` so its samples can also be found in any movie fragments below.
    let mut text_tracks: Vec<(u32, [u8; 4])> = Vec::new();
    let mut pos = moov_start;
    while let Some((fourcc, cs, ce, next)) = next_box(&mut r, pos, moov_end)? {
        if &fourcc == b"trak" {
            if let Some(track) = read_trak(&mut r, cs, ce, &mut out)? {
                text_tracks.push(track);
            }
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    // Fragmented MP4: samples live in `moof` fragments across the file, not the `moov` tables. This
    // top-level scan seeks past `mdat` (never reads it), so it costs nothing for progressive files.
    if !text_tracks.is_empty() {
        let mut pos = 0;
        while let Some((fourcc, cs, ce, next)) = next_box(&mut r, pos, file_end)? {
            if &fourcc == b"moof" {
                read_moof(&mut r, pos, cs, ce, &text_tracks, &mut out)?;
            }
            if next <= pos {
                break;
            }
            pos = next;
        }
    }
    Ok(out)
}

/// If this `trak` is a text/subtitle track, extract its progressive sample text and return its
/// `(track_id, codec)` so fragments for the same track can be matched later. `None` otherwise.
fn read_trak<R: Read + Seek>(r: &mut R, start: u64, end: u64, out: &mut Vec<u8>) -> io::Result<Option<(u32, [u8; 4])>> {
    let Some((mdia_s, mdia_e)) = find_box(r, start, end, b"mdia")? else { return Ok(None) };
    if !is_text_handler(r, mdia_s, mdia_e)? {
        return Ok(None);
    }
    let track_id = read_track_id(r, start, end)?;
    let Some((minf_s, minf_e)) = find_box(r, mdia_s, mdia_e, b"minf")? else { return Ok(None) };
    let Some((stbl_s, stbl_e)) = find_box(r, minf_s, minf_e, b"stbl")? else { return Ok(None) };
    let codec = stsd_codec(r, stbl_s, stbl_e)?;
    let sizes = read_sample_sizes(r, stbl_s, stbl_e)?;
    let chunks = read_chunk_offsets(r, stbl_s, stbl_e)?;
    let stsc = read_sample_to_chunk(r, stbl_s, stbl_e)?;
    extract_samples(r, &codec, &sizes, &chunks, &stsc, out)?;
    Ok(Some((track_id, codec)))
}

/// The track's numeric ID, from `tkhd`.
fn read_track_id<R: Read + Seek>(r: &mut R, trak_s: u64, trak_e: u64) -> io::Result<u32> {
    let Some((s, _)) = find_box(r, trak_s, trak_e, b"tkhd")? else { return Ok(0) };
    let Some(vf) = read_at::<4, _>(r, s)? else { return Ok(0) };
    // track_ID follows [version+flags] and the creation/modification times (8 bytes at v0, 16 at v1).
    let Some(id) = read_at::<4, _>(r, s + 4 + if vf[0] == 1 { 16 } else { 8 })? else { return Ok(0) };
    Ok(u32::from_be_bytes(id))
}

/// Walk a `moof`'s `traf` boxes, extracting subtitle samples for known text tracks.
fn read_moof<R: Read + Seek>(r: &mut R, moof_start: u64, cs: u64, ce: u64, tracks: &[(u32, [u8; 4])], out: &mut Vec<u8>) -> io::Result<()> {
    let mut pos = cs;
    while let Some((fourcc, tcs, tce, next)) = next_box(r, pos, ce)? {
        if &fourcc == b"traf" {
            read_traf(r, moof_start, tcs, tce, tracks, out)?;
        }
        if next <= pos {
            break;
        }
        pos = next;
    }
    Ok(())
}

/// A `traf`: read its `tfhd` (track id + defaults) and `trun` (sample run); if it belongs to a text
/// track, read and decode its samples from where the fragment's data lives.
fn read_traf<R: Read + Seek>(r: &mut R, moof_start: u64, tcs: u64, tce: u64, tracks: &[(u32, [u8; 4])], out: &mut Vec<u8>) -> io::Result<()> {
    let Some((tfhd_s, _)) = find_box(r, tcs, tce, b"tfhd")? else { return Ok(()) };
    let (track_id, base, default_size) = read_tfhd(r, moof_start, tfhd_s)?;
    let Some(&(_, codec)) = tracks.iter().find(|(id, _)| *id == track_id) else { return Ok(()) };
    let Some((trun_s, trun_e)) = find_box(r, tcs, tce, b"trun")? else { return Ok(()) };
    let (data_offset, sizes) = read_trun(r, trun_s, trun_e, default_size)?;
    let mut off = base.wrapping_add(data_offset as u64);
    for size in sizes {
        if size <= 256 * 1024 {
            r.seek(SeekFrom::Start(off))?;
            let mut buf = vec![0u8; size as usize];
            if r.read_exact(&mut buf).is_err() {
                return Ok(());
            }
            decode_sample(&codec, &buf, out);
        }
        off += size;
    }
    Ok(())
}

/// Parse a `tfhd`: `(track_id, base_data_offset, default_sample_size)`. The base defaults to the
/// enclosing `moof` (the common "default-base-is-moof" case) unless an explicit base is present.
fn read_tfhd<R: Read + Seek>(r: &mut R, moof_start: u64, s: u64) -> io::Result<(u32, u64, u64)> {
    // head = [version+flags][track_id]
    let Some(head) = read_at::<8, _>(r, s)? else { return Ok((0, moof_start, 0)) };
    let flags = u32::from_be_bytes([0, head[1], head[2], head[3]]);
    let track_id = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
    let (mut base, mut default_size) = (moof_start, 0u64);
    if flags & 0x00_0001 != 0 {
        let mut b = [0u8; 8]; // base_data_offset
        r.read_exact(&mut b)?;
        base = u64::from_be_bytes(b);
    }
    if flags & 0x00_0002 != 0 {
        skip_u32(r)?; // sample_description_index
    }
    if flags & 0x00_0008 != 0 {
        skip_u32(r)?; // default_sample_duration
    }
    if flags & 0x00_0010 != 0 {
        let mut b = [0u8; 4]; // default_sample_size
        r.read_exact(&mut b)?;
        default_size = u64::from(u32::from_be_bytes(b));
    }
    Ok((track_id, base, default_size))
}

/// Parse a `trun`: `(data_offset, per-sample sizes)`. Sizes come from the run, or fall back to
/// `default_size`; `data_offset` is relative to the fragment's base.
fn read_trun<R: Read + Seek>(r: &mut R, s: u64, e: u64, default_size: u64) -> io::Result<(i64, Vec<u64>)> {
    // head = [version+flags][sample_count]
    let Some(head) = read_at::<8, _>(r, s)? else { return Ok((0, Vec::new())) };
    let flags = u32::from_be_bytes([0, head[1], head[2], head[3]]);
    let sample_count = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let mut data_offset = 0i64;
    if flags & 0x00_0001 != 0 {
        let mut b = [0u8; 4]; // data_offset (i32)
        r.read_exact(&mut b)?;
        data_offset = i64::from(i32::from_be_bytes(b));
    }
    if flags & 0x00_0004 != 0 {
        skip_u32(r)?; // first_sample_flags
    }
    let (has_dur, has_size) = (flags & 0x00_0100 != 0, flags & 0x00_0200 != 0);
    let (has_flags, has_ctime) = (flags & 0x00_0400 != 0, flags & 0x00_0800 != 0);
    let per_sample = 4 * (u64::from(has_dur) + u64::from(has_size) + u64::from(has_flags) + u64::from(has_ctime));
    // Bound the count by what fits in the box (and a ceiling) so a bogus count can't run away.
    let avail = e.saturating_sub(r.stream_position()?);
    let room = avail.checked_div(per_sample).map_or(sample_count, |n| n as usize);
    let count = sample_count.min(room).min(1 << 16);
    let mut sizes = Vec::with_capacity(count);
    for _ in 0..count {
        if has_dur {
            skip_u32(r)?;
        }
        let size = if has_size {
            let mut b = [0u8; 4];
            r.read_exact(&mut b)?;
            u64::from(u32::from_be_bytes(b))
        } else {
            default_size
        };
        if has_flags {
            skip_u32(r)?;
        }
        if has_ctime {
            skip_u32(r)?;
        }
        sizes.push(size);
    }
    Ok((data_offset, sizes))
}

/// Read and discard a big-endian `u32`.
fn skip_u32<R: Read>(r: &mut R) -> io::Result<()> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)
}

/// Whether the track's `hdlr` names a text/subtitle handler (`text`/`sbtl`/`subt`).
fn is_text_handler<R: Read + Seek>(r: &mut R, mdia_s: u64, mdia_e: u64) -> io::Result<bool> {
    let Some((s, e)) = find_box(r, mdia_s, mdia_e, b"hdlr")? else { return Ok(false) };
    if e - s < 12 {
        return Ok(false);
    }
    // b = [version+flags][pre_defined][handler_type]
    let Some(b) = read_at::<12, _>(r, s)? else { return Ok(false) };
    Ok(matches!(&b[8..12], b"text" | b"sbtl" | b"subt"))
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

/// Reconstruct each sample's file offset (its chunk's offset plus the preceding samples in that
/// chunk), read it, and decode its text. Skips implausibly large samples (not subtitle text).
fn extract_samples<R: Read + Seek>(
    r: &mut R,
    codec: &[u8; 4],
    sizes: &[u64],
    chunks: &[u64],
    stsc: &[(u32, u32)],
    out: &mut Vec<u8>,
) -> io::Result<()> {
    let mut sample = 0usize;
    for (i, &chunk_off) in chunks.iter().enumerate() {
        let chunk = i as u32 + 1; // chunks are 1-based in `stsc`
        let per_chunk = stsc.iter().rev().find(|(first, _)| *first <= chunk).map_or(0, |&(_, n)| n);
        let mut off = chunk_off;
        for _ in 0..per_chunk {
            let Some(&size) = sizes.get(sample) else { return Ok(()) };
            sample += 1;
            if size <= 256 * 1024 {
                r.seek(SeekFrom::Start(off))?;
                let mut buf = vec![0u8; size as usize];
                if r.read_exact(&mut buf).is_err() {
                    return Ok(());
                }
                decode_sample(codec, &buf, out);
            }
            off += size;
        }
    }
    Ok(())
}

/// Decode one subtitle sample's text, per codec.
fn decode_sample(codec: &[u8; 4], sample: &[u8], out: &mut Vec<u8>) {
    match codec {
        // tx3g / QuickTime text: `[u16 text length][UTF-8 text][optional style boxes]`.
        b"tx3g" | b"text" => {
            let len = sample.get(..2).map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
            if let Some(text) = sample.get(2..2 + len) {
                push_text(out, text);
            }
        }
        // WebVTT: cue boxes; the visible text lives in `payl` boxes.
        b"wvtt" => push_wvtt_payloads(sample, out),
        // TTML (`stpp`) and anything else text-ish: take the whole (UTF-8) sample.
        _ => push_text(out, sample),
    }
}

/// Append `bytes` as a trimmed line, if it's non-empty UTF-8.
fn push_text(out: &mut Vec<u8>, bytes: &[u8]) {
    if let Ok(text) = std::str::from_utf8(bytes) {
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
    fn mp4_extracts_a_tx3g_sample() {
        fn mp4box(fourcc: &[u8; 4], content: &[u8]) -> Vec<u8> {
            let mut v = (u32::try_from(8 + content.len()).unwrap()).to_be_bytes().to_vec();
            v.extend_from_slice(fourcc);
            v.extend_from_slice(content);
            v
        }
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
        let out = mp4_subtitles(io::Cursor::new([mdat, moov].concat())).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "hi mp4\n");
    }

    #[test]
    fn mp4_extracts_a_fragmented_tx3g_sample() {
        fn mp4box(fourcc: &[u8; 4], content: &[u8]) -> Vec<u8> {
            let mut v = u32::try_from(8 + content.len()).unwrap().to_be_bytes().to_vec();
            v.extend_from_slice(fourcc);
            v.extend_from_slice(content);
            v
        }
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
        let out = mp4_subtitles(io::Cursor::new([mdat, moov, moof].concat())).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "hi frag\n");
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
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sintel-wvtt.mp4"));
        let text = String::from_utf8(mp4_subtitles(io::Cursor::new(&bytes[..])).unwrap()).unwrap();
        assert!(text.contains("This blade has a dark past."), "fragment 1 cue missing: {text:?}");
        assert!(text.contains("It has shed much innocent blood."), "fragment 2 cue missing: {text:?}");
    }

    #[test]
    fn mp4_extracts_fragmented_stpp_ttml() {
        // A real fragmented TTML stream (Tears of Steel). The source segment's sample was an empty
        // `<tt/>`, so a real cue was spliced into its `mdat` for a meaningful assertion; `stpp`
        // decoding returns the whole TTML document, so we match the cue text within it.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tears-of-steel-stpp.mp4"));
        let text = String::from_utf8(mp4_subtitles(io::Cursor::new(&bytes[..])).unwrap()).unwrap();
        assert!(text.contains("A storm approaches."), "stpp cue missing: {text:?}");
    }

    #[test]
    fn matroska_extracts_a_text_subtitle_from_a_real_file() {
        // A ~120-byte slice of a real Matroska file — its EBML header, `S_TEXT/UTF8` subtitle track,
        // and one real cue — reconstructed from a 9.77 GB source with the bulk A/V dropped.
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phm.mkv"));
        let text = matroska_subtitles(io::Cursor::new(&bytes[..])).unwrap();
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("So don't take it personally"), "{text:?}");
    }
}
