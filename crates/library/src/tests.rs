#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

/// A solid `w` x `h` image as (png, rgba).
fn image(w: u32, h: u32, rgb: [u8; 3]) -> (Vec<u8>, Vec<u8>) {
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    let mut png_bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut png_bytes, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(&rgba).unwrap();
    writer.finish().unwrap();
    (png_bytes, rgba)
}

fn meta(app: &str, title: &str) -> Metadata {
    Metadata {
        source: Source::Window,
        app: app.into(),
        window_title: title.into(),
        url: String::new(),
        display_id: Some(7),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    lib: Library,
}

fn library() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::open(dir.path().join("lib")).unwrap();
    Fixture { _dir: dir, lib }
}

fn add(lib: &mut Library, at: i64, rgb: [u8; 3], meta: Metadata) -> i64 {
    let (png, rgba) = image(640, 360, rgb);
    lib.add_at(
        &NewCapture {
            png: &png,
            rgba: &rgba,
            width: 640,
            height: 360,
            meta,
        },
        at,
    )
    .unwrap()
}

fn ids(captures: &[Capture]) -> Vec<i64> {
    captures.iter().map(|c| c.id).collect()
}

#[test]
fn add_stores_image_thumbnail_and_metadata() {
    let mut f = library();
    let id = add(
        &mut f.lib,
        1000,
        [10, 20, 30],
        meta("Figma", "Login screen"),
    );
    let c = f.lib.get(id).unwrap();
    assert_eq!((c.width, c.height, c.created_ms), (640, 360, 1000));
    assert_eq!(c.meta, meta("Figma", "Login screen"));
    assert!(!c.trashed);
    let stored = std::fs::read(f.lib.image_path_of(&c)).unwrap();
    assert_eq!(stored.len() as u64, c.bytes);
    assert_eq!(blake3::hash(&stored).to_hex().as_str(), c.hash);
    let thumb = png::Decoder::new(std::io::Cursor::new(
        std::fs::read(f.lib.thumb_path_of(&c)).unwrap(),
    ))
    .read_info()
    .unwrap();
    assert_eq!((thumb.info().width, thumb.info().height), (320, 180));
}

#[test]
fn identical_images_share_one_file() {
    let mut f = library();
    let a = add(&mut f.lib, 1, [1, 2, 3], meta("A", ""));
    let b = add(&mut f.lib, 2, [1, 2, 3], meta("B", ""));
    let (a, b) = (f.lib.get(a).unwrap(), f.lib.get(b).unwrap());
    assert_eq!(a.hash, b.hash);
    let files = std::fs::read_dir(f.lib.root().join("store").join(&a.hash[..2]))
        .unwrap()
        .count();
    assert_eq!(files, 1);
}

#[test]
fn list_is_newest_first_and_pages() {
    let mut f = library();
    let a = add(&mut f.lib, 100, [1, 0, 0], Metadata::default());
    let b = add(&mut f.lib, 300, [2, 0, 0], Metadata::default());
    let c = add(&mut f.lib, 200, [3, 0, 0], Metadata::default());
    let all = f.lib.list(&Query::default()).unwrap();
    assert_eq!(ids(&all), vec![b, c, a]);
    let page = f
        .lib
        .list(&Query {
            limit: 2,
            offset: 1,
            ..Query::default()
        })
        .unwrap();
    assert_eq!(ids(&page), vec![c, a]);
    let recent = f
        .lib
        .list(&Query {
            since_ms: Some(200),
            ..Query::default()
        })
        .unwrap();
    assert_eq!(ids(&recent), vec![b, c]);
}

#[test]
fn search_matches_word_prefixes_across_fields() {
    let mut f = library();
    let figma = add(
        &mut f.lib,
        1,
        [1, 0, 0],
        meta("Figma", "Login screen — Draft"),
    );
    let mail = add(&mut f.lib, 2, [2, 0, 0], meta("Mail", "Invoice from Acme"));
    let browser = add(
        &mut f.lib,
        3,
        [3, 0, 0],
        Metadata {
            url: "https://docs.rs/rusqlite".into(),
            ..meta("Firefox", "rusqlite docs")
        },
    );
    let search = |lib: &Library, text: &str| {
        ids(&lib
            .list(&Query {
                text: Some(text.into()),
                ..Query::default()
            })
            .unwrap())
    };
    assert_eq!(search(&f.lib, "log"), vec![figma]);
    assert_eq!(search(&f.lib, "fig draft"), vec![figma]);
    assert_eq!(search(&f.lib, "FIGMA invoice"), Vec::<i64>::new());
    assert_eq!(search(&f.lib, "docs.rs"), vec![browser]);
    assert_eq!(search(&f.lib, "acme"), vec![mail]);
    assert_eq!(search(&f.lib, "f"), vec![browser, mail, figma]);
    // Text with no words searches nothing and lists everything.
    assert_eq!(search(&f.lib, " \"* "), vec![browser, mail, figma]);
    // Notes and recognised text are searchable too.
    f.lib.set_note(mail, "pay before Friday").unwrap();
    f.lib.set_text(figma, "Forgot password?").unwrap();
    assert_eq!(search(&f.lib, "friday"), vec![mail]);
    assert_eq!(search(&f.lib, "password"), vec![figma]);
}

