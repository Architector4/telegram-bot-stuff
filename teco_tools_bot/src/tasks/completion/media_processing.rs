#![allow(clippy::manual_clamp)] // It's better here since it also gets rid of NaN

use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::Path,
    process::{ChildStdout, Command, Stdio},
    sync::OnceLock,
    time::Duration,
};

use tokio::sync::watch::Sender;

use magick_rust::{AlphaChannelOption, FilterType, MagickError, MagickWand, PixelWand};
use regex::Regex;
use tempfile::NamedTempFile;

use crate::tasks::{ImageFormat, ResizeCurve, ResizeType};

/// Will error if [`ImageFormat::Preserve`] is sent.
#[allow(clippy::too_many_arguments)]
pub fn resize_image(
    data: &[u8],
    width: isize,
    height: isize,
    rotation: f64,
    resize_type: ResizeType,
    format: ImageFormat,
    // Width, height, and if the resulting image should be stretched to
    // output size instead of fitting.
    output_size: Option<(usize, usize, bool)>,
    crop_rotation: bool,
) -> Result<Vec<u8>, MagickError> {
    if format == ImageFormat::Preserve {
        // yeah this isn't a MagickError, but we'd get one in the last line
        // anyways, so might as well make a better description for ourselves lol
        return Err(MagickError(
            "ImageFormat::Preserve was specified".to_string(),
        ));
    }

    let wand = MagickWand::new();

    wand.read_image_blob(data)?;

    let mut transparent = PixelWand::new();
    transparent.set_alpha(0.0);
    wand.set_image_background_color(&transparent)?;

    // Record and sanitize signs...
    let width_is_negative = width.is_negative();
    let height_is_negative = height.is_negative();
    // Avoid resizing to zero.
    let width = width.unsigned_abs().max(1);
    let height = height.unsigned_abs().max(1);

    let iwidth = wand.get_image_width();
    let iheight = wand.get_image_height();

    // The second and third arguments are "delta_x" and "rigidity"
    // This library doesn't document them, but another bindings
    // wrapper does: https://docs.wand-py.org/en/latest/wand/image.html
    //
    // According to it:
    // > delta_x (numbers.Real) – maximum seam transversal step. 0 means straight seams. default is 0
    // (but delta_x of 0 is very boring so we will pretend the default is 1)
    // > rigidity (numbers.Real) – introduce a bias for non-straight seams. default is 0

    // Also delta_x less than 0 segfaults. Other code prevents that from getting
    // here, but might as well lol
    // And both values in extremely high amounts segfault too, it seems lol

    match resize_type {
        ResizeType::SeamCarve { delta_x, rigidity } => {
            // The point of seam carving in this bot is to be a "distortion"
            // effect, with the intent of looking funny.
            //
            // However, in ImageMagick's seam carving algorithm used below,
            // if you apply integer scaling to the image
            // (i.e. resize it to 200% x 200% of the original resolution),
            // it seems to pretty much result in a boring, normal resize.
            //
            // However, non-integer scaling, like 150% x 150%,
            // gives the most extreme distortion effects.
            //
            // The loop below repeatedly checks the size and upscales at most
            // by 150% x 150% of the original size every time, compounding
            // the distortion up until the target is reached.

            loop {
                let current_width = wand.get_image_width();
                let current_height = wand.get_image_height();

                if current_width <= 1 || current_height <= 1 {
                    // ImageMagick is likely to abort/segfault in this situation.
                    // Switch up resize type.
                    wand.resize_image(width, height, FilterType::Lagrange)?;
                    break;
                }
                wand.liquid_rescale_image(
                    width.min(current_width + current_width / 2),
                    height.min(current_height + current_height / 2),
                    delta_x.abs().min(4.0),
                    rigidity.max(-4.0).min(4.0),
                )?;

                if wand.get_image_width() == width && wand.get_image_height() == height {
                    break;
                }
            }
        }
        ResizeType::Stretch => {
            wand.resize_image(width, height, FilterType::Lagrange)?;
        }
        ResizeType::Fit | ResizeType::ToSticker => {
            wand.fit(width, height);
        }
        ResizeType::Crop | ResizeType::ToCustomEmoji => {
            // We want to scale the image so that it completely covers the area,
            // where at least one dimension is exactly as big,
            // and then crop the other dimension.
            //

            // If we imagine it's width...
            let size_matching_width = (
                width, // == (iwidth * width) / iwidth
                (iheight * width) / iwidth,
            );
            // If we imagine it's height...
            let size_matching_height = (
                (iwidth * height) / iheight,
                height, // == (iheight * height) / iheight
            );

            // Pick the biggest.
            let mut size_pre_crop = if size_matching_width.0 > size_matching_height.0
                || size_matching_width.1 > size_matching_height.1
            {
                size_matching_width
            } else {
                size_matching_height
            };

            // A bit of a safeguard. I don't want to hold images this big
            // in memory lol
            if size_pre_crop.0 > 16384 {
                size_pre_crop.0 = 16384;
                size_pre_crop.1 = (size_pre_crop.1 * size_pre_crop.0) / 16384;
            }
            if size_pre_crop.1 > 16384 {
                size_pre_crop.0 = (size_pre_crop.0 * size_pre_crop.1) / 16384;
                size_pre_crop.1 = 16384;
            }

            // Resize to desired size... Yes, this may stretch, but that's better
            // since then we keep the exact end size, and the crop below
            // will not fail then lol
            wand.resize_image(size_pre_crop.0, size_pre_crop.1, FilterType::Lagrange)?;

            // Now crop the result to desired size.
            wand.crop_image(
                width,
                height,
                ((size_pre_crop.0 - width) / 2) as isize,
                ((size_pre_crop.1 - height) / 2) as isize,
            )?;
            wand.reset_image_page("")?;
        }
    }

    // Flip it according to the signs.
    if width_is_negative {
        wand.flop_image()?;
    }
    if height_is_negative {
        wand.flip_image()?;
    }

    if let Some(output_size) = output_size {
        // Apply output size.
        if output_size.2 {
            wand.resize_image(output_size.0, output_size.1, FilterType::Lagrange)?;
        } else {
            wand.fit(output_size.0, output_size.1);

            let pre_extend_width = wand.get_image_width();
            let pre_extend_height = wand.get_image_height();
            wand.extend_image(
                output_size.0,
                output_size.1,
                (pre_extend_width as isize - output_size.0 as isize) / 2,
                (pre_extend_height as isize - output_size.1 as isize) / 2,
            )?;
        }
    }

    if rotation.signum() % 360.0 != 0.0 {
        if format.supports_alpha_transparency()
            && rotation.signum() % 90.0 != 0.0
            && !wand.get_image_alpha_channel()
        {
            // No alpha channel, but output format supports it, and
            // we are rotating by an angle that will add empty space.
            // Add alpha channel.
            wand.set_image_alpha_channel(AlphaChannelOption::On)?;
        }

        let pre_rotation_width = wand.get_image_width();
        let pre_rotation_height = wand.get_image_height();

        wand.rotate_image(&transparent, rotation)?;
        // Image page data is inconsistent after rotations (when divisible by 90 degrees),
        // so reset it.
        wand.reset_image_page("")?;

        if crop_rotation {
            // If we want cropping after rotation, do the cropping.
            // This also means extending the image so that it fits the old
            // resolution *exactly*, not bigger, not smaller.

            // Crop it to the middle.
            wand.crop_image(
                pre_rotation_width,
                pre_rotation_height,
                (wand.get_image_width() as isize - pre_rotation_width as isize) / 2,
                (wand.get_image_height() as isize - pre_rotation_height as isize) / 2,
            )?;
            // Extend it, with the new image placed in the middle.
            wand.extend_image(
                pre_rotation_width,
                pre_rotation_height,
                (wand.get_image_width() as isize - pre_rotation_width as isize) / 2,
                (wand.get_image_height() as isize - pre_rotation_height as isize) / 2,
            )?;
        }
    }

    wand.write_image_blob(format.as_str())
}

