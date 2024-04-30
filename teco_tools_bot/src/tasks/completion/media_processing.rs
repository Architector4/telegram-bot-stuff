use std::{
    io::{BufReader, Read, Write},
    process::{ChildStdout, Command, Stdio},
};

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

pub fn resize_video(
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
    let input_size = data.len();

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

    let encoder = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "quiet",
            "-rtbufsize",
            "1G",
            "-i",
            "-",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "mp4",
            "-movflags",
            "empty_moov",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let mut encoder = unfail!(encoder);
    let mut encoder_stdin = encoder.stdin.take().unwrap();

    let writing_stream = converted_image_stream.map(|frame| match frame {
        Ok(frame) => encoder_stdin.write_all(&frame),
        Err(e) => Err(e),
    });

    if let Some(last) = writing_stream.last() {
        unfail!(last);
    }

    unfail!(decoder.wait());
    drop(encoder_stdin);

    let mut output = Vec::with_capacity(input_size);
    output.clear();

    unfail!(encoder.stdout.take().unwrap().read_to_end(&mut output));

    Ok(output)
}
