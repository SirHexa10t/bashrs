//! Correct how a media file stores what it holds, rewriting it in place — so far one correction: a
//! picture riding as a plain video track in a video, which players list as one more video and
//! thumbnailers pass over (the cover an earlier remux demoted, or one never attached as a cover),
//! made the file's thumbnail.

#[bashrs_macros::category(command = MediaCorrectionCommand, prefix = "media_")]
mod commands {
    use crate::categories::media::_run_ffmpeg;
    use crate::categories::media::streams::{_attach_argv, _attaches_covers, _cover, _streams, _take_out, Cover, Stream};
    use crate::support::exec::capture_stdout;
    use crate::tools;
    use clap::Args;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    // --- media_correction -----------------------------------------------------

    /// Correct video files in place: a picture riding as one more video track becomes the file's
    /// thumbnail
    pub fn correction(args: CorrectionArgs) {
        let mut failed = false;
        for file in &args.files {
            match _correct(file) {
                Ok(done) => println!("{}: {done}", file.display()),
                Err(why) => {
                    eprintln!("media_correction: {}: {why}", file.display());
                    failed = true;
                }
            }
        }
        if failed {
            std::process::exit(1);
        }
    }

    #[derive(Args)]
    pub struct CorrectionArgs {
        /// Video file(s) to correct, in place
        #[arg(required = true, value_name = "FILE")]
        pub files: Vec<PathBuf>,
    }

    /// The containers that keep a cover as a mapped stream marked `attached_pic` (an mp4's `covr`),
    /// where a picture track is corrected by marking it so. Matroska keeps covers as attachments
    /// instead (`streams::_attaches_covers`); anything else holds no cover art at all.
    const MARKING: &[&str] = &["mp4", "m4v", "mov"];