struct SplitIntoBmps<T: Read> {
    item: T,
    buffer: Vec<u8>,
}

impl<T: Read> SplitIntoBmps<T> {
    pub fn new(item: T) -> SplitIntoBmps<T> {
        SplitIntoBmps {
            item,
            // Somewhere about enough to fit a 2048x204832-bits-per-pixel image
            // plus 4MB of other whiff.
            buffer: Vec::with_capacity(2048 * 2048 * 5),
        }
    }
}

impl<T: Read> Iterator for SplitIntoBmps<T> {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn next(&mut self) -> Option<Self::Item> {
        macro_rules! unfail_read_exact {
            ($thing: expr) => {
                match $thing {
                    Ok(o) => o,
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
                    Err(e) => return Some(Err(e)),
                }
            };
        }

        // We are at a boundary between BMPs.
        // Next byte read will be the first one of a BMP image.
        // First two bytes will be the "BM" marker,
        // then 4 bytes would be the file size in little endian.

        self.buffer.clear();
        self.buffer.resize(6, 0u8);

        unfail_read_exact!(self.item.read_exact(&mut self.buffer[0..6]));

        let bmp_length = u32::from_le_bytes(
            self.buffer[2..6]
                .try_into()
                .expect("Incorrect slice length... somehow."),
        ) as usize;

