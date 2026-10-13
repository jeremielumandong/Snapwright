//! MP4 on macOS: AVAssetWriter with VideoToolbox's H.264 encoder (the media engine on Apple
//! silicon and on Intel Macs with a T2 or recent GPU). Frames go in as BGRA pixel buffers
//! from the adaptor's pool; VideoToolbox converts them to YUV on the GPU.

use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVAssetWriterStatus,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoCompressionPropertiesKey, AVVideoExpectedSourceFrameRateKey,
    AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::CFString;
use objc2_core_media::CMTime;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA, kCVReturnSuccess,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use snapwright_capture::Frame;

use crate::encoder::{Encoder, bitrate};
use crate::{RecordError, Result};

/// How long `finishWriting` may take before the recording is reported as failed.
const FINISH_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for the input to accept a frame before dropping it.
const READY_TIMEOUT: Duration = Duration::from_millis(100);

fn fail(api: &'static str, message: impl Into<String>) -> RecordError {
    RecordError::Encoder {
        api,
        message: message.into(),
    }
}

/// Microsecond timestamps.
fn cm_time(d: Duration) -> CMTime {
    // SAFETY: CMTimeMake only builds a struct.
    unsafe { CMTime::new(d.as_micros() as i64, 1_000_000) }
}

/// A CoreFoundation string constant as the NSString it is toll-free bridged to.
fn ns(s: &CFString) -> &NSString {
    // SAFETY: CFString and NSString are toll-free bridged: the same object under two names.
    unsafe { &*(s as *const CFString).cast::<NSString>() }
}

/// A framework string constant, which the bindings expose as optional.
fn constant<T: ?Sized>(c: Option<&'static T>, name: &'static str) -> Result<&'static T> {
    c.ok_or_else(|| fail(name, "missing from AVFoundation"))
}

pub(crate) struct AvEncoder {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: u32,
    height: u32,
    path: PathBuf,
}

impl AvEncoder {
    pub(crate) fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self> {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: framework constants read once; the dictionaries hold the value types
        // AVFoundation documents for each key.
        unsafe {
            let file_type = constant(AVFileTypeMPEG4, "AVFileTypeMPEG4")?;
            let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type)
                .map_err(|e| fail("AVAssetWriter", e.localizedDescription().to_string()))?;
            // The index before the data, so the file plays while it downloads.
            writer.setShouldOptimizeForNetworkUse(true);

            let compression = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    constant(AVVideoAverageBitRateKey, "AVVideoAverageBitRateKey")?,
                    constant(
                        AVVideoExpectedSourceFrameRateKey,
                        "AVVideoExpectedSourceFrameRateKey",
                    )?,
                ],
                &[
                    &NSNumber::new_u32(bitrate(width, height, fps)),
                    &NSNumber::new_u32(fps),
                ],
            );
            let codec: &AnyObject = constant(AVVideoCodecTypeH264, "AVVideoCodecTypeH264")?;
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    constant(AVVideoCodecKey, "AVVideoCodecKey")?,
                    constant(AVVideoWidthKey, "AVVideoWidthKey")?,
                    constant(AVVideoHeightKey, "AVVideoHeightKey")?,
                    constant(
                        AVVideoCompressionPropertiesKey,
                        "AVVideoCompressionPropertiesKey",
                    )?,
                ],
                &[
                    codec,
                    &NSNumber::new_u32(width),
                    &NSNumber::new_u32(height),
                    &compression,
                ],
            );
            let media_type = constant(AVMediaTypeVideo, "AVMediaTypeVideo")?;
            let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            );
            input.setExpectsMediaDataInRealTime(true);

            let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    ns(kCVPixelBufferPixelFormatTypeKey),
                    ns(kCVPixelBufferWidthKey),
                    ns(kCVPixelBufferHeightKey),
                ],
                &[
                    &NSNumber::new_u32(kCVPixelFormatType_32BGRA),
                    &NSNumber::new_u32(width),
                    &NSNumber::new_u32(height),
                ],
            );
            let adaptor = AVAssetWriterInputPixelBufferAdaptor::assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                &input,
                Some(&attributes),
            );
            if !writer.canAddInput(&input) {
                return Err(fail("AVAssetWriter", "cannot add an H.264 video input"));
            }
            writer.addInput(&input);
            if !writer.startWriting() {
                return Err(writer_error(&writer, "startWriting"));
            }
            writer.startSessionAtSourceTime(cm_time(Duration::ZERO));
            Ok(Self {
                writer,
                input,
                adaptor,
                width,
                height,
                path: path.to_path_buf(),
            })
        }
    }

    /// A pixel buffer from the adaptor's pool holding `frame`.
    fn pixel_buffer(
        &self,
        frame: &Frame,
    ) -> Result<objc2_core_foundation::CFRetained<CVPixelBuffer>> {
        // SAFETY: the pool exists once writing started; the new buffer is locked while its
        // memory is written and unlocked before it is returned. Rows are copied within the
        // buffer's bytes-per-row and the frame's own row length.
        unsafe {
            let pool = self
                .adaptor
                .pixelBufferPool()
                .ok_or_else(|| writer_error(&self.writer, "pixelBufferPool"))?;
            let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
            let status =
                CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut out));
            let buffer = NonNull::new(out)
                .filter(|_| status == kCVReturnSuccess)
                .ok_or_else(|| fail("CVPixelBufferPoolCreatePixelBuffer", status.to_string()))?;
            let buffer = objc2_core_foundation::CFRetained::from_raw(buffer);
            if CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags(0)) != kCVReturnSuccess
            {
                return Err(fail("CVPixelBufferLockBaseAddress", "could not lock"));
            }
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            let row = self.width as usize * 4;
            if !base.is_null() && stride >= row {
                for (y, src) in frame
                    .data
                    .chunks_exact(row)
                    .take(self.height as usize)
                    .enumerate()
                {
                    std::ptr::copy_nonoverlapping(src.as_ptr(), base.add(y * stride), row);
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags(0));
            if base.is_null() || stride < row {
                return Err(fail(
                    "CVPixelBufferGetBaseAddress",
                    "unexpected buffer layout",
                ));
            }
            Ok(buffer)
        }
    }
}

