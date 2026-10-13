//! Moves an MP4's index (`moov`) in front of its data (`mdat`), so the file starts playing
//! before it has fully downloaded.
//!
//! Used where the encoder cannot do it itself: Media Foundation's own option
//! (`MF_MPEG4SINK_MOOV_BEFORE_MDAT`) writes a file without its `mdat` header. Only the index
//! is held in memory; the data is streamed into a new file next to the old one, which then
//! replaces it.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::{RecordError, Result};

/// A top-level box: where it starts and how long it is, header included.
#[derive(Debug, Clone, Copy)]
struct Atom {
    kind: [u8; 4],
    start: u64,
    size: u64,
}

/// Boxes inside `moov` that hold other boxes on the way to the chunk offset tables.
const CONTAINERS: [&[u8; 4]; 5] = [b"moov", b"trak", b"mdia", b"minf", b"stbl"];

/// Rewrites the MP4 at `path` with `moov` first. Files already in that order, and files
/// this does not understand, are left as they are.
pub(crate) fn faststart(path: &Path) -> Result<()> {
    let io = |source| RecordError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = File::open(path).map_err(io)?;
    let len = file.metadata().map_err(io)?.len();
    let atoms = top_level(&mut file, len).map_err(io)?;
    let find = |kind: &[u8; 4]| atoms.iter().position(|a| &a.kind == kind);
    let (Some(moov_at), Some(mdat_at)) = (find(b"moov"), find(b"mdat")) else {
        return Ok(());
    };
    if moov_at < mdat_at {
        return Ok(());
    }
    let moov = atoms[moov_at];
    let mut index = vec![0u8; moov.size as usize];
    file.seek(SeekFrom::Start(moov.start)).map_err(io)?;
    file.read_exact(&mut index).map_err(io)?;
    // Every chunk after the old index position moves forward by the index's size.
    if !shift_offsets(&mut index, moov.size) {
        tracing::warn!(path = %path.display(), "MP4 index not moved: offsets would overflow");
        return Ok(());
    }

    let tmp = path.with_extension("faststart.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut out = BufWriter::new(File::create(&tmp)?);
        let mut input = BufReader::new(&mut file);
        for (i, atom) in atoms.iter().enumerate() {
            if i == moov_at {
                continue;
            }
            if i == mdat_at {
                out.write_all(&index)?;
            }
            input.seek(SeekFrom::Start(atom.start))?;
            let copied = std::io::copy(&mut (&mut input).take(atom.size), &mut out)?;
            if copied != atom.size {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
        }
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(io)
}

/// The file's top-level boxes, or an error if they do not tile it exactly.
fn top_level(file: &mut File, len: u64) -> std::io::Result<Vec<Atom>> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "not an MP4 box layout");
    let mut atoms = Vec::new();
    let mut at = 0;
    while at < len {
        let mut header = [0u8; 16];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut header[..8])?;
        let kind = [header[4], header[5], header[6], header[7]];
        let size = match u32::from_be_bytes([header[0], header[1], header[2], header[3]]) {
            0 => len - at,
            1 => {
                file.read_exact(&mut header[8..])?;
                u64::from_be_bytes([
                    header[8], header[9], header[10], header[11], header[12], header[13],
                    header[14], header[15],
                ])
            }
            s => u64::from(s),
        };
        if size < 8 || at + size > len {
            return Err(bad());
        }
        atoms.push(Atom {
            kind,
            start: at,
            size,
        });
        at += size;
    }
    Ok(atoms)
}

/// Adds `by` to every chunk offset (`stco`, `co64`) inside the `moov` box in `index`.
/// Returns false, changing nothing that matters, if a 32-bit offset would overflow.
fn shift_offsets(index: &mut [u8], by: u64) -> bool {
    let Some(header) = header_len(index) else {
        return false;
    };
    walk(&mut index[header..], by)
}

/// Header length of the box at the start of `b` (8, or 16 with a 64-bit size).
fn header_len(b: &[u8]) -> Option<usize> {
    let size = u32::from_be_bytes(b.get(..4)?.try_into().ok()?);
    Some(if size == 1 { 16 } else { 8 })
}

