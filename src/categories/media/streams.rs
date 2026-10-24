//! What a media file holds, stream by stream, as ffprobe reports it ([`Stream`], [`_streams`]) —
//! and the rules for the cover pictures among them in Matroska, whose muxer makes a real video track
//! of a cover passed through it: each is taken out to a file and attached again ([`Cover`]). Shared
//! by the media commands that rewrite a file: the trim ([`super::transcode`]) and the correction
//! ([`super::correction`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::_run_ffmpeg;
use crate::support::exec::capture_stdout;
use crate::tools;

/// One stream of a media file, as ffprobe reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stream {
    pub(super) index: usize,
    /// ffprobe's `codec_type`: `video`, `audio`, `subtitle`, `attachment`, `data`.
    pub(super) kind: String,
    /// The codec's name — empty where ffprobe gives none, as for some attachments.
    pub(super) codec: String,
    /// Marked a cover picture: ffprobe's `attached_pic` disposition.
    pub(super) attached: bool,
    /// A cover picture riding as a video stream: attached — or a cover an earlier remux demoted to
    /// a plain video track, still tagged with the image type it was attached as (an `image/…`
    /// mimetype, which only an attachment is given).
    pub(super) cover: bool,
    /// The file name and mimetype a cover carried as a Matroska attachment, where it did.
    pub(super) filename: Option<String>,
    pub(super) mimetype: Option<String>,
}

/// `input`'s streams, via ffprobe — `None` when unprobeable (callers treat that as the cautious
/// answer: don't stream-copy, leave the stream choice to ffmpeg — never as an error).
pub(super) fn _streams(input: &Path) -> Option<Vec<Stream>> {
    let mut argv: Vec<OsString> =
        ["-v", "error", "-show_entries", "stream=index,codec_type,codec_name:stream_disposition=attached_pic:stream_tags=filename,mimetype", "-of", "json"]
            .into_iter()
            .map(OsString::from)
            .collect();
    argv.push(input.as_os_str().to_owned());
    _parse_streams(&capture_stdout(tools::resolve("ffprobe"), argv)?)
}

/// The tag `name` of a stream in ffprobe's JSON, matched whatever its case: an attachment's arrive
/// lowercase (`filename`), the same tags on a track as Matroska stores them, uppercase
/// (`FILENAME`).
fn _tag<'a>(stream: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    stream["tags"].as_object()?.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))?.1.as_str()
}

/// ffprobe's JSON stream list as [`Stream`]s; `None` when it doesn't parse.
pub(super) fn _parse_streams(json: &str) -> Option<Vec<Stream>> {
    let report: serde_json::Value = serde_json::from_str(json).ok()?;
    report["streams"]
        .as_array()?
        .iter()
        .map(|stream| {
            let attached = stream["disposition"]["attached_pic"].as_u64() == Some(1);
            Some(Stream {
                index: usize::try_from(stream["index"].as_u64()?).ok()?,
                kind: stream["codec_type"].as_str().unwrap_or_default().to_string(),
                codec: stream["codec_name"].as_str().unwrap_or_default().to_string(),
                attached,
                cover: attached
                    || (stream["codec_type"] == "video"
                        && _tag(stream, "mimetype").is_some_and(|mimetype| mimetype.starts_with("image/"))),
                filename: _tag(stream, "filename").map(str::to_string),
                mimetype: _tag(stream, "mimetype").map(str::to_string),
            })
        })
        .collect()
}

/// The containers that keep a cover picture only as an attachment: Matroska's, whose muxer writes a
/// mapped cover as a real video track, whatever disposition it is given (ffmpeg n9.0) — which
/// players then list as one more video, and thumbnailers ignore. A command writing one takes each
/// cover out and attaches it again, as `dl` does when it remuxes. WebM is no member: it holds no
/// attachments at all.
const ATTACHING: &[&str] = &["mkv", "mka", "mk3d"];

/// Whether `output`'s container keeps covers only as attachments ([`ATTACHING`]), by extension.
pub(super) fn _attaches_covers(output: &Path) -> bool {
    output
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| ATTACHING.contains(&ext.as_str()))
}