        if self.buffer[0..2] != [0x42, 0x4D] || bmp_length <= 6 {
            return Some(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid BMP header",
            )));
        }

        // Read exactly the rest of the file.
        self.buffer.resize(bmp_length, 0u8);
        unfail_read_exact!(self.item.read_exact(&mut self.buffer[6..bmp_length]));

        Some(Ok(self.buffer.clone()))
    }
}

fn get_bmp_width_height(buffer: &[u8]) -> Option<(isize, isize)> {
    // Based on data from:
    // http://www.dragonwins.com/domains/getteched/bmp/bmpfileformat.htm#The%20Image%20Header

    // Check if this has a BMP header.
    if buffer[0..2] != [0x42, 0x4D] {
        return None;
    }

    if buffer.len() < 22 + 4 {
        return None;
    }

    let width = u32::from_le_bytes(buffer[18..18 + 4].try_into().unwrap());
    let height = i32::from_le_bytes(buffer[22..22 + 4].try_into().unwrap());
    let height = height.unsigned_abs();

    Some((width as isize, height as isize))
}

pub fn count_video_frames_and_framerate_and_audio_and_length(
    path: &std::path::Path,
) -> Result<(u64, f64, bool, Duration), std::io::Error> {
    macro_rules! goodbye {
        ($desc: expr) => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, $desc))
        };
    }

    let counter = Command::new("ffmpeg")
        .args([
            OsStr::new("-stats"),
            OsStr::new("-i"),
            path.as_ref(),
            OsStr::new("-vsync"),
            OsStr::new("passthrough"),
            OsStr::new("-f"),
            OsStr::new("null"),
            OsStr::new("-"),
        ])
        .stderr(Stdio::piped())
        .spawn()?;

    let output = counter.wait_with_output()?;
    let Ok(output) = String::from_utf8(output.stderr) else {
        goodbye!("Frame counter returned non UTF-8 response");
    };

    // Output may be in a format like
    // ...
    // (OPTIONAL)  Stream #0:1(eng): Audio: pcm_s16le, 44100 Hz, stereo, s16, 1411 kb/s (default)
    // ...
    // frame= 2280 fps=0.0 q=-0.0 Lsize=N/A time=00:38:00.00 bitrate=N/A speed=3.19e+04x
    // Whitespace after "frame=" is not guaranteed
    //
    // If input is audio only, the first 'frame = 2280' field will not be present.

    let audio_stream_regex = Regex::new(r" *Stream .*: Audio:.*").unwrap();

    let has_audio = audio_stream_regex.is_match(&output);

    let frame_regex = Regex::new(r"frame= *(\d+).*").unwrap();
    let time_regex = Regex::new(r".*time=(\d+):(\d+):(\d+)\.(\d+).*").unwrap();

    let Some(last_line) = output.lines().last() else {
        goodbye!("Frame counter returned no output");
    };

    let frame_count = if let Some(frame_capture) = frame_regex.captures(last_line) {
        let Ok(frame_count): Result<u64, _> = frame_capture[1].parse() else {
            goodbye!("Failed to parse frame count");
        };
        frame_count
    } else {
        0
    };

    let Some(time_captures) = time_regex.captures(last_line) else {
        goodbye!("Frame counter returned an invalid response");
    };

    assert_eq!(time_captures.len(), 5);

    let Ok(hours): Result<u64, _> = time_captures[1].parse() else {
        goodbye!("Failed to parse hours in length");
    };

    let Ok(minutes): Result<u64, _> = time_captures[2].parse() else {
        goodbye!("Failed to parse minutes in length");
    };

    let Ok(seconds): Result<u64, _> = time_captures[3].parse() else {
        goodbye!("Failed to parse seconds in length");
    };

    let Ok(centiseconds): Result<u64, _> = time_captures[4].parse() else {
        goodbye!("Failed to parse centiseconds in length");
    };

    let length: Duration = Duration::from_millis(10 * centiseconds)
        + Duration::from_secs(seconds)
        + Duration::from_secs(minutes * 60)
        + Duration::from_secs(hours * 60 * 60);

    let framerate = frame_count as f64 / length.as_secs_f64();

    Ok((frame_count, framerate, has_audio, length))
}