    /// How a file's container keeps a cover, if it keeps one.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Keeps {
        /// As an attachment (Matroska): each picture is taken out and attached again.
        Attaching,
        /// As a mapped stream marked `attached_pic` (the mp4 family).
        Marking,
    }

    /// How `file`'s container keeps a cover, by its extension — `None` for one that holds no cover
    /// art (WebM, AVI, …).
    fn _keeps(file: &Path) -> Option<Keeps> {
        if _attaches_covers(file) {
            return Some(Keeps::Attaching);
        }
        let ext = file.extension()?.to_string_lossy().to_lowercase();
        MARKING.contains(&ext.as_str()).then_some(Keeps::Marking)
    }

    /// The pictures riding as plain video tracks in a video, among `streams` with `packets` each: a
    /// video stream not marked a cover, holding a single packet, in an image type a cover is kept in
    /// — and only where some other video stream is a moving picture, so a file that is a picture
    /// alone is none of this. In stream order.
    fn _picture_tracks<'a>(streams: &'a [Stream], packets: &[(usize, u64)], dir: &Path) -> Vec<&'a Stream> {
        let single = |stream: &Stream| packets.iter().any(|&(index, count)| index == stream.index && count == 1);
        let picture = |stream: &&Stream| {
            stream.kind == "video" && !stream.attached && single(stream) && _cover(stream, dir).is_some()
        };
        let pictures: Vec<&Stream> = streams.iter().filter(picture).collect();
        let moving = streams.iter().any(|stream| stream.kind == "video" && !stream.attached && !pictures.contains(&stream));
        if moving { pictures } else { Vec::new() }
    }

    /// How many packets each of `file`'s streams holds — one read of the whole file, no decoding.
    /// `None` when ffprobe cannot read it.
    fn _packet_counts(file: &Path) -> Option<Vec<(usize, u64)>> {
        let mut argv: Vec<OsString> =
            ["-v", "error", "-count_packets", "-show_entries", "stream=index,nb_read_packets", "-of", "csv=p=0"]
                .into_iter()
                .map(OsString::from)
                .collect();
        argv.push(file.as_os_str().to_owned());
        Some(_parse_counts(&capture_stdout(tools::resolve("ffprobe"), argv)?))
    }

    /// ffprobe's `index,nb_read_packets` rows as pairs; a row that is not one is skipped.
    fn _parse_counts(csv: &str) -> Vec<(usize, u64)> {
        csv.lines()
            .filter_map(|row| {
                let (index, count) = row.trim().split_once(',')?;
                Some((index.parse().ok()?, count.parse().ok()?))
            })
            .collect()
    }

    /// What the correction does to a file's pictures, once they are taken out and compared.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Plan {
        /// Streams left out of the remux: pictures attached again, or copies of one dropped.
        excluded: Vec<usize>,
        /// The pictures attached again (Matroska) — the covers in place first, so the thumbnail shown
        /// stays the one it was — under their names and types.
        attached: Vec<Cover>,
        /// Picture tracks marked as cover art in place (the mp4 family), by stream index.
        marked: Vec<usize>,
        /// Picture tracks made the thumbnail, and copies of one removed: what is said of the file.
        restored: Vec<String>,
        removed: usize,
    }

    /// Correct `file` in place, and say what was done — or, the file left as it was, why not.
    fn _correct(file: &Path) -> Result<String, String> {
        let streams = _streams(file).ok_or("ffprobe could not read it")?;
        let packets = _packet_counts(file).ok_or("ffprobe could not read it")?;
        let covers = std::env::temp_dir().join(format!("bashrs_correction_covers_{}", std::process::id()));
        let pictures = _picture_tracks(&streams, &packets, &covers);
        if pictures.is_empty() {
            let thumbnail = streams.iter().any(|stream| stream.attached);
            return Ok(if thumbnail { "nothing to correct — its thumbnail is in place" } else { "nothing to correct" }.to_string());
        }
        let Some(keeps) = _keeps(file) else {
            let ext = file.extension().map(|ext| ext.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(format!("a picture rides in it as a video track, but a .{ext} holds no thumbnail to make it — left as it was"));
        };
        let plan = _take_out_pictures(file, &streams, &pictures, keeps, &covers);
        let result = plan.and_then(|plan| _rewrite(file, &streams, &plan, keeps).map(|()| plan));
        let _ = std::fs::remove_dir_all(&covers);
        let plan = result?;
        let mut done: Vec<String> = plan.restored.iter().map(|name| format!("its picture track is its thumbnail now ({name})")).collect();
        match plan.removed {
            0 => {}
            1 => done.push("a copy of its thumbnail riding as a track removed".to_string()),
            n => done.push(format!("{n} copies of its thumbnail riding as tracks removed")),
        }
        Ok(done.join("; "))
    }

    /// Take `file`'s pictures out — its picture tracks, and, where the container attaches covers,
    /// the covers in place, which the remux would otherwise make tracks of — and compare them: a
    /// picture already kept is a copy, dropped; any other is kept, its track restored as cover art.
    fn _take_out_pictures(file: &Path, streams: &[Stream], pictures: &[&Stream], keeps: Keeps, dir: &Path) -> Result<Plan, String> {
        let mut plan = Plan::default();
        let mut kept: Vec<Vec<u8>> = Vec::new();
        let attached: Vec<&Stream> = streams.iter().filter(|stream| stream.attached).collect();
        for stream in attached.iter().chain(pictures) {
            let is_track = !stream.attached;
            let Some(mut cover) = _cover(stream, dir) else {
                // An attached cover in an image type Matroska keeps no cover in stays mapped as it is.
                continue;
            };
            if !_take_out(file, stream.index, &cover) {
                return Err(format!("its picture (stream {}) would not come out — left as it was", stream.index));
            }
            let bytes = std::fs::read(&cover.file).map_err(|err| format!("could not read its picture back: {err}"))?;
            if kept.contains(&bytes) {
                plan.excluded.push(stream.index);
                plan.removed += 1;
                continue;
            }
            // A second, different picture must not outrank the thumbnail shown: thumbnailers prefer
            // `cover.*` — so it takes a numbered name of its own.
            if is_track && !kept.is_empty() {
                let ext = cover.file.extension().map(|ext| ext.to_string_lossy().into_owned()).unwrap_or_default();
                cover.filename = format!("cover-{}.{ext}", kept.len() + 1);
            }
            kept.push(bytes);
            match keeps {
                Keeps::Attaching => {
                    plan.excluded.push(stream.index);
                    if is_track {
                        plan.restored.push(cover.filename.clone());
                    }
                    plan.attached.push(cover);
                }
                Keeps::Marking if is_track => {
                    plan.marked.push(stream.index);
                    plan.restored.push(cover.filename.clone());
                }
                Keeps::Marking => {}
            }
        }
        Ok(plan)
    }

    /// ffmpeg argv writing `file` corrected by `plan` to `out`: every stream copied but those left
    /// out — and an mp4's data tracks (its chapter and timecode tracks, remade from its chapters and
    /// metadata, or not remuxable at all) — the pictures attached again after them, or marked as
    /// cover art where they stand.
    fn _rewrite_argv(file: &Path, streams: &[Stream], plan: &Plan, keeps: Keeps, out: &Path) -> Vec<OsString> {
        let mut argv: Vec<OsString> = ["-v", "error", "-y", "-i"].map(OsString::from).to_vec();
        argv.push(file.as_os_str().to_owned());
        argv.extend(["-map", "0"].map(OsString::from));
        let left_out = |stream: &Stream| plan.excluded.contains(&stream.index) || (keeps == Keeps::Marking && stream.kind == "data");
        for stream in streams.iter().filter(|stream| left_out(stream)) {
            argv.extend(["-map".to_string(), format!("-0:{}", stream.index)].map(OsString::from));
        }
        argv.extend(["-c", "copy"].map(OsString::from));
        let mapped: Vec<&Stream> = streams.iter().filter(|stream| !left_out(stream)).collect();
        for (at, stream) in mapped.iter().enumerate() {
            if plan.marked.contains(&stream.index) {
                argv.extend([format!("-disposition:{at}"), "attached_pic".to_string()].map(OsString::from));
            }
        }
        let attachments = mapped.iter().filter(|stream| stream.kind == "attachment").count();
        argv.extend(_attach_argv(&plan.attached.iter().collect::<Vec<_>>(), attachments));
        argv.push(out.as_os_str().to_owned());
        argv
    }

    /// Whether `corrected` holds what `plan` makes of `streams`: every stream not left out, as it was
    /// and in its order, and as many cover pictures as the plan keeps, marked as such.
    fn _checks_out(streams: &[Stream], plan: &Plan, keeps: Keeps, corrected: &[Stream]) -> bool {
        let kept_covers = streams.iter().filter(|stream| stream.attached && !plan.excluded.contains(&stream.index)).count()
            + plan.attached.len()
            + plan.marked.len();
        let carried = |stream: &&Stream| !stream.attached;
        let before: Vec<(&str, &str)> = streams
            .iter()
            .filter(|stream| !plan.excluded.contains(&stream.index) && !plan.marked.contains(&stream.index))
            .filter(|stream| !(keeps == Keeps::Marking && stream.kind == "data"))
            .filter(carried)
            .map(|stream| (stream.kind.as_str(), stream.codec.as_str()))
            .collect();
        let after: Vec<(&str, &str)> =
            corrected.iter().filter(carried).map(|stream| (stream.kind.as_str(), stream.codec.as_str())).collect();
        before == after && corrected.iter().filter(|stream| stream.attached).count() == kept_covers
    }

    /// Write `file` corrected by `plan` beside it, check the result, and put it in the original's
    /// place, with the original's permissions — or leave the original as it was, and say why.
    fn _rewrite(file: &Path, streams: &[Stream], plan: &Plan, keeps: Keeps) -> Result<(), String> {
        let ext = file.extension().map(|ext| ext.to_string_lossy().into_owned()).unwrap_or_default();
        let out = file.with_extension(format!("correcting.{ext}"));
        let written = _run_ffmpeg(_rewrite_argv(file, streams, plan, keeps, &out)) == 0;
        let checked = written && _streams(&out).is_some_and(|corrected| _checks_out(streams, plan, keeps, &corrected));
        let replaced = checked
            && std::fs::metadata(file).and_then(|meta| std::fs::set_permissions(&out, meta.permissions())).is_ok()
            && std::fs::rename(&out, file).is_ok();
        if !replaced {
            let _ = std::fs::remove_file(&out);
        }
        match (written, checked, replaced) {
            (false, ..) => Err("ffmpeg could not write it corrected — left as it was".to_string()),
            (_, false, _) => Err("the corrected file did not check out — left as it was".to_string()),
            (.., false) => Err("could not put the corrected file in its place — left as it was".to_string()),
            _ => Ok(()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::categories::media::strs;

        fn stream(index: usize, kind: &str, codec: &str) -> Stream {
            Stream { index, kind: kind.to_string(), codec: codec.to_string(), attached: false, cover: false, filename: None, mimetype: None }
        }

        fn attached(index: usize) -> Stream {
            Stream { attached: true, cover: true, filename: Some("cover.jpg".to_string()), mimetype: Some("image/jpeg".to_string()), ..stream(index, "video", "mjpeg") }
        }

        /// A picture track is a one-packet picture in a cover's image type, beside a moving video —
        /// not a cover already, not an mjpeg video of many frames, not a picture in a file of
        /// nothing else.
        #[test]
        fn a_picture_track_is_a_single_image_beside_a_moving_video() {
            let dir = Path::new("/tmp/covers");
            let streams = [stream(0, "video", "av1"), stream(1, "audio", "opus"), stream(2, "video", "mjpeg"), attached(3), stream(4, "video", "mjpeg")];
            let packets = [(0, 3880), (1, 8000), (2, 1), (3, 1), (4, 240)];
            let found: Vec<usize> = _picture_tracks(&streams, &packets, dir).iter().map(|stream| stream.index).collect();
            assert_eq!(found, [2], "the one-packet track only: not the cover, not the mjpeg video");
            let alone = [stream(0, "video", "png")];
            assert!(_picture_tracks(&alone, &[(0, 1)], dir).is_empty(), "a picture alone is no video");
            let odd = [stream(0, "video", "h264"), stream(1, "video", "bmp")];
            assert!(_picture_tracks(&odd, &[(0, 50), (1, 1)], dir).is_empty(), "no image type a cover is kept in");
            assert_eq!(_parse_counts("0,3880\n1,8000\nbroken\n2,N/A\n"), [(0, 3880), (1, 8000)]);
        }

        /// Matroska attaches covers, the mp4 family marks them, and the rest keeps none.
        #[test]
        fn each_container_keeps_a_cover_its_own_way() {
            assert_eq!(_keeps(Path::new("a.mkv")), Some(Keeps::Attaching));
            assert_eq!(_keeps(Path::new("a.MP4")), Some(Keeps::Marking));
            assert_eq!(_keeps(Path::new("a.mov")), Some(Keeps::Marking));
            assert_eq!(_keeps(Path::new("a.webm")), None);
            assert_eq!(_keeps(Path::new("a.avi")), None);
        }

        fn cover(file: &str, filename: &str) -> Cover {
            Cover { file: PathBuf::from(file), filename: filename.to_string(), mimetype: "image/jpeg".to_string() }
        }

        /// Matroska: every picture left out of the map and attached again after the file's own
        /// attachments; the mp4 family: the track marked where it stands, its data tracks left out.
        #[test]
        fn the_rewrite_attaches_or_marks_the_pictures() {
            let font = Stream { filename: Some("font.ttf".to_string()), ..stream(4, "attachment", "ttf") };
            let streams = [stream(0, "video", "av1"), stream(1, "audio", "opus"), stream(2, "video", "mjpeg"), attached(3), font];
            let plan = Plan {
                excluded: vec![3, 2],
                attached: vec![cover("/t/cover-3.jpg", "cover.jpg"), cover("/t/cover-2.jpg", "cover-2.jpg")],
                restored: vec!["cover-2.jpg".to_string()],
                ..Plan::default()
            };
            assert_eq!(
                strs(&_rewrite_argv(Path::new("v.mkv"), &streams, &plan, Keeps::Attaching, Path::new("v.correcting.mkv"))),
                [
                    "-v", "error", "-y", "-i", "v.mkv", "-map", "0", "-map", "-0:2", "-map", "-0:3", "-c", "copy",
                    "-attach", "/t/cover-3.jpg", "-metadata:s:t:1", "mimetype=image/jpeg", "-metadata:s:t:1", "filename=cover.jpg",
                    "-attach", "/t/cover-2.jpg", "-metadata:s:t:2", "mimetype=image/jpeg", "-metadata:s:t:2", "filename=cover-2.jpg",
                    "v.correcting.mkv",
                ]
            );
            let mp4 = [stream(0, "video", "h264"), stream(1, "audio", "aac"), stream(2, "data", "bin_data"), stream(3, "video", "mjpeg")];
            let marked = Plan { marked: vec![3], restored: vec!["cover.jpg".to_string()], ..Plan::default() };
            assert_eq!(
                strs(&_rewrite_argv(Path::new("v.mp4"), &mp4, &marked, Keeps::Marking, Path::new("v.correcting.mp4"))),
                ["-v", "error", "-y", "-i", "v.mp4", "-map", "0", "-map", "-0:2", "-c", "copy", "-disposition:2", "attached_pic", "v.correcting.mp4"]
            );
        }

        /// A corrected file checks out when every stream not left out is there as it was, in order,
        /// and the covers kept are marked so — not when a stream went missing, nor when a picture
        /// is still a plain track.
        #[test]
        fn a_corrected_file_checks_out_only_as_planned() {
            let streams = [stream(0, "video", "av1"), stream(1, "audio", "opus"), stream(2, "video", "mjpeg")];
            let plan = Plan { excluded: vec![2], attached: vec![cover("/t/c.jpg", "cover.jpg")], restored: vec!["cover.jpg".to_string()], ..Plan::default() };
            let good = [stream(0, "video", "av1"), stream(1, "audio", "opus"), attached(2)];
            assert!(_checks_out(&streams, &plan, Keeps::Attaching, &good));
            let lost = [stream(0, "video", "av1"), attached(1)];
            assert!(!_checks_out(&streams, &plan, Keeps::Attaching, &lost), "the sound went missing");
            let still = [stream(0, "video", "av1"), stream(1, "audio", "opus"), stream(2, "video", "mjpeg")];
            assert!(!_checks_out(&streams, &plan, Keeps::Attaching, &still), "the picture is still a track");
        }
    }
}