/// The cover pictures Matroska keeps — the image types its demuxer hands back as cover streams — by
/// codec: the extension one is taken out to, and the mimetype it is attached under where it carried
/// none.
const COVER_IMAGES: &[(&str, &str, &str)] = &[
    ("mjpeg", "jpg", "image/jpeg"),
    ("png", "png", "image/png"),
    ("gif", "gif", "image/gif"),
    ("tiff", "tiff", "image/tiff"),
];

/// A cover picture taken out of a file, to be attached to another again: the file it is taken out
/// to, and the name and type it is attached under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Cover {
    pub(super) file: PathBuf,
    pub(super) filename: String,
    pub(super) mimetype: String,
}

/// How `stream`, if it is a picture in an image type Matroska keeps as a cover ([`COVER_IMAGES`]),
/// is attached again: taken out into `dir`, and attached under the name and type it carried —
/// `cover.<ext>` and its image's own type where it carried none.
pub(super) fn _cover(stream: &Stream, dir: &Path) -> Option<Cover> {
    let &(_, ext, mimetype) = COVER_IMAGES.iter().find(|(codec, ..)| *codec == stream.codec)?;
    Some(Cover {
        file: dir.join(format!("cover-{}.{ext}", stream.index)),
        filename: stream.filename.clone().unwrap_or_else(|| format!("cover.{ext}")),
        mimetype: stream.mimetype.clone().unwrap_or_else(|| mimetype.to_string()),
    })
}

/// Take stream `index` of `input` — a cover — out to `cover`'s file: its one packet, copied. Whether
/// it came out.
pub(super) fn _take_out(input: &Path, index: usize, cover: &Cover) -> bool {
    cover.file.parent().is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
        && _run_ffmpeg(_take_out_argv(input, index, &cover.file)) == 0
        && cover.file.is_file()
}

/// ffmpeg argv taking stream `index` out of `input` to `file`: its one packet, copied, as a single
/// image (`-update 1`, or ffmpeg warns of a missing `%03d` pattern).
fn _take_out_argv(input: &Path, index: usize, file: &Path) -> Vec<OsString> {
    let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
    argv.push(input.as_os_str().to_owned());
    argv.extend(["-map".to_string(), format!("0:{index}")].map(OsString::from));
    argv.extend(["-frames:v", "1", "-update", "1", "-c", "copy"].map(OsString::from));
    argv.push(file.as_os_str().to_owned());
    argv
}

