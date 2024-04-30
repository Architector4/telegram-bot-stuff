use magick_rust::{MagickError, MagickWand};

use crate::tasks::{ImageFormat, ResizeType};

/// Will error if [`ImageFormat::Preserve`] is sent.
pub fn resize_image(
    data: Vec<u8>,
    width: usize,
    height: usize,
    resize_type: ResizeType,
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