fn walk(mut body: &mut [u8], by: u64) -> bool {
    while body.len() >= 8 {
        let Some(header) = header_len(body) else {
            return false;
        };
        let size = match u32::from_be_bytes([body[0], body[1], body[2], body[3]]) {
            0 => body.len() as u64,
            1 => match body.get(8..16) {
                Some(s) => u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]),
                None => return false,
            },
            s => u64::from(s),
        };
        if size < header as u64 || size > body.len() as u64 {
            return false;
        }
        let (this, rest) = std::mem::take(&mut body).split_at_mut(size as usize);
        let kind = [this[4], this[5], this[6], this[7]];
        let content = &mut this[header..];
        let ok = if CONTAINERS.contains(&&kind) {
            walk(content, by)
        } else if &kind == b"stco" {
            patch(content, 4, |o| {
                u32::try_from(u64::from(u32::from_be_bytes(o.try_into().ok()?)) + by)
                    .ok()
                    .map(|v| v.to_be_bytes().to_vec())
            })
        } else if &kind == b"co64" {
            patch(content, 8, |o| {
                u64::from_be_bytes(o.try_into().ok()?)
                    .checked_add(by)
                    .map(|v| v.to_be_bytes().to_vec())
            })
        } else {
            true
        };
        if !ok {
            return false;
        }
        body = rest;
    }
    true
}

/// Rewrites each entry of a chunk offset table (version and flags, a count, then entries of
/// `width` bytes) through `f`.
fn patch(table: &mut [u8], width: usize, f: impl Fn(&[u8]) -> Option<Vec<u8>>) -> bool {
    let Some(count) = table.get(4..8) else {
        return false;
    };
    let count = u32::from_be_bytes([count[0], count[1], count[2], count[3]]) as usize;
    let Some(entries) = table.get_mut(8..8 + count * width) else {
        return false;
    };
    for entry in entries.chunks_exact_mut(width) {
        match f(entry) {
            Some(v) => entry.copy_from_slice(&v),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut b = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(content);
        b
    }

    #[test]
    fn moves_the_index_first_and_shifts_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mp4");
        // ftyp, mdat (8 header + 8 data), moov > trak > mdia > minf > stbl > stco [24, 28].
        let ftyp = boxed(b"ftyp", b"isom\0\0\0\0");
        let mdat = boxed(b"mdat", b"AAAABBBB");
        let mut stco = vec![0, 0, 0, 0, 0, 0, 0, 2];
        stco.extend_from_slice(&24u32.to_be_bytes());
        stco.extend_from_slice(&28u32.to_be_bytes());
        let mut moov = boxed(b"stco", &stco);
        for kind in [b"stbl", b"minf", b"mdia", b"trak", b"moov"] {
            moov = boxed(kind, &moov);
        }
        let mut file = ftyp.clone();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&moov);
        std::fs::write(&path, &file).unwrap();

        faststart(&path).unwrap();
        let out = std::fs::read(&path).unwrap();
        assert_eq!(out.len(), file.len());
        assert_eq!(&out[..ftyp.len()], &ftyp[..]);
        assert_eq!(&out[ftyp.len() + 4..ftyp.len() + 8], b"moov");
        // The data moved by the index's size and the offsets followed it.
        let shift = moov.len();
        assert_eq!(&out[24 + shift..32 + shift], b"AAAABBBB");
        let table = ftyp.len() + moov.len() - 8;
        assert_eq!(&out[table..table + 4], &((24 + shift) as u32).to_be_bytes());
        assert_eq!(
            &out[table + 4..table + 8],
            &((28 + shift) as u32).to_be_bytes()
        );

        // A second pass changes nothing.
        faststart(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), out);
    }

    /// A real H.264 file with its index last (ffmpeg's default) decodes to the same frames
    /// afterwards.
    #[test]
    fn real_files_keep_their_frames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=15",
            ])
            .args(["-t", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(&path)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("skipped: no ffmpeg with libx264");
            return;
        }
        let order = |path: &Path| {
            let len = path.metadata().unwrap().len();
            let kinds: Vec<_> = top_level(&mut File::open(path).unwrap(), len)
                .unwrap()
                .iter()
                .map(|a| a.kind)
                .collect();
            let at = |k: &[u8; 4]| kinds.iter().position(|x| x == k).unwrap();
            at(b"moov") < at(b"mdat")
        };
        let frames = |path: &Path| {
            let out = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-xerror", "-i"])
                .arg(path)
                .args(["-f", "framemd5", "-"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        assert!(!order(&path));
        let before = frames(&path);
        faststart(&path).unwrap();
        assert!(order(&path));
        assert_eq!(frames(&path), before);
        assert_eq!(before.lines().filter(|l| !l.starts_with('#')).count(), 30);
    }

    #[test]
    fn files_it_does_not_understand_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.mp4");
        std::fs::write(&path, b"\0\0\0\x08ftyp\0\0\0\x40mdat").unwrap();
        assert!(faststart(&path).is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\0\0\0\x08ftyp\0\0\0\x40mdat"
        );
    }
}