#[test]
fn tags_filter_search_and_count() {
    let mut f = library();
    let a = add(&mut f.lib, 1, [1, 0, 0], meta("A", "one"));
    let b = add(&mut f.lib, 2, [2, 0, 0], meta("B", "two"));
    f.lib.tag(a, "Bug").unwrap();
    f.lib.tag(b, "bug").unwrap();
    f.lib.tag(b, "release-2").unwrap();
    f.lib.tag(b, "  ").unwrap();
    assert_eq!(f.lib.get(b).unwrap().tags, vec!["Bug", "release-2"]);
    assert_eq!(
        f.lib.tags().unwrap(),
        vec![("Bug".into(), 2), ("release-2".into(), 1)]
    );
    let tagged = |lib: &Library, tag: &str| {
        ids(&lib
            .list(&Query {
                tag: Some(tag.into()),
                ..Query::default()
            })
            .unwrap())
    };
    assert_eq!(tagged(&f.lib, "BUG"), vec![b, a]);
    assert_eq!(tagged(&f.lib, "release-2"), vec![b]);
    // Tags are part of the full-text index.
    let q = Query {
        text: Some("release".into()),
        ..Query::default()
    };
    assert_eq!(ids(&f.lib.list(&q).unwrap()), vec![b]);
    assert_eq!(f.lib.count(&q).unwrap(), 1);
    f.lib.untag(b, "release-2").unwrap();
    assert!(f.lib.list(&q).unwrap().is_empty());
    assert!(matches!(f.lib.tag(999, "x"), Err(Error::NotFound(999))));
}

#[test]
fn trash_restore_and_purge() {
    let mut f = library();
    let a = add(&mut f.lib, 1, [1, 0, 0], meta("A", "keep"));
    let b = add(&mut f.lib, 2, [2, 0, 0], meta("B", "drop"));
    // A third capture shares b's image, so purging b must keep the file.
    let c = add(&mut f.lib, 3, [2, 0, 0], meta("C", "same pixels"));
    let d = add(&mut f.lib, 4, [4, 0, 0], meta("D", "drop too"));
    f.lib.tag(d, "temp").unwrap();
    f.lib.trash(b).unwrap();
    f.lib.trash(d).unwrap();
    f.lib.trash(d).unwrap(); // twice is fine
    assert_eq!(ids(&f.lib.list(&Query::default()).unwrap()), vec![c, a]);
    let trash = Query {
        trash: true,
        ..Query::default()
    };
    assert_eq!(ids(&f.lib.list(&trash).unwrap()), vec![d, b]);
    assert!(
        f.lib.tags().unwrap().is_empty(),
        "trashed captures' tags are hidden"
    );

    f.lib.restore(b).unwrap();
    f.lib.trash(b).unwrap();
    let (b_cap, d_cap) = (f.lib.get(b).unwrap(), f.lib.get(d).unwrap());
    assert_eq!(f.lib.purge_trash().unwrap(), 2);
    assert!(matches!(f.lib.get(b), Err(Error::NotFound(_))));
    assert!(f.lib.image_path_of(&b_cap).exists(), "still used by c");
    assert!(!f.lib.image_path_of(&d_cap).exists());
    assert!(!f.lib.thumb_path_of(&d_cap).exists());
    let search = Query {
        text: Some("drop".into()),
        ..Query::default()
    };
    assert!(f.lib.list(&search).unwrap().is_empty());
    assert!(matches!(f.lib.restore(b), Err(Error::NotFound(_))));
}

#[test]
fn reopening_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let mut lib = Library::open(dir.path()).unwrap();
        let id = add(&mut lib, 5, [9, 9, 9], meta("Term", "cargo test"));
        lib.tag(id, "ci").unwrap();
        id
    };
    let lib = Library::open(dir.path()).unwrap();
    let c = lib.get(id).unwrap();
    assert_eq!(c.tags, vec!["ci"]);
    let q = Query {
        text: Some("cargo".into()),
        ..Query::default()
    };
    assert_eq!(ids(&lib.list(&q).unwrap()), vec![id]);
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    drop(Library::open(dir.path()).unwrap());
    let db = Connection::open(dir.path().join("library.sqlite3")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    assert!(matches!(Library::open(dir.path()), Err(Error::TooNew(99))));
}

#[test]
fn bad_pixels_are_rejected_before_anything_is_recorded() {
    let mut f = library();
    let (png, _) = image(4, 4, [0, 0, 0]);
    let err = f.lib.add(&NewCapture {
        png: &png,
        rgba: &[0; 10],
        width: 4,
        height: 4,
        meta: Metadata::default(),
    });
    assert!(matches!(err, Err(Error::Image(_))));
    assert!(f.lib.list(&Query::default()).unwrap().is_empty());
}

fn edit(lib: &mut Library, id: i64, rgb: [u8; 3], scene: &str) {
    let (png, rgba) = image(320, 200, rgb);
    lib.save_edit(
        id,
        scene,
        &EditedImage {
            png: &png,
            rgba: &rgba,
            width: 320,
            height: 200,
        },
    )
    .unwrap();
}

