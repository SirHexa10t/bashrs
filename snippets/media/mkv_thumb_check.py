#!/usr/bin/env python3
"""Check whether MKV/WebM files carry an embedded thumbnail (Matroska
cover-art attachment) that players and thumbnailers can actually use.

Usage:
    python3 mkv_thumb_check.py VIDEO.mkv [MORE.mkv ...]
    python3 mkv_thumb_check.py --extract DIR VIDEO.mkv   # also dump the image(s)

Pure Python standard library — nothing to install. Exit code 0 if every
file has a usable cover, 1 otherwise.

What "usable" means (rules mirror the actual consumers):
- ffmpeg's Matroska demuxer only maps attachments with mimetype
  image/jpeg, image/png, image/gif or image/tiff to an "attached picture"
  video stream (libavformat/matroskadec.c, mkv_image_mime_tags). Anything
  else (notably image/webp) is invisible to ffmpeg-based tools.
- ffmpegthumbnailer -m only accepts MJPEG/PNG still-image streams and
  prefers those whose metadata filename starts with "cover."
  (libffmpegthumbnailer/moviedecoder.cpp).
So: a JPEG or PNG attachment named cover.<ext> is the portable choice.
"""

import os
import sys

NAMES = {
    0x18538067: 'Segment', 0x1654AE6B: 'Tracks', 0x1941A469: 'Attachments',
    0x61A7: 'AttachedFile', 0x466E: 'FileName', 0x4660: 'FileMimeType',
    0xAE: 'TrackEntry', 0x83: 'TrackType', 0x86: 'CodecID',
    0x536E: 'TrackName', 0xD7: 'TrackNumber', 0xE0: 'Video', 0xE1: 'Audio',
    0xB0: 'PixelWidth', 0xBA: 'PixelHeight', 0x22B59C: 'Language',
}
DESCEND = {'Segment', 'Tracks', 'TrackEntry', 'Video', 'Audio',
           'Attachments', 'AttachedFile'}
TRACK_TYPES = {1: 'video', 2: 'audio', 17: 'subtitle'}

# mimetypes ffmpeg maps to an attached-picture stream, and the subset
# ffmpegthumbnailer's -m will actually render
FFMPEG_IMAGE_MIMES = {'image/jpeg', 'image/png', 'image/gif', 'image/tiff'}
THUMBNAILER_MIMES = {'image/jpeg', 'image/png'}

MAGICS = (
    (b'\xff\xd8\xff', 'jpeg'),
    (b'\x89PNG', 'png'),
    (b'GIF8', 'gif'),
    (b'II*\x00', 'tiff'),
    (b'MM\x00*', 'tiff'),
)


def sniff(head):
    for magic, kind in MAGICS:
        if head.startswith(magic):
            return kind
    if head[:4] == b'RIFF' and head[8:12] == b'WEBP':
        return 'webp'
    return 'unknown'


def parse(path, extract_dir=None):
    f = open(path, 'rb')
    fsize = os.path.getsize(path)
    info = {'tracks': [], 'attachments': []}
    state = {'track': None, 'att': None, 'n': 0}

    def rv(keep):
        c = f.read(1)
        if not c:
            return None, False
        b0 = c[0]
        ln = 0
        for i in range(8):
            if b0 & (0x80 >> i):
                ln = i + 1
                break
        if ln == 0:
            raise ValueError('bad EBML varint @%d' % (f.tell() - 1))
        v = b0 if keep else b0 & (0xFF >> ln)
        for x in f.read(ln - 1):
            v = (v << 8) | x
        unknown = (not keep) and v == (1 << (7 * ln)) - 1
        return v, unknown

    def walk(end):
        while f.tell() < end:
            eid, _ = rv(True)
            if eid is None:
                return
            size, unknown = rv(False)
            dstart = f.tell()
            dend = end if unknown else dstart + size
            name = NAMES.get(eid, '')
            if name == 'TrackEntry':
                state['track'] = {}
                info['tracks'].append(state['track'])
            if name == 'AttachedFile':
                state['att'] = {}
                info['attachments'].append(state['att'])
            if name in DESCEND:
                walk(dend)
            elif name in ('FileName', 'FileMimeType', 'CodecID',
                          'TrackName', 'Language'):
                val = f.read(size).decode('utf-8', 'replace')
                target = state['att'] if name.startswith('File') else state['track']
                if target is not None:
                    target[name] = val
            elif name in ('TrackType', 'TrackNumber', 'PixelWidth',
                          'PixelHeight'):
                val = int.from_bytes(f.read(size), 'big')
                if state['track'] is not None:
                    state['track'][name] = val
            elif eid == 0x465C:  # FileData
                head = f.read(min(16, size))
                if state['att'] is not None:
                    state['att'].update(size=size, sniffed=sniff(head))
                    if extract_dir:
                        f.seek(dstart)
                        state['n'] += 1
                        base = state['att'].get('FileName') or 'attachment%d' % state['n']
                        out = os.path.join(extract_dir, os.path.basename(base))
                        with open(out, 'wb') as o:
                            remaining = size
                            while remaining:
                                chunk = f.read(min(1 << 20, remaining))
                                if not chunk:
                                    break
                                o.write(chunk)
                                remaining -= len(chunk)
                        state['att']['extracted_to'] = out
            f.seek(dend)

    walk(fsize)
    f.close()
    return info


