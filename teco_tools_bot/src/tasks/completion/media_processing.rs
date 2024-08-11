#![allow(clippy::manual_clamp)] // It's better here since it also gets rid of NaN

use std::{
    ffi::OsStr,
    io::{BufReader, Read, Write},
    process::{ChildStdout, Command, Stdio},
    sync::OnceLock,
};

use crossbeam_channel::Sender;
use rayon::prelude::*;

use magick_rust::{MagickError, MagickWand, PixelWand};
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
    mut resize_type: ResizeType,
    format: ImageFormat,
    // Width, height, and if the resulting image should be stretched to
    // output size instead of fitting.
    output_size: Option<(usize, usize, bool)>,
    crop_rotation: bool,
) -> Result<Vec<u8>, MagickError> {
    if format == ImageFormat::Preserve {
        // yeah this isn't a MagickError, but we'd get one in the last line
        // anyways, so might as well make a better description for ourselves lol
        return Err(MagickError("ImageFormat::Preserve was specified"));
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

    if (iwidth <= 1 || iheight <= 1) && resize_type.is_seam_carve() {
        // ImageMagick is likely to abort/segfault in this situation.
        // Switch up resize type.
        resize_type = ResizeType::Stretch;
    }

    match resize_type {
        ResizeType::SeamCarve { delta_x, rigidity } => {
            wand.liquid_rescale_image(
                width,
                height,
                delta_x.abs().min(4.0),
                rigidity.max(-4.0).min(4.0),
            )?;
        }
        ResizeType::Stretch => {
            wand.resize_image(
                width,
                height,
                magick_rust::bindings::FilterType_LagrangeFilter,
            );
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
                size_pre_crop.1 = (size_pre_crop.1 * size_pre_crop.0) / 16384;
                size_pre_crop.0 = 16384;
            }
            if size_pre_crop.1 > 16384 {
                size_pre_crop.0 = (size_pre_crop.0 * size_pre_crop.1) / 16384;
                size_pre_crop.1 = 16384;
            }

            // Resize to desired size... Yes, this may stretch, but that's better
            // since then we keep the exact end size, and the crop below
            // will not fail then lol
            wand.resize_image(
                size_pre_crop.0,
                size_pre_crop.1,
                magick_rust::bindings::FilterType_LagrangeFilter,
            );

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
            wand.resize_image(
                output_size.0,
                output_size.1,
                magick_rust::bindings::FilterType_LagrangeFilter,
            );
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
            wand.set_image_alpha_channel(magick_rust::bindings::AlphaChannelOption_OnAlphaChannel)?;
        }

        let pre_rotation_width = wand.get_image_width();
        let pre_rotation_height = wand.get_image_height();

        wand.rotate_image(&transparent, rotation)?;

        if crop_rotation {
            // If we want cropping after rotation, do the cropping.
            wand.crop_image(pre_rotation_width, pre_rotation_height, 0, 0)?;
        }
    }

    wand.write_image_blob(format.as_str())
}

struct SplitIntoBmps<T: Read> {
    item: BufReader<T>,
    buffer: Vec<u8>,
    /// Positive if we don't have a full BMP in yet,
    until_next_bmp: usize,
}

impl<T: Read> SplitIntoBmps<T> {
    pub fn new<N: Read>(item: N) -> SplitIntoBmps<N> {
        SplitIntoBmps {
            item: BufReader::new(item),
            buffer: Vec::with_capacity(1024 * 1024),
            until_next_bmp: 0,
        }
    }
}

impl<T: Read> Iterator for SplitIntoBmps<T> {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn next(&mut self) -> Option<Self::Item> {
        macro_rules! unfail {
            ($thing: expr) => {
                match $thing {
                    Ok(o) => o,
                    Err(e) => return Some(Err(e)),
                }
            };
        }

        if self.buffer.is_empty() {
            // We are at a boundary between BMPs.
            assert_eq!(self.until_next_bmp, 0);
            // Read a bit to know the length of the next one.
            for byte in self.item.by_ref().bytes() {
                let byte = unfail!(byte);
                self.buffer.push(byte);
                if self.buffer.len() > 6 {
                    break;
                }
            }
            if self.buffer.len() < 6 {
                // Not enough bytes? Bye.
                return None;
            }

            // Assert that this is a BMP header.
            assert_eq!(self.buffer[0..2], [0x42, 0x4D]);

            // This means that bytes [2..6] have the file length.
            let new_bmp_length = u32::from_le_bytes(
                self.buffer[2..6]
                    .try_into()
                    .expect("Incorrect slice length... somehow."),
            );

            self.until_next_bmp = new_bmp_length as usize - self.buffer.len();
        }

        if self.until_next_bmp > 0 {
            for byte in self.item.by_ref().bytes() {
                let byte = unfail!(byte);
                self.buffer.push(byte);
                self.until_next_bmp -= 1;
                if self.until_next_bmp == 0 {
                    break;
                }
            }
        }