/// The options attaching `covers` to an output after every mapped stream, each under its name and
/// type: ffmpeg numbers attachments in output order, so the first is the output's attachment
/// number `mapped` — the attachments the mapped streams bring come first.
pub(super) fn _attach_argv(covers: &[&Cover], mapped: usize) -> Vec<OsString> {
    let mut argv: Vec<OsString> = Vec::new();
    for (number, cover) in covers.iter().enumerate() {
        let tag = format!("-metadata:s:t:{}", mapped + number);
        argv.extend(["-attach".into(), cover.file.as_os_str().to_owned()]);
        argv.extend(
            [tag.clone(), format!("mimetype={}", cover.mimetype), tag, format!("filename={}", cover.filename)]
                .map(OsString::from),
        );
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::categories::media::strs;

    fn stream(index: usize, kind: &str, codec: &str) -> Stream {
        Stream { index, kind: kind.to_string(), codec: codec.to_string(), attached: false, cover: false, filename: None, mimetype: None }
    }

    /// Each stream as ffprobe lists it: a missing codec name is an empty one; a cover is told by its
    /// disposition — or, demoted to a plain track by an earlier remux, by the image type it is still
    /// tagged with, the tags then in a track's uppercase.
    #[test]
    fn ffprobes_stream_list_reads_as_streams() {
        let json = r#"{"streams": [
            {"index": 0, "codec_name": "av1", "codec_type": "video", "disposition": {"attached_pic": 0}},
            {"index": 1, "codec_name": "mjpeg", "codec_type": "video", "disposition": {"attached_pic": 1},
             "tags": {"filename": "cover.jpg", "mimetype": "image/jpeg"}},
            {"index": 2, "codec_type": "attachment", "disposition": {"attached_pic": 0},
             "tags": {"filename": "font.ttf", "mimetype": "application/x-truetype-font"}},
            {"index": 3, "codec_name": "mjpeg", "codec_type": "video", "disposition": {"attached_pic": 0},
             "tags": {"FILENAME": "cover.jpg", "MIMETYPE": "image/jpeg", "DURATION": "00:00:00.000000000"}}
        ]}"#;
        let streams = _parse_streams(json).expect("parses");
        assert_eq!(streams[0], stream(0, "video", "av1"));
        let tagged = |stream: Stream, filename: &str, mimetype: &str| Stream {
            filename: Some(filename.to_string()),
            mimetype: Some(mimetype.to_string()),
            ..stream
        };
        assert_eq!(streams[1], tagged(Stream { attached: true, cover: true, ..stream(1, "video", "mjpeg") }, "cover.jpg", "image/jpeg"));
        assert_eq!(streams[2], tagged(stream(2, "attachment", ""), "font.ttf", "application/x-truetype-font"));
        assert_eq!(streams[3], tagged(Stream { cover: true, ..stream(3, "video", "mjpeg") }, "cover.jpg", "image/jpeg"), "demoted");
        assert!(_parse_streams("not json").is_none());
    }

    /// The containers that attach covers are Matroska's, whatever the extension's case.
    #[test]
    fn matroska_outputs_attach_their_covers() {
        for name in ["a.mkv", "b.MKA", "c.mk3d"] {
            assert!(_attaches_covers(Path::new(name)), "{name}");
        }
        for name in ["a.webm", "b.mp4", "c.mov", "noext"] {
            assert!(!_attaches_covers(Path::new(name)), "{name}");
        }
    }

    /// A cover is attached again under the name and type it carried — `cover.<ext>` and its image's
    /// own type where it carried none — and only in an image type Matroska keeps.
    #[test]
    fn a_cover_is_attached_again_as_it_was_attached() {
        let dir = Path::new("/tmp/covers");
        assert_eq!(
            _cover(&stream(7, "video", "png"), dir),
            Some(Cover { file: dir.join("cover-7.png"), filename: "cover.png".to_string(), mimetype: "image/png".to_string() })
        );
        let tagged = Stream { filename: Some("cover_land.jpg".to_string()), mimetype: Some("image/jpeg".to_string()), ..stream(4, "video", "mjpeg") };
        assert_eq!(_cover(&tagged, dir).map(|cover| cover.filename), Some("cover_land.jpg".to_string()));
        assert_eq!(_cover(&stream(7, "video", "bmp"), dir), None, "no image type Matroska keeps");
        assert_eq!(
            strs(&_take_out_argv(Path::new("in.mkv"), 4, &dir.join("cover-4.jpg"))),
            ["-v", "error", "-y", "-i", "in.mkv", "-map", "0:4", "-frames:v", "1", "-update", "1", "-c", "copy", "/tmp/covers/cover-4.jpg"]
        );
    }

    /// Covers are attached after the attachments the mapped streams bring, each under its own name
    /// and type.
    #[test]
    fn covers_are_attached_after_the_mapped_attachments() {
        let cover = |file: &str, filename: &str| Cover {
            file: PathBuf::from(file),
            filename: filename.to_string(),
            mimetype: "image/jpeg".to_string(),
        };
        let (one, two) = (cover("/t/a.jpg", "cover.jpg"), cover("/t/b.jpg", "cover-2.jpg"));
        assert_eq!(
            strs(&_attach_argv(&[&one, &two], 1)),
            [
                "-attach", "/t/a.jpg", "-metadata:s:t:1", "mimetype=image/jpeg", "-metadata:s:t:1", "filename=cover.jpg",
                "-attach", "/t/b.jpg", "-metadata:s:t:2", "mimetype=image/jpeg", "-metadata:s:t:2", "filename=cover-2.jpg",
            ]
        );
        assert!(_attach_argv(&[], 3).is_empty());
    }
}
