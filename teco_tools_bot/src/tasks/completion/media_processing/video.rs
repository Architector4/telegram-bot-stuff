use std::{path::Path, time::Duration};

use ffmpeg_next::{util::error::Error as FfmpegError, Rational};

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

    if base.denominator() == 0 {
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
    let mut input = ffmpeg_next::format::input(path)?;

    let parallelisms = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);

    let mut video_data =
        if let Some(best_video) = input.streams().best(ffmpeg_next::media::Type::Video) {
            // TODO: in codec_context, set skip_frame to yes?? we don't care about the pixel data
            let mut codec_context =
                ffmpeg_next::codec::context::Context::from_parameters(best_video.parameters())?;

            codec_context.set_threading(ffmpeg_next::threading::Config {
                kind: ffmpeg_next::threading::Type::Frame,
                count: parallelisms,
            });
            let video_decoder = codec_context.decoder().video()?;

            let video_frame = ffmpeg_next::util::frame::Video::empty();

            Some((best_video.index(), video_decoder, video_frame))
        } else {
            None
        };

    let mut audio_data =
        if let Some(best_audio) = input.streams().best(ffmpeg_next::media::Type::Audio) {
            let mut codec_context =
                ffmpeg_next::codec::context::Context::from_parameters(best_audio.parameters())?;

            codec_context.set_threading(ffmpeg_next::threading::Config {
                kind: ffmpeg_next::threading::Type::Frame,
                count: parallelisms,
            });
            let audio_decoder = codec_context.decoder().audio()?;

            let audio_frame = ffmpeg_next::util::frame::Audio::empty();

            Some((best_audio.index(), audio_decoder, audio_frame))
        } else {
            None
        };

    let mut frame_count = 0u64;
    let mut video_length_in_time_base = 0;
    let mut audio_length_in_time_base = 0;

    let mut drain_video_decoder = |packet: Option<&(
        ffmpeg_next::Stream<'_>,
        ffmpeg_next::Packet,
    )>|
     -> Result<(), FfmpegError> {
        if let Some((video_idx, decoder, frame)) = &mut video_data {
            // If there's a packet, feed it.
            if let Some((stream, packet)) = packet {
                if stream.index() == *video_idx {
                    decoder.send_packet(packet)?;

                    // While we're here, get the length stuff.
                    if let Some(pts) = packet.pts() {
                        let presentation_end = pts + packet.duration();

                        video_length_in_time_base = video_length_in_time_base.max(presentation_end);
                    }
                }
            } else {
                // No packet?? Buh-bye.
                decoder.send_eof()?;
            }

            loop {
                match decoder.receive_frame(frame) {
                    Ok(()) => {
                        // weow a frame!
                        frame_count += 1;

                        if let Some(pts) = frame.timestamp().or_else(|| frame.pts()) {
                            video_length_in_time_base = video_length_in_time_base.max(pts);
                        }
                    }
                    Err(FfmpegError::Other { errno: 11 }) | Err(FfmpegError::Eof) => {
                        // No more frames yet...
                        break;
                    }
                    Err(e) => Err(e)?,
                }
            }

            Ok(())
        } else {
            Ok(())
        }
    };

    let mut drain_audio_decoder = |packet: Option<&(
        ffmpeg_next::Stream<'_>,
        ffmpeg_next::Packet,
    )>|
     -> Result<(), FfmpegError> {
        if let Some((audio_idx, decoder, frame)) = &mut audio_data {
            // If there's a packet, feed it.
            if let Some((stream, packet)) = packet {
                if stream.index() == *audio_idx {
                    decoder.send_packet(packet)?;

                    // While we're here, get the length stuff.
                    if let Some(pts) = packet.pts() {
                        let presentation_end = pts + packet.duration();

                        audio_length_in_time_base = audio_length_in_time_base.max(presentation_end);
                    }
                }
            } else {
                // No packet?? Buh-bye.
                decoder.send_eof()?;
            }

            loop {
                match decoder.receive_frame(frame) {
                    Ok(()) => {
                        // weow an audio frame!
                        if let Some(pts) = frame.timestamp().or_else(|| frame.pts()) {
                            audio_length_in_time_base = audio_length_in_time_base.max(pts);
                        }
                    }
                    Err(FfmpegError::Other { errno: 11 }) | Err(FfmpegError::Eof) => {
                        // No more frames yet...
                        break;
                    }
                    Err(e) => Err(e)?,
                }
            }

            Ok(())
        } else {
            Ok(())
        }
    };

    for data in input.packets() {
        drain_video_decoder(Some(&data))?;
        drain_audio_decoder(Some(&data))?;
    }

    drain_video_decoder(None)?;
    drain_audio_decoder(None)?;

    let video_length = if let Some((video_idx, _, _)) = video_data {
        let stream = input
            .stream(video_idx)
            .expect("Video stream should be present");

        time_base_to_duration(stream.time_base(), video_length_in_time_base)
    } else {
        Duration::ZERO
    };

    let audio_length = if let Some((audio_idx, _, _)) = audio_data {
        let stream = input
            .stream(audio_idx)
            .expect("Video stream should be present");

        time_base_to_duration(stream.time_base(), audio_length_in_time_base)
    } else {
        Duration::ZERO
    };

    Ok(MediaMetadata {
        frame_count,
        frame_rate: frame_count as f64 / video_length.as_secs_f64(),
        video_length,
        audio_length,
    })
}