fn approx_same_aspect_ratio(
    (input_width, input_height): (f64, f64),
    (end_width, end_height): (f64, f64),
) -> bool {
    let input_aspect_ratio = input_width / input_height;
    let end_aspect_ratio = end_width / end_height;

    // Now, compute the stretch we'd have on the smallest width and height
    // of the two if we were to correct from one aspect ratio to another.
    let smallest_width = input_width.min(end_width);
    let smallest_width_corrected = smallest_width * input_aspect_ratio / end_aspect_ratio;
    let smallest_height = input_height.min(end_height);
    let smallest_height_corrected = smallest_height * input_aspect_ratio / end_aspect_ratio;

    // If both of them don't change significantly, then we can just stretch.

    let width_diff_is_insignificant = f64::abs(smallest_width_corrected - smallest_width) < 1.5;
    let height_diff_is_insignificant = f64::abs(smallest_height_corrected - smallest_height) < 1.5;

    width_diff_is_insignificant && height_diff_is_insignificant
}

#[allow(clippy::too_many_arguments)]
pub fn resize_video(
    status_report: Sender<String>,
    inputfile: &Path,
    (width, height): (isize, isize),
    rotation: f64,
    resize_type: ResizeType,
    strip_audio: bool,
    vibrato_hz: f64,
    vibrato_depth: f64,
    input_dimensions: (u32, u32),
    resize_curve: ResizeCurve,
) -> Result<Vec<u8>, String> {
    macro_rules! unfail {
        ($thing: expr) => {
            match $thing {
                Ok(o) => o,
                Err(e) => return Err(e.to_string()),
            }
        };
    }

    // First, compute some stuff that will be in use during resizing.

    // If we resize every frame to a different size, we want to scale them
    // back to original size, so that the video would be
    // of constant actual size that will preserve the biggest frame.
    let is_curved = resize_curve != ResizeCurve::Constant;
    let (mut output_width, mut output_height) = if is_curved {
        (
            (input_dimensions.0 as usize).max(width.unsigned_abs()),
            (input_dimensions.1 as usize).max(height.unsigned_abs()),
        )
    } else {
        (width.unsigned_abs(), height.unsigned_abs())
    };

    // h264 needs output dimensions divisible by 2; make absolutely sure we do that.
    output_width += output_width % 2;
    output_height += output_height % 2;

    // Now, if the size is dynamic, we want to know if we want to
    // stretch every frame to the output dimensions, or just fit them.
    // This is needed because neither are universally appropriate.
    //
    // Stretching each frame is not expected behavior when input
    // and output sizes have vastly different aspect ratios.
    //
    // Fitting each frame brings an issue when the input and output
    // sizes are intended to have the same aspect ratio, but don't
    // due to having to be defined by integer numbers. In these
    // cases, the very slight mismatch causes black bars on edges
    // of the image to pop in and out.
    let stretch_to_output_size = if is_curved {
        if width < 0 || height < 0 {
            // If target is flipped or flopped or both,
            // don't stretch.
            false
        } else {
            let input_width = input_dimensions.0 as f64;
            let input_height = input_dimensions.1 as f64;

            // "max" to avoid inf/NaN values
            let end_width = (width as f64).max(1.0);
            let end_height = (height as f64).max(1.0);

            approx_same_aspect_ratio((input_width, input_height), (end_width, end_height))
        }
    } else {
        false
    };

    let _ = status_report.send("Creating temp files...".to_string());

    let outputfile = unfail!(NamedTempFile::new());

    let _ = status_report.send("Counting frames...".to_string());

    let (input_frame_count, input_frame_rate, has_audio, _input_length) = unfail!(
        count_video_frames_and_framerate_and_audio_and_length(inputfile)
    );

    let decoder = Command::new("ffmpeg")
        .args([
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-i"),
            inputfile.as_ref(),
            OsStr::new("-c:v"),
            OsStr::new("bmp"),
            OsStr::new("-vsync"),
            OsStr::new("passthrough"),
            OsStr::new("-f"),
            OsStr::new("image2pipe"),
            OsStr::new("-"),
        ])
        .stdout(Stdio::piped())
        .spawn();
    let mut decoder = unfail!(decoder);
    let decoder_stdout = decoder.stdout.take().unwrap();

    let converted_image_stream = SplitIntoBmps::<ChildStdout>::new(decoder_stdout)
        .enumerate()
        .map(|(count, frame)| match frame {
            Ok(frame) => {
                let curved_width = resize_curve.apply_resize_for(
                    count,
                    input_frame_count,
                    input_dimensions.0 as f64,
                    width as f64,
                );
                let curved_height = resize_curve.apply_resize_for(
                    count,
                    input_frame_count,
                    input_dimensions.1 as f64,
                    height as f64,
                );
                let curved_rotation =
                    resize_curve.apply_resize_for(count, input_frame_count, 0.0, rotation);

                // Check if this operation changes the image at all.
                // If the dimensions (both target and output) and rotation
                // are the same, it doesn't.
                let input_dimensions = get_bmp_width_height(&frame);
                let resize_result = if rotation.abs() == 0.0
                    && input_dimensions == Some((output_width as isize, output_height as isize))
                    && input_dimensions == Some((curved_width as isize, curved_height as isize))
                {
                    // It doesn't. Just return the same buffer directly.
                    Ok(frame)
                } else {
                    resize_image(
                        &frame,
                        curved_width as isize,
                        curved_height as isize,
                        curved_rotation,
                        resize_type,
                        ImageFormat::Bmp,
                        Some((output_width, output_height, stretch_to_output_size)),
                        is_curved, // Prevent bounds bouncing.
                    )
                };

                match resize_result {
                    Ok(resize) => Ok((count, resize)),
                    Err(e) => Err(std::io::Error::other(e)),
                }
            }
            Err(e) => Err(e),
        });

    let _ = status_report.send("Initializing encoder...".to_string());

    let encoder = Command::new("ffmpeg")
        .args([
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-framerate"),
            OsStr::new(input_frame_rate.to_string().as_str()),
            OsStr::new("-i"),
            OsStr::new("-"),
            OsStr::new("-vf"), // Pad uneven pixels with black.
            OsStr::new("pad=ceil(iw/2)*2:ceil(ih/2)*2"),
            // I'd prefer the crop filter instead, but it leaves
            // a chance of cropping to 0 width/height and stuff breaking :(
            //OsStr::new("crop=trunc(iw/2)*2:trunc(ih/2)*2"),
            OsStr::new("-pix_fmt"),
            OsStr::new("yuv420p"),
            OsStr::new("-f"),
            OsStr::new("mp4"),
            outputfile.path().as_os_str(),
        ])
        .stdin(Stdio::piped())
        .spawn();
    let mut encoder = unfail!(encoder);
    let mut encoder_stdin = encoder.stdin.take().unwrap();

    let mut writing_stream = converted_image_stream.map(|frame| match frame {
        Ok(frame) => {
            encoder_stdin.write_all(frame.1.as_slice())?;
            if input_frame_count != 0 {
                let _ = status_report.send(format!("Frame {} / {}", frame.0, input_frame_count));
            } else {
                let _ = status_report.send(format!("Frame {}", frame.0));
            }

            Ok(())
        }
        Err(e) => Err(e),
    });

    // Try to find an error and fail on it, if any lol
    if let Some(err) = writing_stream.find(Result::is_err) {
        unfail!(err);
    }

    let _ = status_report.send("Finalizing...".to_string());

    drop(encoder_stdin);
    unfail!(decoder.wait());
    unfail!(encoder.wait());

    let mut finalfile = if has_audio && !strip_audio {
        let _ = status_report.send("Writing audio...".to_string());
        // Now to transfer audio... This means we need a THIRD file to put the final result into.
        let muxfile = unfail!(NamedTempFile::new());
        // Will exclude cases of 0.0, -0.0, and all the NaNs and infinities
        let distort_audio = vibrato_hz.is_normal()
            && vibrato_depth.is_normal()
            && vibrato_hz >= 0.1
            && vibrato_depth > 0.0;

        let mut args = vec![
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-i"),
            inputfile.as_ref(),
            OsStr::new("-i"),
            outputfile.path().as_ref(),
            OsStr::new("-c:v"),
            OsStr::new("copy"),
            OsStr::new("-map"),
            OsStr::new("1:v:0"),
            OsStr::new("-map"),
            OsStr::new("0:a:0"),
        ];

        let mut vibrato_str_temp: String = String::new();

        if distort_audio {
            args.push(OsStr::new("-af"));

            let mut vibrato_depth_left = vibrato_depth;
            while vibrato_depth_left > 0.0 {
                use std::fmt::Write;
                write!(
                    vibrato_str_temp,
                    "vibrato=f={}:d={},aformat=s16p,",
                    vibrato_hz.min(20000.0),
                    vibrato_depth.min(1.0)
                )
                .expect("this literally cannot panic");

                vibrato_depth_left -= 1.0;
            }

            args.push(vibrato_str_temp.as_ref());
        }

        args.extend_from_slice(&[
            OsStr::new("-f"),
            OsStr::new("mp4"),
            OsStr::new("-preset"),
            OsStr::new("slow"),
            muxfile.path().as_ref(),
        ]);

        let audiomuxer = Command::new("ffmpeg").args(args).spawn();

        unfail!(unfail!(audiomuxer).wait());

        muxfile
    } else {
        outputfile
    };

    unfail!(finalfile.reopen());

    let mut output = Vec::new();
    unfail!(finalfile.read_to_end(&mut output));

    Ok(output)
}