def report(path, info):
    ok = False
    print('== %s' % path)
    for t in info['tracks']:
        kind = TRACK_TYPES.get(t.get('TrackType'), '?')
        extra = ''
        if kind == 'video':
            extra = ' %sx%s' % (t.get('PixelWidth', '?'), t.get('PixelHeight', '?'))
        print('   track %s: %s %s%s (%s)' % (
            t.get('TrackNumber', '?'), kind, t.get('CodecID', '?'),
            extra, t.get('Language', 'und')))
        if kind == 'video' and t.get('CodecID') in ('V_MJPEG', 'V_PNG'):
            print('     note: still-image codec as a REAL video track — '
                  'non-standard thumbnail embedding; prefer an attachment.')
    if not info['attachments']:
        print('   no attachments: NO embedded thumbnail. Players will not '
              'list a cover track; thumbnailers can only frame-grab.')
    for a in info['attachments']:
        name = a.get('FileName', '?')
        mime = a.get('FileMimeType', '?')
        sniffed = a.get('sniffed', 'not-read')
        print('   attachment: %s (%s, %s bytes, content looks like: %s)%s' % (
            name, mime, a.get('size', '?'), sniffed,
            ' -> ' + a['extracted_to'] if 'extracted_to' in a else ''))
        expected = 'image/' + {'jpeg': 'jpeg', 'png': 'png', 'gif': 'gif',
                               'tiff': 'tiff'}.get(sniffed, '???')
        if sniffed != 'not-read' and expected != mime:
            print('     MISMATCH: declared %s but content is %s — fix the '
                  'mimetype or convert the image.' % (mime, sniffed))
            continue
        if mime not in FFMPEG_IMAGE_MIMES:
            print('     UNUSABLE: ffmpeg does not map %s attachments to a '
                  'picture stream (webp etc. are invisible). Re-embed as '
                  'JPEG or PNG.' % mime)
        elif mime not in THUMBNAILER_MIMES:
            print('     PARTIAL: players will list it, but ffmpegthumbnailer '
                  'only renders JPEG/PNG covers. Re-embed as JPEG or PNG.')
        else:
            ok = True
            print('     OK: standard Matroska cover art. Players list it as '
                  'an extra "attached picture" video track (normal); '
                  'ffmpegthumbnailer renders it when run with -m.')
            if not name.startswith('cover.'):
                print('     hint: name it cover.%s — streams named cover.* '
                      'are preferred when several images are embedded.' % sniffed)
    print()
    return ok


def main(argv):
    args = argv[1:]
    extract_dir = None
    if args[:1] == ['--extract']:
        if len(args) < 3:
            print(__doc__)
            return 2
        extract_dir = args[1]
        os.makedirs(extract_dir, exist_ok=True)
        args = args[2:]
    if not args:
        print(__doc__)
        return 2
    all_ok = True
    for path in args:
        try:
            info = parse(path, extract_dir)
        except (OSError, ValueError) as e:
            print('== %s\n   parse error: %s (not Matroska?)\n' % (path, e))
            all_ok = False
            continue
        all_ok &= report(path, info)
    return 0 if all_ok else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv))
