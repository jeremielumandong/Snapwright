#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use super::*;

/// A synthetic screen: a grey background with a square moving one step per frame.
struct Moving {
    width: u32,
    height: u32,
    n: u32,
}

impl FrameSource for Moving {
    fn next_frame(&mut self) -> Result<Frame> {
        let (w, h) = (self.width, self.height);
        let mut data = [60u8, 60, 60, 255].repeat((w * h) as usize);
        let side = (h / 4).max(2).min(w);
        let x0 = (self.n * 4) % (w - side + 1);
        for y in h / 3..(h / 3 + side).min(h) {
            for x in x0..x0 + side {
                let i = ((y * w + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[40, 120, 240, 255]);
            }
        }
        self.n += 1;
        Ok(Frame {
            display_id: 0,
            width: w,
            height: h,
            data,
        })
    }
}

fn record_for(
    format: Format,
    size: (u32, u32),
    seconds: f32,
) -> Result<(Summary, tempfile::TempDir)> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("clip.{}", format.extension()));
    let source = Moving {
        width: size.0,
        height: size.1,
        n: 0,
    };
    let recorder = Recorder::start(source, Options::new(format), &path)?;
    std::thread::sleep(Duration::from_secs_f32(seconds));
    Ok((recorder.stop()?, dir))
}

/// MP4 boxes at the top level of the file, in order.
fn top_level_boxes(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        out.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
        let size = match size {
            1 => u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize,
            0 => bytes.len() - at,
            s => s,
        };
        if size < 8 {
            break;
        }
        at += size;
    }
    out
}

#[test]
fn mp4_has_its_index_first_and_the_right_size() {
    // An odd size checks the even-size rounding H.264 needs.
    let (summary, _dir) = match record_for(Format::Mp4, (321, 241), 1.2) {
        Err(RecordError::Unsupported(why)) if cfg!(target_os = "linux") => {
            eprintln!("skipped: {why}");
            return;
        }
        other => other.expect("record"),
    };
    assert_eq!((summary.width, summary.height), (320, 240));
    assert_eq!((summary.poster.width, summary.poster.height), (321, 241));
    // The capture side keeps its pace; the encoder may shed frames while it warms up (CI's
    // macOS VMs have no hardware encoder), but most of them must reach the file.
    let (frames, dropped) = (summary.frames, summary.dropped);
    assert!(frames + dropped >= 24, "{frames} frames, {dropped} dropped");
    assert!(frames >= 8, "{frames} frames, {dropped} dropped");
    assert!(summary.duration >= Duration::from_secs(1));
    let bytes = std::fs::read(&summary.path).unwrap();
    let boxes = top_level_boxes(&bytes);
    let moov = boxes.iter().position(|b| b == "moov").expect("moov box");
    let mdat = boxes.iter().position(|b| b == "mdat").expect("mdat box");
    assert_eq!(boxes.first().map(String::as_str), Some("ftyp"), "{boxes:?}");
    assert!(moov < mdat, "index after the data: {boxes:?}");
    // The track says H.264.
    assert!(
        bytes.windows(4).any(|w| w == b"avc1"),
        "no avc1 sample entry"
    );
    if let Some(probe) = ffprobe(&summary.path) {
        assert!(probe.contains("codec_name=h264"), "{probe}");
        assert!(
            probe.contains("width=320") && probe.contains("height=240"),
            "{probe}"
        );
        let duration: f64 = probe
            .lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.parse().ok())
            .expect("duration");
        assert!((1.0..2.0).contains(&duration), "duration {duration}");
    }
}

/// `ffprobe`'s view of the video stream, when ffprobe is installed.
fn ffprobe(path: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=codec_name,width,height,duration"])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn gif_loops_keeps_time_and_writes_only_changes() {
    let (summary, _dir) = record_for(Format::Gif, (200, 120), 1.0).expect("record");
    let file = std::fs::File::open(&summary.path).unwrap();
    let mut options = ::gif::DecodeOptions::new();
    options.set_color_output(::gif::ColorOutput::Indexed);
    let mut decoder = options.read_info(file).unwrap();
    assert_eq!((decoder.width(), decoder.height()), (200, 120));
    let mut frames = 0;
    let mut centis = 0u32;
    let mut partial = 0;
    while let Some(frame) = decoder.read_next_frame().unwrap() {
        frames += 1;
        centis += u32::from(frame.delay);
        if frame.width < 200 || frame.height < 120 {
            partial += 1;
        }
    }
    assert!(frames >= 8, "{frames} frames");
    assert_eq!(
        partial,
        frames - 1,
        "every frame after the first is a changed rectangle"
    );
    let seconds = centis as f32 / 100.0;
    assert!((0.9..1.6).contains(&seconds), "{seconds} s");
}

#[test]
fn gif_scales_wide_frames_down() {
    let (summary, _dir) = record_for(Format::Gif, (2600, 100), 0.3).expect("record");
    // 2600 wide needs a factor of 3 to fit 1280.
    assert_eq!((summary.width, summary.height), (866, 33));
}

#[test]
fn pause_leaves_time_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paused.gif");
    let source = Moving {
        width: 64,
        height: 64,
        n: 0,
    };
    let recorder = Recorder::start(source, Options::new(Format::Gif), &path).expect("start");
    std::thread::sleep(Duration::from_millis(300));
    recorder.pause();
    assert!(recorder.is_paused());
    std::thread::sleep(Duration::from_millis(200));
    let during = recorder.elapsed();
    std::thread::sleep(Duration::from_millis(400));
    assert!(recorder.elapsed() - during < Duration::from_millis(100));
    recorder.resume();
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let summary = recorder.stop().expect("stop");
    assert!(started.elapsed() < Duration::from_secs(5));
    let d = summary.duration.as_secs_f32();
    assert!(
        (0.5..0.95).contains(&d),
        "recorded {d} s of 1.2 s with 0.6 s paused"
    );
}

#[test]
fn a_source_failing_at_start_is_reported() {
    struct Broken;
    impl FrameSource for Broken {
        fn next_frame(&mut self) -> Result<Frame> {
            Err(RecordError::Unsupported("no screen".into()))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let err = Recorder::start(Broken, Options::default(), dir.path().join("x.mp4"))
        .err()
        .expect("fails");
    assert!(
        matches!(err, RecordError::Unsupported(ref m) if m == "no screen"),
        "{err}"
    );
}

#[test]
fn a_one_pixel_region_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let source = Moving {
        width: 1,
        height: 40,
        n: 0,
    };
    let err = Recorder::start(source, Options::default(), dir.path().join("x.mp4"))
        .err()
        .expect("fails");
    assert!(matches!(err, RecordError::EmptyRegion(_)), "{err}");
}

#[test]
fn formats_from_paths() {
    use std::path::Path;
    assert_eq!(Format::from_path(Path::new("a.MP4")), Some(Format::Mp4));
    assert_eq!(Format::from_path(Path::new("a.gif")), Some(Format::Gif));
    assert_eq!(Format::from_path(Path::new("a.mov")), None);
}

#[test]
fn bitrate_scales_with_pixels() {
    assert_eq!(encoder::bitrate(1920, 1080, 30), 6_220_800);
    assert_eq!(encoder::bitrate(100, 100, 30), 1_000_000);
}
