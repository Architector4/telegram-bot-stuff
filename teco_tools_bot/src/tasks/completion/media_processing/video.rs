use std::{path::Path, time::Duration};

use ffmpeg_next::{util::error::Error as FfmpegError, Rational};

#[derive(Debug)]
pub struct MediaMetadata {
    /// Count of frames in the video stream. Zero if no video.
    pub frame_count: u64,
    /// Frame count divided by video length, producing frames per second.
    pub frame_rate: f64,
    /// Length of the video stream, or [`Duration::ZERO`] if no video. Specifically, the
    /// presentation time of the last frame plus its duration.
    pub video_length: Duration,
    /// Length of the audio stream, or [`Duration::ZERO`] if no audio.
    pub audio_length: Duration,
}

/// Given a time base (representing how long is a step relative to a second), and a value (a step),
/// compute and return [`Duration`].
fn time_base_to_duration(base: Rational, value: i64) -> Duration {
    const NANOS_PER_SEC: u128 = 1_000_000_000;

    if base.numerator() == 0 || base.denominator() == 0 {
        // fuck off lmao
        return Duration::ZERO;
    }

    let negative_base = (base.numerator() < 0) != (base.denominator() < 0);

    if negative_base != (value < 0) {
        // Value is below zero. Just assume zero.
        return Duration::ZERO;
    }

    let value = value.unsigned_abs() as u128;
    // Multiply by NANOS_PER_SEC to ensure precision lol
    let numerator = base.numerator().unsigned_abs() as u128 * value * NANOS_PER_SEC;
    let denominator = base.denominator().unsigned_abs() as u128;

    let nanos = numerator / denominator;

    if nanos > Duration::MAX.as_nanos() {
        // Saturate on overflow lol
        Duration::MAX
    } else {
        Duration::from_nanos_u128(numerator / denominator)
    }
}

pub fn get_media_metadata(path: &Path) -> Result<MediaMetadata, FfmpegError> {
    let mut video_length = Duration::ZERO;
    let mut audio_length = Duration::ZERO;
    let mut frame_count = 0;

    let mut stream = MediaStream::new(path)?;

    while let Some(data) = stream.next_frame() {
        let data = data?;

        match data.data {
            VideoOrAudioFrame::Video(_) => {
                if let Some(start) = data.approx_presentation_start {
                    video_length = video_length.max(start);
                }

                if let Some(end) = data.approx_presentation_end {
                    video_length = video_length.max(end);
                }

                frame_count += 1;
            }
            VideoOrAudioFrame::Audio(_) => {
                if let Some(start) = data.approx_presentation_start {
                    audio_length = audio_length.max(start);
                }

                if let Some(end) = data.approx_presentation_end {
                    audio_length = audio_length.max(end);
                }
            }
        }
    }

    Ok(MediaMetadata {
        frame_count,
        frame_rate: frame_count as f64 / video_length.as_secs_f64(),
        video_length,
        audio_length,
    })
}

/// A media stream object that can output audio frames or video frames.
pub struct MediaStream {
    input: ffmpeg_next::format::context::Input,
    video_frame: ffmpeg_next::util::frame::Video,
    audio_frame: ffmpeg_next::util::frame::Audio,
    best_video_stream_index: usize,
    best_audio_stream_index: usize,
    video_decoder: Option<ffmpeg_next::decoder::Video>,
    audio_decoder: Option<ffmpeg_next::decoder::Audio>,
    video_last_presentation_end: Option<i64>,
    audio_last_presentation_end: Option<i64>,
    video_time_base: Rational,
    audio_time_base: Rational,
    sent_eof: bool,
}

impl MediaStream {
    pub fn new(path: &Path) -> Result<Self, FfmpegError> {
        let input = ffmpeg_next::format::input(path)?;

        let parallelisms = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1);

        let video_frame = ffmpeg_next::util::frame::Video::empty();
        let audio_frame = ffmpeg_next::util::frame::Audio::empty();

        let (best_video_stream_index, video_decoder, video_time_base) =
            if let Some(best_video) = input.streams().best(ffmpeg_next::media::Type::Video) {
                let mut codec_context =
                    ffmpeg_next::codec::context::Context::from_parameters(best_video.parameters())?;

                codec_context.set_threading(ffmpeg_next::threading::Config {
                    kind: ffmpeg_next::threading::Type::Frame,
                    count: parallelisms,
                });
                let video_decoder = codec_context.decoder().video()?;

                (
                    best_video.index(),
                    Some(video_decoder),
                    best_video.time_base(),
                )
            } else {
                (usize::MAX, None, Rational::new(0, 0))
            };

        let (best_audio_stream_index, audio_decoder, audio_time_base) =
            if let Some(best_audio) = input.streams().best(ffmpeg_next::media::Type::Audio) {
                let mut codec_context =
                    ffmpeg_next::codec::context::Context::from_parameters(best_audio.parameters())?;

                codec_context.set_threading(ffmpeg_next::threading::Config {
                    kind: ffmpeg_next::threading::Type::Frame,
                    count: parallelisms,
                });
                let audio_decoder = codec_context.decoder().audio()?;

                (
                    best_audio.index(),
                    Some(audio_decoder),
                    best_audio.time_base(),
                )
            } else {
                (usize::MAX, None, Rational::new(0, 0))
            };