#[test]
fn edits_are_shown_in_place_of_the_original_and_can_be_reverted() {
    let mut f = library();
    let id = add(&mut f.lib, 1, [10, 20, 30], meta("Figma", "Board"));
    let original = f.lib.image_path_of(&f.lib.get(id).unwrap());

    edit(&mut f.lib, id, [200, 0, 0], r#"{"version":1,"shapes":[]}"#);
    let c = f.lib.get(id).unwrap();
    let first = c.edit.clone().unwrap();
    assert_eq!(first.scene, r#"{"version":1,"shapes":[]}"#);
    assert_eq!(c.shown_size(), (320, 200));
    assert_eq!((c.width, c.height), (640, 360));
    assert_ne!(f.lib.image_path_of(&c), original);
    assert_eq!(f.lib.original_path_of(&c), original);
    assert!(f.lib.image_path_of(&c).exists() && f.lib.thumb_path_of(&c).exists());
    let first_path = f.lib.image_path_of(&c);

    // A new edit replaces the old one, whose files go.
    edit(&mut f.lib, id, [0, 200, 0], "{}");
    let c = f.lib.get(id).unwrap();
    assert_ne!(c.edit.as_ref().unwrap().hash, first.hash);
    assert!(!first_path.exists());

    let edited = f.lib.image_path_of(&c);
    f.lib.revert_edit(id).unwrap();
    let c = f.lib.get(id).unwrap();
    assert_eq!(c.edit, None);
    assert_eq!(f.lib.image_path_of(&c), original);
    assert!(original.exists() && !edited.exists());
}

#[test]
fn purging_removes_edits_too() {
    let mut f = library();
    let id = add(&mut f.lib, 1, [10, 20, 30], meta("a", "b"));
    edit(&mut f.lib, id, [1, 2, 3], "{}");
    let c = f.lib.get(id).unwrap();
    let (shown, original) = (f.lib.image_path_of(&c), f.lib.original_path_of(&c));
    f.lib.trash(id).unwrap();
    assert_eq!(f.lib.purge_trash().unwrap(), 1);
    assert!(!shown.exists() && !original.exists());
}

#[test]
fn version_1_libraries_upgrade_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let mut lib = Library::open(dir.path()).unwrap();
        add(&mut lib, 5, [1, 1, 1], meta("Safari", "Docs"))
    };
    // Turn the database back into a version 1 library.
    let db = Connection::open(dir.path().join("library.sqlite3")).unwrap();
    db.execute_batch(
        "DROP INDEX captures_video_hash;
         ALTER TABLE captures DROP COLUMN video_hash;
         ALTER TABLE captures DROP COLUMN video_ext;
         ALTER TABLE captures DROP COLUMN video_bytes;
         ALTER TABLE captures DROP COLUMN duration_ms;
         DROP INDEX captures_edit_hash;
         ALTER TABLE captures DROP COLUMN edit_hash;
         ALTER TABLE captures DROP COLUMN edit_width;
         ALTER TABLE captures DROP COLUMN edit_height;
         ALTER TABLE captures DROP COLUMN edit_bytes;
         ALTER TABLE captures DROP COLUMN scene;
         PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(db);
    let mut lib = Library::open(dir.path()).unwrap();
    assert_eq!(lib.get(id).unwrap().meta.app, "Safari");
    edit(&mut lib, id, [9, 9, 9], "{}");
    assert!(lib.get(id).unwrap().edit.is_some());
    assert_eq!(lib.get(id).unwrap().video, None);
}

#[test]
fn recordings_keep_their_video_beside_the_poster() {
    let mut f = library();
    let clip = f._dir.path().join("Clip.MP4");
    std::fs::write(&clip, b"not really a movie").unwrap();
    let (png, rgba) = image(64, 36, [9, 9, 9]);
    let id = f
        .lib
        .add_recording(&NewRecording {
            poster: NewCapture {
                png: &png,
                rgba: &rgba,
                width: 64,
                height: 36,
                meta: meta("Editor", "demo"),
            },
            video: &clip,
            duration_ms: 4200,
        })
        .unwrap();
    let rec = f.lib.get(id).unwrap();
    let video = rec.video.clone().expect("a recording");
    assert_eq!(
        (video.ext.as_str(), video.bytes, video.duration_ms),
        ("mp4", 18, 4200)
    );
    let stored = f.lib.video_path_of(&rec).unwrap();
    assert_eq!(std::fs::read(&stored).unwrap(), b"not really a movie");
    assert!(clip.exists(), "the user's file stays where it was");
    assert!(
        f.lib.thumb_path_of(&rec).exists(),
        "the poster is a normal capture"
    );
    let plain = add(&mut f.lib, 1, [1, 2, 3], meta("A", "b"));
    assert_eq!(f.lib.get(plain).unwrap().video, None);

    f.lib.trash(id).unwrap();
    f.lib.purge_trash().unwrap();
    assert!(!stored.exists(), "purging removes the stored video");
    assert!(clip.exists());
}