        if self.until_next_bmp != 0 {
            // Not enough bytes? Seeya.
            return None;
        }

        // Then we have accumulated a BMP. Send it.
        let response = self.buffer.clone();
        self.buffer.clear();
        Some(Ok(response))
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

pub fn count_video_frames_and_framerate_and_audio(
    path: &std::path::Path,
) -> Result<(u64, f64, bool), std::io::Error> {
    macro_rules! goodbye {
        ($desc: expr) => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, $desc))
        };
    }
    let counter = Command::new("ffprobe")
        .args([
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-count_frames"),
            OsStr::new("-show_entries"),
            OsStr::new("stream=nb_read_frames,codec_type"),
            OsStr::new("-show_entries"),
            OsStr::new("format=duration"),
            OsStr::new("-of"),
            OsStr::new("default=noprint_wrappers=1"),
            path.as_ref(),
        ])
        .stdout(Stdio::piped())
        .spawn()?;

    let output = counter.wait_with_output()?;
    let Ok(output) = String::from_utf8(output.stdout) else {
        goodbye!("Counter returned non UTF-8 response");
    };

    // output may be in a format like
    // codec_type=video
    // nb_read_frames=69
    // duration=69.420
    // Or
    // codec_type=audio
    // avg_frame_rate=0/0
    // codec_type=video
    // nb_read_frames=80
    // duration=1312.1312

    let mut count = 0;
    let mut observing_video_codecs = false;
    let mut has_audio = false;
    // Random ass default value lol
    let mut duration = 10.0;

    for line in output.lines() {
        if let Some(line) = line.strip_prefix("duration=") {
            let Ok(d) = line.parse::<f64>() else {
                goodbye!("Duration couldn't be parsed");
            };
            duration = d;
        }

        if line == "codec_type=audio" {
            observing_video_codecs = false;
            has_audio = true;
            continue;
        }
        if line == "codec_type=video" {
            observing_video_codecs = true;
            continue;
        }
        if !observing_video_codecs {
            continue;
        }

        if let Some(line) = line.strip_prefix("nb_read_frames=") {
            if line == "N/A" {
                continue;
            }
            let Ok(this_count) = line.parse::<u64>() else {
                goodbye!("Counter returned a non-integer");
            };
            count = this_count;
        }
    }

    let framerate = count as f64 / duration;

    Ok((count, framerate, has_audio))
}

