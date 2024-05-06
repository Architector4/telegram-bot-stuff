use std::{
    ffi::OsStr,
    io::{BufReader, Read, Write},
    process::{ChildStdout, Command, Stdio},
};

use crossbeam_channel::Sender;
use rayon::prelude::*;

use magick_rust::{MagickError, MagickWand, PixelWand};
use tempfile::NamedTempFile;

use crate::tasks::{ImageFormat, ResizeType};

/// Will error if [`ImageFormat::Preserve`] is sent.
pub fn resize_image(
    data: &[u8],
    width: usize,
    height: usize,
    rotation: f64,
    mut resize_type: ResizeType,
    format: ImageFormat,
) -> Result<Vec<u8>, MagickError> {
    if format == ImageFormat::Preserve {
        // yeah this isn't a MagickError, but we'd get one in the last line
        // anyways, so might as well make a better description for ourselves lol
        return Err(MagickError("ImageFormat::Preserve was specified"));
    }

    let wand = MagickWand::new();

    wand.read_image_blob(data)?;

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

    let iwidth = wand.get_image_width();
    let iheight = wand.get_image_height();

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
        ResizeType::Crop => {
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
        let mut pixelwand = PixelWand::new();
        pixelwand.set_alpha(0.0);
        wand.rotate_image(&pixelwand, rotation)?;
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
            OsStr::new("quiet"),
            OsStr::new("-show_entries"),
            OsStr::new("stream=nb_frames,avg_frame_rate,codec_type"),
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
    // avg_frame_rate=30/1
    // nb_frames=69
    // Or
    // codec_type=audio
    // avg_frame_rate=0/0
    // codec_type=video
    // nb_frames=80
    // avg_frame_rate=3200000/53387
    // nb_frames=N/A

    let mut count = 0;
    let mut framerate: f64 = 30.0;
    let mut observing_video_codecs = false;
    let mut has_audio = false;

    for line in output.split('\n') {
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
        if let Some(line) = line.strip_prefix("nb_frames=") {
            if line == "N/A" {
                continue;
            }
            let Ok(this_count) = line.parse::<u64>() else {
                goodbye!("Counter returned a non-integer");
            };
            count = this_count;
        } else if let Some(line) = line.strip_prefix("avg_frame_rate=") {
            framerate = if let Some(slash) = line.find('/') {
                let (a, b) = line.split_at(slash);
                let Ok(a) = a.parse::<f64>() else {
                    goodbye!("Framerate value a couldn't be parsed");
                };
                let Ok(b) = b[1..].parse::<f64>() else {
                    goodbye!("Framerate value b couldn't be parsed");
                };
                a / b
            } else {
                let Ok(this_framerate) = line.parse::<f64>() else {
                    goodbye!("Framerate value is without slash and unparsable");
                };

                this_framerate
            }
        }
    }

    Ok((count, framerate, has_audio))
}

pub fn resize_video(
    status_report: Sender<String>,
    data: Vec<u8>,
    mut width: usize,
    mut height: usize,
    rotation: f64,
    resize_type: ResizeType,
) -> Result<Vec<u8>, String> {
    // We will be encoding into h264, which needs width and height divisible by 2.
    width += width % 2;
    height += height % 2;

    macro_rules! unfail {
        ($thing: expr) => {
            match $thing {
                Ok(o) => o,
                Err(e) => return Err(e.to_string()),
            }
        };
    }

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
                Ok(frame) => Ok((
                    count,
                    resize_image(
                        &frame,
                        width,
                        height,
                        rotation,
                        resize_type,
                        ImageFormat::Bmp,
                    )
                    .expect("ImageMagick failed!"),
                )),
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

        std::thread::spawn(move || {
            let mut frame_number: usize = 0;
            let mut out_of_order_frames: Vec<(usize, Vec<u8>)> = Vec::new();

            while let Ok(frame) = frame_receiver.recv() {
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

                    loop {
                        let Some(in_order_frame) =
                            out_of_order_frames.iter().position(|x| x.0 == frame_number)
                        else {
                            break;
                        };

                        let in_order_frame = out_of_order_frames.swap_remove(in_order_frame);

                        encoder_stdin
                            .write_all(&in_order_frame.1)
                            .expect("Failed writing frame to encoder!");
                        frame_number += 1;
                    }
                }

                if input_frame_count != 0 {
                    let _ = status_report_for_encoder
                        .send(format!("Frame {} / {}", frame_number, input_frame_count));
                } else {
                    let _ = status_report_for_encoder.send(format!("Frame {}", frame_number));
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
        if let Ok(e) = e.downcast() {
            let e: Box<&'static str> = e;
            *e.as_ref()
        } else {
            "Joining encoder thread failed!"
        }
    });

    unfail!(decoder.wait());
    unfail!(encoder_thread);

    let mut finalfile = if has_audio {
        let _ = status_report.send("Writing audio...".to_string());
        // Now to transfer audio... This means we need a THIRD file to put the final result into.
        let muxfile = unfail!(NamedTempFile::new());
        let distort = resize_type.is_seam_carve();
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
                OsStr::new(if distort { "-af" } else { "-c:a" }),
                OsStr::new(if distort {
                    "vibrato=f=7:d=1,aformat=s16p"
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