pub fn ocr_image(data: &[u8]) -> Result<String, MagickError> {
    // Use ImageMagick to normalize colors and export to PNG,
    // which Tesseract can read.
    let wand = MagickWand::new();
    wand.read_image_blob(data)?;
    wand.normalize_image()?;

    let data = wand.write_image_blob("png")?;

    let mut result = String::new();

    fn tesseract_it(data: &[u8], buffer: &mut String, grab_all_text: bool) {
        let args_grab_all = &[
            OsStr::new("--psm"),
            // PSM mode 12's name sounds more attractive than 11,
            // but in my experience it produces worse results.
            OsStr::new("11"),
            OsStr::new("-"),
            OsStr::new("-"),
        ];
        let args_default = &[OsStr::new("-"), OsStr::new("-")];

        let args = if grab_all_text {
            &args_grab_all[..]
        } else {
            &args_default[..]
        };

        let tesseract = Command::new("tesseract")
            .args(args)
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::null()) // Tesseract is noisy and I don't want to make a config file lol
            .spawn()
            .expect("Spawning tesseract failed!");

        tesseract
            .stdin
            .unwrap()
            .write_all(data)
            .expect("Failed sending image to Tesseract!");

        tesseract
            .stdout
            .unwrap()
            .read_to_string(buffer)
            .expect("Failed reading Tesseract's output!");

        // Postprocess the text...
        let new = {
            static OCR_REGEX_1: OnceLock<Regex> = OnceLock::new();
            static OCR_REGEX_2: OnceLock<Regex> = OnceLock::new();
            let regex_1 = OCR_REGEX_1.get_or_init(|| Regex::new(r#"[ \t]{2,}"#).unwrap());
            let regex_2 = OCR_REGEX_2.get_or_init(|| Regex::new(r#"\s\s\s+"#).unwrap());

            let stripped = regex_1.replace_all(buffer, " ");
            let stripped = regex_2.replace_all(&stripped, "\n");

            let stripped = stripped.trim();

            // Count amount of empty lines and non-empty lines.
            let (empty_lines, non_empty_lines) =
                stripped
                    .split('\n')
                    .fold((0usize, 0usize), |(empty, non_empty), line| {
                        if line.len() < 4 {
                            // Count lines with less than 4 bytes as empty.
                            (empty + 1, non_empty)
                        } else {
                            (empty, non_empty + 1)
                        }
                    });

            // If there's more empty lines than non-empty, get rid of them.
            if empty_lines >= non_empty_lines {
                stripped.replace("\n\n", "\n")
            } else {
                stripped.to_string()
            }
        };
        buffer.clear();
        buffer.push_str(&new);
    }

    tesseract_it(&data, &mut result, false);

    if result.is_empty() {
        tesseract_it(&data, &mut result, true);
    }

    Ok(result)
}

pub fn amen_break_video(
    status_report: Sender<String>,
    inputfile: &Path,
) -> Result<Vec<u8>, String> {
    macro_rules! unfail {
        ($thing: expr) => {
            match $thing {
                Ok(o) => o,
                Err(e) => return Err(e.to_string()),
            }
        };
    }

    let _ = status_report.send("Creating temp files...".to_string());
    let mut outputfile = unfail!(NamedTempFile::new());

    let _ = status_report.send("Choosing an amen break...".to_string());
    let count = unfail!(std::fs::read_dir("amen-breaks")).count();
    let mut rng = rand::thread_rng();
    use rand::Rng;
    let which_to_pick = rng.gen_range(0..count);
    let Some(the_break) = unfail!(std::fs::read_dir("amen-breaks")).nth(which_to_pick) else {
        return Err("Failed to pick an amen break!".to_string());
    };

    let break_path = unfail!(the_break).path();

    let _ = status_report.send("Checking amen break length".to_string());
    let (_input_frame_count, _input_frame_rate, _has_audio, amen_break_length) = unfail!(
        count_video_frames_and_framerate_and_audio_and_length(&break_path)
    );

    let _ = status_report.send("Checking video length...".to_string());
    let (_input_frame_count, _input_frame_rate, _has_audio, input_length) = unfail!(
        count_video_frames_and_framerate_and_audio_and_length(inputfile)
    );

    let _ = status_report.send("Amen breaking...".to_string());

    let input_params = if amen_break_length > input_length {
        [
            OsStr::new("-stream_loop"),
            OsStr::new("-1"),
            OsStr::new("-i"),
            inputfile.as_ref(),
        ]
    } else {
        [
            OsStr::new("-i"),
            inputfile.as_ref(),
            OsStr::new("-stream_loop"),
            OsStr::new("-1"),
        ]
    };

    let converter = Command::new("ffmpeg")
        .args([
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            input_params[0],
            input_params[1],
            input_params[2],
            input_params[3],
            OsStr::new("-i"),
            break_path.as_ref(),
            OsStr::new("-map"),
            OsStr::new("0:"),
            OsStr::new("-map"),
            OsStr::new("-0:a"),
            OsStr::new("-map"),
            OsStr::new("1:a"),
            OsStr::new("-shortest"),
            OsStr::new("-pix_fmt"),
            OsStr::new("yuv420p"),
            OsStr::new("-f"),
            OsStr::new("mp4"),
            OsStr::new("-preset"),
            OsStr::new("slow"),
            outputfile.path().as_os_str(),
        ])
        .spawn();

    let converter_result = unfail!(converter).wait();
    let converter_result = unfail!(converter_result);
    if !converter_result.success() {
        return Err("Converter returned an error.".to_string());
    }

    unfail!(outputfile.reopen());

    let mut output = Vec::new();
    unfail!(outputfile.read_to_end(&mut output));

    Ok(output)
}