        Ok(Self {
            input,
            video_frame,
            audio_frame,
            best_video_stream_index,
            best_audio_stream_index,
            video_decoder,
            audio_decoder,
            video_last_presentation_end: None,
            audio_last_presentation_end: None,
            video_time_base,
            audio_time_base,
            sent_eof: false,
        })
    }
}

/// Either a video or an audio frame.
pub enum VideoOrAudioFrame<'a> {
    #[allow(unused)]
    Video(&'a ffmpeg_next::util::frame::Video),
    #[allow(unused)]
    Audio(&'a ffmpeg_next::util::frame::Audio),
}

impl std::fmt::Debug for VideoOrAudioFrame<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Video(_) => "Video frame".fmt(f),
            Self::Audio(_) => "Audio frame".fmt(f),
        }
    }
}

#[derive(Debug)]
pub struct MediaStreamFrame<'a> {
    /// The frame itself.
    pub data: VideoOrAudioFrame<'a>,
    /// Approximate timestamp into the media where this frame should start being presented.
    pub approx_presentation_start: Option<Duration>,
    /// Approximate timestamp into the media where this frame should stop being presented. Might be
    /// a number too big for this frame; only really reliable for the last frame.
    pub approx_presentation_end: Option<Duration>,
}

impl MediaStream {
    /// Get the next frame.
    ///
    /// This is not an iterator because it returns a reference to itself to avoid extraneous
    /// allocation. I could fix this with a separate wrapper iterator type to sort this out, but
    /// bleh.
    pub fn next_frame(&mut self) -> Option<Result<MediaStreamFrame<'_>, FfmpegError>> {
        macro_rules! unfail {
            ($thing: expr) => {{
                match $thing {
                    Ok(x) => x,
                    Err(e) => {
                        return Some(Err(e));
                    }
                }
            }};
        }

        // Loop forever until we find something to output.
        loop {
            if let Some(decoder) = &mut self.video_decoder {
                match decoder.receive_frame(&mut self.video_frame) {
                    Ok(()) => {
                        // weow a frame!

                        let approx_presentation_start = self
                            .video_frame
                            .timestamp()
                            .or_else(|| self.video_frame.pts())
                            .map(|pts| time_base_to_duration(self.video_time_base, pts));

                        let approx_presentation_end = self
                            .video_last_presentation_end
                            .map(|pts| time_base_to_duration(self.video_time_base, pts));

                        return Some(Ok(MediaStreamFrame {
                            data: VideoOrAudioFrame::Video(&self.video_frame),
                            approx_presentation_start,
                            approx_presentation_end,
                        }));
                    }
                    Err(FfmpegError::Other { errno: 11 }) | Err(FfmpegError::Eof) => {
                        // No more frames yet, just fall over...
                    }
                    Err(e) => return Some(Err(e)),
                }
            }

            if let Some(decoder) = &mut self.audio_decoder {
                match decoder.receive_frame(&mut self.audio_frame) {
                    Ok(()) => {
                        // weow a frame!

                        let approx_presentation_start = self
                            .audio_frame
                            .timestamp()
                            .or_else(|| self.audio_frame.pts())
                            .map(|pts| time_base_to_duration(self.audio_time_base, pts));

                        let approx_presentation_end = self
                            .audio_last_presentation_end
                            .map(|pts| time_base_to_duration(self.audio_time_base, pts));

                        return Some(Ok(MediaStreamFrame {
                            data: VideoOrAudioFrame::Audio(&self.audio_frame),
                            approx_presentation_start,
                            approx_presentation_end,
                        }));
                    }
                    Err(FfmpegError::Other { errno: 11 }) | Err(FfmpegError::Eof) => {
                        // No more frames yet, just fall over...
                    }
                    Err(e) => return Some(Err(e)),
                }
            }

            // None of the two decoders had frames. Try to pull from input?

            let Some((stream, packet)) = self.input.packets().next() else {
                // Weow! Nothing ever!
                if self.sent_eof {
                    // We're done here. Buh-bye!
                    return None;
                } else {
                    if let Some(decoder) = &mut self.video_decoder {
                        unfail!(decoder.send_eof());
                    }
                    if let Some(decoder) = &mut self.audio_decoder {
                        unfail!(decoder.send_eof());
                    }

                    self.sent_eof = true;

                    // We sent EOFs. Now do draining.
                    continue;
                }
            };

            if stream.index() == self.best_video_stream_index {
                if let Some(decoder) = &mut self.video_decoder {
                    unfail!(decoder.send_packet(&packet));

                    // While we're here, get the length stuff.
                    if let Some(pts) = packet.pts() {
                        let presentation_end = pts + packet.duration();

                        self.video_last_presentation_end = Some(presentation_end);
                    }
                }
            }

            if stream.index() == self.best_audio_stream_index {
                if let Some(decoder) = &mut self.audio_decoder {
                    unfail!(decoder.send_packet(&packet));

                    // While we're here, get the length stuff.
                    if let Some(pts) = packet.pts() {
                        let presentation_end = pts + packet.duration();

                        self.audio_last_presentation_end = Some(presentation_end);
                    }
                }
            }

            // If it's some other kind of random goofy ahh stream, we don't care. ✨
        }
    }
}
