use std::{
    io::{BufReader, Read, Write},
    process::{ChildStdout, Command, Stdio},
};

use crossbeam_channel::Sender;
use magick_rust::{MagickError, MagickWand};

use crate::tasks::{ImageFormat, ResizeType};

/// Will error if [`ImageFormat::Preserve`] is sent.
pub fn resize_image(
    data: &[u8],
    width: usize,
    height: usize,
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

    if wand.get_image_width() <= 1 || wand.get_image_height() <= 1 && resize_type.is_seam_carve() {
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

pub fn count_video_frames_and_framerate(data: &[u8]) -> Result<(u64, f64), std::io::Error> {
    macro_rules! goodbye {
        ($desc: expr) => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, $desc))
        };
    }
    let mut counter = Command::new("ffprobe")
        .args([
            "-loglevel",
            "quiet",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_frames,avg_frame_rate",
            "-of",
            "default=noprint_wrappers=1",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let mut counter_stdin = counter.stdin.take().unwrap();

    let _ = counter_stdin.write_all(data);

    let output = counter.wait_with_output()?;
    let Ok(output) = String::from_utf8(output.stdout) else {
        goodbye!("Counter returned non UTF-8 response");
    };

    // output may be in a format like
    // avg_frame_rate=30/1
    // nb_frames=69
    // Or
    // avg_frame_rate=3200000/53387
    // nb_frames=N/A

    let mut count = 0;
    let mut framerate: f64 = 30.0;

    for line in output.split('\n') {
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

    Ok((count, framerate))
}

pub fn resize_video(
    status_report: Sender<String>,
    data: Vec<u8>,
    width: usize,
    height: usize,
    resize_type: ResizeType,
) -> Result<Vec<u8>, String> {
    macro_rules! unfail {
        ($thing: expr) => {
            match $thing {
                Ok(o) => o,
                Err(e) => return Err(e.to_string()),
            }
        };
    }
    let (input_frame_count, input_frame_rate) = unfail!(count_video_frames_and_framerate(&data));

    let decoder = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "quiet",
            "-rtbufsize",
            "1G",
            "-i",
            "-",
            "-c:v",
            "bmp",
            "-f",
            "image2pipe",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut decoder = unfail!(decoder);
    let mut decoder_stdin = decoder.stdin.take().unwrap();
    // Just      spam it out from another thread.
    // Hacky, but prevents deadlocking with all the iteration here lmao
    std::thread::spawn(move || decoder_stdin.write_all(&data));
    let data_stream = decoder.stdout.take().unwrap();

    let converted_image_stream =
        SplitIntoBmps::<ChildStdout>::new(data_stream).map(|frame| match frame {
            Ok(frame) => Ok(
                resize_image(&frame, width, height, resize_type, ImageFormat::Bmp)
                    .expect("ImageMagick failed!"),
            ),
            Err(e) => Err(e),
        });

    let (frame_sender, frame_receiver) = crossbeam_channel::unbounded::<Vec<u8>>();

    let encoder_thread = std::thread::spawn(move || {
        let frame_receiver = frame_receiver;
        let mut encoder = Command::new("ffmpeg")
            .args([
                "-loglevel",
                "quiet",
                "-rtbufsize",
                "1G",
                "-i",
                "-",
                "-pix_fmt",
                "yuv420p",
                "-r",
                input_frame_rate.to_string().as_str(),
                "-f",
                "mp4",
                "-movflags",
                "empty_moov",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Spawning encoder failed!");
        let mut encoder_stdin = encoder.stdin.take().unwrap();

        std::thread::spawn(move || {
            let mut frame_count: usize = 0;
            while let Ok(frame) = frame_receiver.recv() {
                frame_count += 1;
                if input_frame_count != 0 {
                    let _ = status_report
                        .send(format!("Frame {} / {}", frame_count, input_frame_count));
                } else {
                    let _ = status_report.send(format!("Frame {}", frame_count));
                }
                dbg!("Writing frame");
                encoder_stdin
                    .write_all(&frame)
                    .expect("Failed writing frame to encoder!");
                dbg!("Wrote frame");
            }
            dbg!("Dropping stdin");
            drop(encoder_stdin);
            dbg!("Dropped stdin");
        });

        dbg!("Waiting for encoder...");
        let output = encoder
            .wait_with_output()
            .expect("Waiting for encoder failed!");
        dbg!("Waited for encoder.");

        output.stdout
    });

    let writing_stream = converted_image_stream.map(|frame| match frame {
        Ok(frame) => {
            dbg!("Sending frame");
            let Ok(()) = frame_sender.send(frame) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Failed sending frame to encoder!",
                ));
            };
            dbg!("Sent frame");
            Ok(())
        }
        Err(e) => Err(e),
    });

    if let Some(last) = writing_stream.last() {
        unfail!(last);
    }

    drop(frame_sender);

    let output = encoder_thread.join().map_err(|e| {
        if let Ok(e) = e.downcast() {
            let e: Box<&'static str> = e;
            *e.as_ref()
        } else {
            "Joining encoder thread failed!"
        }
    });

    Ok(unfail!(output))
}