fn writer_error(writer: &AVAssetWriter, api: &'static str) -> RecordError {
    // SAFETY: plain getter.
    let message = unsafe { writer.error() }
        .map(|e| e.localizedDescription().to_string())
        .unwrap_or_else(|| "failed without an error".into());
    fail(api, message)
}

impl Encoder for AvEncoder {
    fn push(&mut self, frame: Frame, at: Duration) -> Result<()> {
        // In real-time mode the input may briefly refuse more data while the encoder catches
        // up; a frame that still cannot go in is dropped, as a live source would.
        let deadline = Instant::now() + READY_TIMEOUT;
        // SAFETY: plain getter.
        while !unsafe { self.input.isReadyForMoreMediaData() } {
            // SAFETY: plain getter.
            if unsafe { self.writer.status() } == AVAssetWriterStatus::Failed {
                return Err(writer_error(&self.writer, "AVAssetWriterInput"));
            }
            if Instant::now() > deadline {
                tracing::debug!("encoder busy; frame dropped");
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let buffer = self.pixel_buffer(&frame)?;
        // SAFETY: the input is ready, the time is numeric and after the previous frame's.
        if unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(&buffer, cm_time(at))
        } {
            Ok(())
        } else {
            Err(writer_error(&self.writer, "appendPixelBuffer"))
        }
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn finish(self: Box<Self>, end: Duration) -> Result<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        let done = RcBlock::new(move || {
            let _ = tx.send(());
        });
        // SAFETY: the session started at zero and `end` follows every frame; the completion
        // block only signals a channel, so it is fine to run on any queue.
        unsafe {
            self.input.markAsFinished();
            self.writer.endSessionAtSourceTime(cm_time(end));
            self.writer.finishWritingWithCompletionHandler(&done);
        }
        rx.recv_timeout(FINISH_TIMEOUT)
            .map_err(|_| fail("finishWriting", "timed out"))?;
        // SAFETY: plain getter.
        if unsafe { self.writer.status() } == AVAssetWriterStatus::Completed {
            Ok(())
        } else {
            Err(RecordError::Encoder {
                api: "finishWriting",
                message: format!(
                    "{}: {}",
                    self.path.display(),
                    writer_error(&self.writer, "finishWriting")
                ),
            })
        }
    }
}