#[allow(clippy::too_many_arguments)]
pub fn resize_video(
    status_report: Sender<String>,
    data: Vec<u8>,
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
        let input_width = input_dimensions.0 as f64;
        let input_height = input_dimensions.1 as f64;
        let input_aspect_ratio = input_width / input_height;

        // "max" to avoid inf/NaN values
        let end_width = f64::abs(width as f64).max(1.0);
        let end_height = f64::abs(height as f64).max(1.0);
        let end_aspect_ratio = end_width / end_height;

        // Now, compute the stretch we'd have on the smallest width and height
        // of the two if we were to correct from one aspect ratio to another.
        let smallest_width = input_width.min(end_width);
        let smallest_width_corrected = smallest_width * input_aspect_ratio / end_aspect_ratio;
        let smallest_height = input_height.min(end_height);
        let smallest_height_corrected = smallest_height * input_aspect_ratio / end_aspect_ratio;

        // If both of them don't change significantly, then we can just stretch.

        let width_diff_is_insignificant = f64::abs(smallest_width_corrected - smallest_width) < 1.5;
        let height_diff_is_insignificant =
            f64::abs(smallest_height_corrected - smallest_height) < 1.5;

        width_diff_is_insignificant && height_diff_is_insignificant
    } else {
        false
    };

    let _ = status_report.send("Creating temp files...".to_string());

    let mut inputfile = unfail!(NamedTempFile::new());
    unfail!(inputfile.write_all(&data));
    unfail!(inputfile.flush());
    let outputfile = unfail!(NamedTempFile::new());

    let _ = status_report.send("Counting frames...".to_string());

    let (input_frame_count, input_frame_rate, has_audio) =
        unfail!(count_video_frames_and_framerate_and_audio(inputfile.path()));

    let decoder = Command::new("ffmpeg")
        .args([
            OsStr::new("-y"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-i"),
            inputfile.path().as_ref(),
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
    let data_stream = decoder.stdout.take().unwrap();

    let converted_image_stream = {
        SplitIntoBmps::<ChildStdout>::new(data_stream)
            .enumerate()
            .par_bridge()
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
                    // If the dimension and rotation are the same, it doesn't.
                    let input_dimensions = get_bmp_width_height(&frame);
                    let resize_result = if input_dimensions
                        == Some((curved_width as isize, curved_height as isize))
                        && (rotation == 0.0 || rotation == -0.0)
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
            })
    };

    let _ = status_report.send("Initializing encoder...".to_string());

    let (frame_sender, frame_receiver) = crossbeam_channel::bounded::<(usize, Vec<u8>)>(256);
    let outputfilepath_for_encoder = outputfile.path().as_os_str().to_os_string();
    let status_report_for_encoder = status_report.clone();

    let encoder_thread = std::thread::spawn(move || {
        let frame_receiver = frame_receiver;
        let mut encoder = Command::new("ffmpeg")
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
                outputfilepath_for_encoder.as_ref(),
            ])
            .stdin(Stdio::piped())
            .spawn()
            .expect("Spawning encoder failed!");
        let mut encoder_stdin = encoder.stdin.take().unwrap();

        let _ = status_report_for_encoder.send("Waiting for the thread pool...".to_string());

        std::thread::spawn(move || {
            let mut frame_number: usize = 0;
            let mut frames_received: usize = 0;
            let mut out_of_order_frames: Vec<(usize, Vec<u8>)> = Vec::new();

            while let Ok(frame) = frame_receiver.recv() {
                frames_received += 1;
                if frame.0 == frame_number {
                    // Frame received in order. Push it in directly.
                    encoder_stdin
                        .write_all(&frame.1)
                        .expect("Failed writing frame to encoder!");
                    frame_number += 1;
                } else {
                    // It's out of order. Push it away.
                    out_of_order_frames.push(frame);
                }

                if !out_of_order_frames.is_empty() {
                    // If possible, send them in order.

                    while let Some(in_order_frame) =
                        out_of_order_frames.iter().position(|x| x.0 == frame_number)
                    {
                        let in_order_frame = out_of_order_frames.swap_remove(in_order_frame);

                        encoder_stdin
                            .write_all(&in_order_frame.1)
                            .expect("Failed writing frame to encoder!");
                        frame_number += 1;
                    }
                }

                if input_frame_count != 0 {
                    let _ = status_report_for_encoder
                        .send(format!("Frame {} / {}", frames_received, input_frame_count));
                } else {
                    let _ = status_report_for_encoder.send(format!("Frame {}", frames_received));
                }
            }
            drop(encoder_stdin);
        });

        encoder.wait().expect("Encoder died!");
    });

    let writing_stream = converted_image_stream.map(|frame| match frame {
        Ok(frame) => {
            let Ok(()) = frame_sender.send(frame) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Failed sending frame to encoder!",
                ));
            };
            Ok(())
        }
        Err(e) => Err(e),
    });

    // Try to find an error and fail on it, if any lol
    let reduce = writing_stream.reduce(|| Ok(()), |a, b| if b.is_err() { b } else { a });
    unfail!(reduce);

    let _ = status_report.send("Finalizing...".to_string());

    drop(frame_sender);

    let encoder_thread = encoder_thread.join().map_err(|e| {
        if let Ok(e) = e.downcast::<Box<&'static str>>() {
            **e
        } else {
            "Joining encoder thread failed!"
        }
    });

    unfail!(decoder.wait());
    unfail!(encoder_thread);

    let mut finalfile = if has_audio && !strip_audio {
        let _ = status_report.send("Writing audio...".to_string());
        // Now to transfer audio... This means we need a THIRD file to put the final result into.
        let muxfile = unfail!(NamedTempFile::new());
        // Will exclude cases of 0.0, -0.0, and all the NaNs and infinities
        let distort_audio = vibrato_hz.is_normal() && vibrato_depth.is_normal();

        let vibrato_str_temp;

        let audiomuxer = Command::new("ffmpeg")
            .args([
                OsStr::new("-y"),
                OsStr::new("-loglevel"),
                OsStr::new("error"),
                OsStr::new("-i"),
                inputfile.path().as_ref(),
                OsStr::new("-i"),
                outputfile.path().as_ref(),
                OsStr::new("-c:v"),
                OsStr::new("copy"),
                OsStr::new("-map"),
                OsStr::new("1:v:0"),
                OsStr::new("-map"),
                OsStr::new("0:a:0"),
                OsStr::new(if distort_audio { "-af" } else { "-c:a" }),
                OsStr::new(if distort_audio {
                    vibrato_str_temp = format!(
                        "vibrato=f={}:d={},aformat=s16p",
                        vibrato_hz.max(0.1).min(20000.0),
                        vibrato_depth.max(0.0).min(1.0)
                    );
                    vibrato_str_temp.as_str()
                } else {
                    "copy"
                }),
                OsStr::new("-f"),
                OsStr::new("mp4"),
                muxfile.path().as_ref(),
            ])
            .spawn();

        unfail!(unfail!(audiomuxer).wait());

        muxfile
    } else {
        outputfile
    };

    unfail!(finalfile.reopen());

    let mut output = data;
    output.clear();
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
