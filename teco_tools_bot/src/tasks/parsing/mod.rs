pub mod tokenizer;

use super::*;
use html_escape::encode_text;
use tokenizer::{Token, Tokenizer};

pub static MAX_OUTPUT_MEDIA_DIMENSION_SIZE: u32 = 2048;

#[derive(Debug)]
pub enum TaskError {
    Error(String),
    Descriptory(String),
    Cancel(String),
}

impl TaskError {
    pub fn cancel_to_error(self) -> Self {
        if let Self::Cancel(e) = self {
            Self::Error(e)
        } else {
            self
        }
    }
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Error(e) => e.is_empty(),
            Self::Descriptory(d) => d.is_empty(),
            Self::Cancel(c) => c.is_empty(),
        }
    }
    pub fn is_cancel(&self) -> bool {
        matches!(self, TaskError::Cancel(_))
    }
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            Ok(())
        } else {
            match self {
                Self::Error(e) => {
                    writeln!(f, "Error: {}", e)
                }
                Self::Cancel(c) => {
                    writeln!(f, "Cancelling task: {}", c)
                }
                Self::Descriptory(d) => {
                    writeln!(f, "{}", d)
                }
            }
        }
    }
}

const PARAM_HELP: &str = concat!(
    "\nPlease provide parameters in the format of\n",
    "<code>  setting: value, setting: value    </code>\n",
    "and separated by commas or newlines.\n\n",
    "<b>Possible parameters for this function:</b>\n"
);

/// Returns true if this isn't a plain parameter,
/// false if it is but failed to parse, or continues if it succeeds.
macro_rules! parse_plain_param_with_parser_optional {
    ($input: expr, $name: expr, $parser: expr, $help: expr) => {{
        #[allow(clippy::redundant_closure_call)]
        if let Token::Plain(value) = $input {
            if let Ok(value) = $parser(value) {
                $name = value;
                continue;
            } else {
                true
            }
        } else {
            false
        }
    }};
}
macro_rules! parse_plain_param_optional {
    ($input: expr, $name: expr, $help: expr) => {
        parse_plain_param_with_parser_optional!($input, $name, std::str::FromStr::from_str, $help)
    };
}

macro_rules! parse_plain_param_with_parser_mandatory {
    ($input: expr, $name: expr, $parser: expr, $help: expr) => {
        #[allow(clippy::redundant_closure_call)]
        if let Token::Plain(value) = $input {
            let Ok(value) = $parser(value) else {
                return Err(TaskError::Error(format!(
                    "the value <code>{}</code> is incorrect for parameter <code>{}</code>.{}{}",
                    encode_text(value),
                    encode_text(stringify!($name)),
                    PARAM_HELP,
                    $help
                )));
            };
            $name = value;
            continue;
        }
    };
}
macro_rules! parse_plain_param {
    ($input: expr, $name: expr, $help: expr) => {
        parse_plain_param_with_parser_mandatory!($input, $name, std::str::FromStr::from_str, $help)
    };
}

macro_rules! parse_keyval_param_with_parser {
    ($input: expr, $name: expr, $parser: expr, $help: expr) => {
        let (key, value) = match $input {
            Token::KeyVal(key, value) => (key, value),
            Token::Plain(plain) => {
                return Err(TaskError::Error(format!(
                    "can't parse <code>{}</code> as a parameter.{}{}",
                    encode_text(plain),
                    PARAM_HELP,
                    $help
                )));
            }
        };

        if key == stringify!($name).to_lowercase() {
            parse_plain_param_with_parser_mandatory!(Token::Plain(value), $name, $parser, $help);
            continue;
        }
    };
}

macro_rules! parse_keyval_param {
    ($input: expr, $name: expr, $help: expr) => {
        parse_keyval_param_with_parser!($input, $name, std::str::FromStr::from_str, $help)
    };
}
macro_rules! parse_stop {
    ($input: expr, $help: expr) => {
        let response = match $input {
            Token::KeyVal(key, val) => format!(
                "unexpected parameter <code>{}</code> with value <code>{}</code>{}{}",
                encode_text(key),
                encode_text(val),
                PARAM_HELP,
                $help
            ),
            Token::Plain(plain) => format!(
                "unexpected parameter <code>{}</code>{}{}",
                encode_text(plain),
                PARAM_HELP,
                $help
            ),
        };

        return Err(TaskError::Error(response));
    };
}

impl Task {
    pub fn param_help(&self) -> &'static str {
        match self {
            Task::Amogus { .. } => {
                "<code>amogus</code>: How much amogus. Negative numbers are allowed."
            }
            Task::ImageResize { resize_type, ..} => {
                match resize_type {
                    ResizeType::ToSticker | ResizeType::ToCustomEmoji => "",
                    ResizeType::SeamCarve { .. } =>
                        concat!(
                            "Size specification (values can be negative for mirroring):\n",
                            "<code>WxH</code>: Width and height of the output image, in pixels or percentages; ",
                            "can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "Above parameters may be specified multiple times and will be applied cumulatively.\n",
                            "\n",
                            "<code>format</code>: Format to save the image in: jpeg, webp or preserve\n",
                            "<code>rot</code>: Rotate the image by this much after distorting.\n",
                            "<code>delta_x</code>: Maximum seam transversal step. 0 means straight seams. Default is 2. ",
                            "Can't be less than -4 or bigger than 4.\n",
                            "<code>rigidity</code>: Bias for non-straight seams. Default is 0. ",
                            "Can't be less than -1024 or bigger than 1024.\n",
                            "<code>format</code>: Output image format. Can be \"webp\" or \"jpg\"."
                            ),
                    ResizeType::Stretch | ResizeType::Fit | ResizeType::Crop =>
                        concat!(
                            "Size specification (values can be negative for mirroring):\n",
                            "<code>WxH</code>: Width and height of the output image, in pixels or percentages; ",
                            "can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "Above parameters may be specified multiple times and will be applied cumulatively.\n",
                            "\n",
                            "<code>rot</code>: Rotate the image by this much after resizing.\n",
                            "<code>method</code>: Resize method. Can only be \"fit\", \"stretch\" or \"crop\".\n",
                            "<code>format</code>: Output image format. Can be \"webp\" or \"jpg\"."
                            ),
                }
            },
            Task::VideoResize { resize_type, .. } => {
                match resize_type {
                    ResizeType::ToSticker| ResizeType::ToCustomEmoji  => "",
                    ResizeType::SeamCarve { ..} => concat!(
                            "Size specification (values can be negative for mirroring):\n",
                            "<code>WxH</code>: Width and height of the output video, in pixels or percentages; ",
                            "can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "Above parameters may be specified multiple times and will be applied cumulatively.\n",
                            "\n",
                            "<code>rot</code>: Rotate the video by this much after distorting.\n",
                            "<code>delta_x</code>: Maximum seam transversal step. 0 means straight seams. Default is 2. ",
                            "Can't be less than -4 or bigger than 4.\n",
                            "<code>rigidity</code>: Bias for non-straight seams. Default is 0. ",
                            "Can't be less than -1024 or bigger than 1024.\n",
                            "\n",
                            "<code>vibrato_hz</code>: Frequency of vibrato applied to audio. ",
                            "Can only be between 0.1 or 20000.0. Default is 7.\n",
                            "<code>vibrato_depth</code>: Vibrato depth. Can only be between 0.0 and 1.0. Default is 1."

                        ),
                    ResizeType::Stretch | ResizeType::Fit | ResizeType::Crop => concat!(
                            "Size specification (values can be negative for mirroring):\n",
                            "<code>WxH</code>: Width and height of the output video, in pixels or percentages; ",
                            "can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "Above parameters may be specified multiple times and will be applied cumulatively.\n",
                            "\n",
                            "<code>rot</code>: Rotate the video by this much after resizing.\n",
                            "<code>method</code>: Resize method. Can only be \"fit\", \"stretch\" or \"crop\".\n",
                            "\n",
                            "<code>vibrato_hz</code>: Frequency of vibrato applied to audio. ",
                            "Can only be between 0.1 or 20000.0. Default is 7.",
                            "<code>vibrato_depth</code> also needs to be set for this to apply.\n",
                            "<code>vibrato_depth</code>: Vibrato depth. Can only be between 0.0 and 1.0. Default is 0."
                        ),
                }
            }
        }
    }

    pub fn parse_params(&self, params: &str) -> Result<Task, TaskError> {
        let params = Tokenizer::new(params);
        let help = self.param_help();
        match self {
            Task::Amogus { amogus } => {
                let mut amogus = *amogus;

                for param in params {
                    parse_plain_param!(param, amogus, help);
                    parse_keyval_param!(param, amogus, help);
                    parse_stop!(param, help);
                }

                if amogus == 69 {
                    return Err(TaskError::Cancel("WEIRD AMOGUS".to_string()));
                }

                Ok(Task::Amogus { amogus })
            }
            Task::ImageResize {
                new_dimensions: original_dimensions,
                rotation,
                percentage: _,
                format: _,
                mut resize_type,
            }
            | Task::VideoResize {
                new_dimensions: original_dimensions,
                rotation,
                percentage: _,
                mut resize_type,
                vibrato_hz: _,
                vibrato_depth: _,
            } => {
                if let ResizeType::ToSticker | ResizeType::ToCustomEmoji = resize_type {
                    return Ok(self.clone());
                }

                let mut old_dimensions = (original_dimensions.0.get(), original_dimensions.1.get());

                let (is_video, mut format) = if let Task::ImageResize { format, .. } = self {
                    (false, *format)
                } else {
                    (true, ImageFormat::Preserve)
                };

                let (mut vibrato_hz, mut vibrato_depth) = if resize_type.is_seam_carve() {
                    (7.0, 1.0)
                } else {
                    (7.0, 0.0)
                };

                let mut rot = *rotation;
                // Width, height, and percentage.
                let mut new_dimensions: Option<(i32, i32)> = None;
                let ResizeType::SeamCarve {
                    mut delta_x,
                    mut rigidity,
                } = ResizeType::default_seam_carve()
                else {
                    unreachable!();
                };

                // This closure returns a closure that parses a
                // string to a float within specified range inclusively lol
                let sanitized_f64_parser = |min: f64, max: f64| {
                    move |val: &str| -> Result<f64, ()> {
                        let result: f64 = val.parse().map_err(|_| ())?;

                        if result.is_finite() && (min..=max).contains(&result) {
                            Ok(result)
                        } else {
                            Err(())
                        }
                    }
                };

                for param in params {
                    if let Some(new_dimensions) = new_dimensions {
                        // If new dimensions were set by anything here,
                        // reset them to old dimensions.
                        // This makes dimension changing parameters accumulate.
                        old_dimensions = (new_dimensions.0, new_dimensions.1);
                    }

                    let dimensions_parser_err = |data| {
                        dimensions_parser(data, old_dimensions)
                            .map(|x| Some((x.0, x.1)))
                            .ok_or(())
                    };

                    if !is_video {
                        parse_plain_param_optional!(param, format, help);
                    }
                    parse_plain_param_with_parser_optional!(
                        param,
                        new_dimensions,
                        dimensions_parser_err,
                        help
                    );
                    parse_plain_param_with_parser_optional!(
                        param,
                        rot,
                        |x| {
                            if let Some((rotation, true)) = rotation_parser(x) {
                                Ok(rotation)
                            } else {
                                Err(())
                            }
                        },
                        help
                    );

                    if let ResizeType::SeamCarve { .. } = &mut resize_type {
                        parse_keyval_param_with_parser!(
                            param,
                            delta_x,
                            sanitized_f64_parser(-4.0, 4.0),
                            help
                        );
                        parse_keyval_param_with_parser!(
                            param,
                            rigidity,
                            sanitized_f64_parser(-1024.0, 1024.0),
                            help
                        );
                    } else {
                        parse_plain_param_optional!(param, resize_type, help);
                    }

                    if is_video {
                        parse_keyval_param_with_parser!(
                            param,
                            vibrato_hz,
                            sanitized_f64_parser(0.1, 20000.0),
                            help
                        );
                        parse_keyval_param_with_parser!(
                            param,
                            vibrato_depth,
                            sanitized_f64_parser(0.0, 1.0),
                            help
                        );
                    }

                    if let Token::KeyVal(k, v) = param {
                        let v = (k, v);
                        // Try to parse it as an aspect ratio lol
                        if let Some(parse) = aspect_ratio_parser(v, old_dimensions) {
                            new_dimensions = Some((parse.0, parse.1));
                            continue;
                        }
                        // If it fails to parse, the format parser below will complain with all the
                        // help lol
                    }
                    parse_keyval_param!(param, format, help);
                    parse_keyval_param_with_parser!(
                        param,
                        rot,
                        |x| rotation_parser(x).map(|x| x.0).ok_or(()),
                        help
                    );

                    parse_stop!(param, help);
                }

                // New dimension, and maybe the percentage of the original size it is.
                let new_dimensions: (i32, i32, Option<f32>) = if let Some(new_dimensions) =
                    new_dimensions
                {
                    // Ensure it's not too big.
                    let media_too_big = new_dimensions.0.unsigned_abs()
                        > MAX_OUTPUT_MEDIA_DIMENSION_SIZE
                        || new_dimensions.1.unsigned_abs() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE;
                    if media_too_big {
                        return Err(TaskError::Error(format!(
                            concat!(
                            "output size <b>{}x{}</b> is too big. ",
                            "This bot only allows generating media no bigger than <b>{}x{}</b>.\n",
                            "For reference, input media's size is <b>{}x{}</b>, ",
                            "so the output must be no bigger than <b>{}%</b> of it."
                        ),
                            new_dimensions.0,
                            new_dimensions.1,
                            MAX_OUTPUT_MEDIA_DIMENSION_SIZE,
                            MAX_OUTPUT_MEDIA_DIMENSION_SIZE,
                            old_dimensions.0,
                            old_dimensions.1,
                            biggest_percentage_that_can_fit(old_dimensions)
                        )));
                    };

                    // Calculate percentages.
                    let p_x = 100.0 * new_dimensions.0 as f32 / original_dimensions.0.get() as f32;
                    let p_y = 100.0 * new_dimensions.1 as f32 / original_dimensions.1.get() as f32;

                    // Only true if the X and Y percentages are close enough.
                    let percentage = if (p_x - p_y).abs() < 1.5 {
                        Some((p_x + p_y) / 2.0)
                    } else {
                        None
                    };

                    (new_dimensions.0, new_dimensions.1, percentage)
                } else {
                    // No width/height nor percentage was specified.
                    // Preset one.

                    // Calculate if the input media is too big.
                    let media_too_big = old_dimensions.0.unsigned_abs()
                        > MAX_OUTPUT_MEDIA_DIMENSION_SIZE
                        || old_dimensions.1.unsigned_abs() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE;
                    let media_too_big_2x = old_dimensions.0.unsigned_abs()
                        > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2
                        || old_dimensions.1.unsigned_abs() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2;
                    let default_percentage =
                        if format == ImageFormat::Preserve || resize_type.is_seam_carve() {
                            // We aren't changing format and/or we want seam carving.
                            // Either way, this means we likely want to resize the media then. Do 50%.
                            if media_too_big_2x {
                                // Image is more than 200% big.
                                // Scalling it to 50% will still be too big. Scale down.
                                biggest_percentage_that_can_fit(old_dimensions)
                            } else {
                                50.0
                            }
                        } else if media_too_big {
                            // We want to preserve the media size, but it's too big.
                            // Scale down.
                            biggest_percentage_that_can_fit(old_dimensions)
                        } else {
                            100.0
                        };

                    if let (Some(new_width), Some(new_height)) = (
                        perc_calc(default_percentage, old_dimensions.0),
                        perc_calc(default_percentage, old_dimensions.1),
                    ) {
                        (new_width, new_height, Some(default_percentage))
                    } else {
                        panic!(
                            "computed bad default percentage {} from dimensions {:?}",
                            default_percentage, old_dimensions
                        );
                    }
                };

                if let ResizeType::SeamCarve {
                    delta_x: dx,
                    rigidity: rg,
                } = &mut resize_type
                {
                    *dx = delta_x;
                    *rg = rigidity;
                }

                // Ensure none of the sizes are zero.

                let (Some(new_x), Some(new_y)) = (
                    NonZeroI32::new(new_dimensions.0),
                    NonZeroI32::new(new_dimensions.1),
                ) else {
                    return Err(TaskError::Error(format!(
                        concat!("output size {}x{} has an empty dimension in it. ",),
                        new_dimensions.0, new_dimensions.1,
                    )));
                };

                if is_video {
                    Ok(Task::VideoResize {
                        new_dimensions: (new_x, new_y),
                        rotation: rot,
                        percentage: new_dimensions.2,
                        resize_type,
                        vibrato_hz,
                        vibrato_depth,
                    })
                } else {
                    Ok(Task::ImageResize {
                        new_dimensions: (new_x, new_y),
                        rotation: rot,
                        percentage: new_dimensions.2,
                        resize_type,
                        format,
                    })
                }
            }
        }
    }
}

#[test]
fn image_resize_parse_test() -> Result<(), TaskError> {
    let x = NonZeroI32::new(512).unwrap();
    let y = NonZeroI32::new(256).unwrap();
    let default = Task::default_image_resize(x, y, ResizeType::Fit, ImageFormat::Preserve);

    let result = default.parse_params("")?;
    let Task::ImageResize { new_dimensions, .. } = result else {
        unreachable!()
    };
    assert_eq!(new_dimensions.0.get(), 256);
    assert_eq!(new_dimensions.1.get(), 128);

    let result = default.parse_params("150%x-100% 86deg webp")?;
    let Task::ImageResize {
        new_dimensions,
        rotation,
        format,
        ..
    } = result
    else {
        unreachable!()
    };
    assert_eq!(new_dimensions.0.get(), 768);
    assert_eq!(new_dimensions.1.get(), -256);
    assert_eq!(rotation, 86.0);
    assert_eq!(format, ImageFormat::Webp);

    Ok(())
}

///////////////////////
////////// HELPER FUNCTIONS
//////////////////////

/// Given a `percentage` and a `input`, sanitize `percentage` and
/// compute a value that is that much percentage of that input.
fn perc_calc(percentage: f32, input: i32) -> Option<i32> {
    let factor = percentage / 100.0;

    if !factor.is_normal() && factor != 0.0 && factor != -0.0 {
        return None;
    }

    let dim = input as f32 * factor;

    Some(dim as i32)
}

#[test]
fn perc_calc_test() {
    assert_eq!(perc_calc(100.0, 144), Some(144));
    assert_eq!(perc_calc(50.0, 144), Some(72));
    assert_eq!(perc_calc(f32::NAN, 144), None);
    assert_eq!(perc_calc(100.0, -144), Some(-144));
    assert_eq!(perc_calc(-50.0, 144), Some(-72));
    assert_eq!(perc_calc(0.0, 144), Some(0));
}

/// Returns rotation in degrees, and a boolean denoting if there
/// was any indication that this value is specifically a rotation.
fn rotation_parser(data: &str) -> Option<(f64, bool)> {
    let new_data_deg = data
        .trim_end_matches("deg")
        .trim_end_matches('d')
        .trim_end_matches('°');
    let new_data_rad = data
        .trim_end_matches("rad")
        .trim_end_matches('r')
        .trim_end_matches('㎭');

    let matched_degrees = new_data_deg.len() != data.len();
    let matched_radians = new_data_rad.len() != data.len();

    let mut matched_anything = matched_degrees || matched_radians;

    if matched_degrees && matched_radians {
        // Both matched. We got nonsense. Bye!
        return None;
    }

    // Default assumption is degrees.
    let in_radians = matched_radians;

    let data = if in_radians {
        new_data_rad
    } else {
        new_data_deg
    };

    // Pi/Tau checks because I know someone will try this lmao
    let mut rotation: f64 = if data == "π" || data == "Π" {
        matched_anything = true;
        if matched_radians {
            // Nonsense.
            return None;
        }
        std::f64::consts::PI.to_degrees()
    } else if data == "τ" || data == "Τ" {
        matched_anything = true;
        if matched_radians {
            // Nonsense.
            return None;
        }
        std::f64::consts::TAU.to_degrees()
    } else {
        data.parse().ok()?
    };

    if in_radians {
        rotation = rotation.to_degrees();
    }

    Some((rotation, matched_anything))
}

#[test]
fn rotation_parser_test() {
    assert_eq!(rotation_parser("60"), Some((60.0, false)));
    assert_eq!(rotation_parser("90deg"), Some((90.0, true)));
    assert_eq!(rotation_parser("9999°"), Some((9999.0, true)));
    assert_eq!(rotation_parser("90deg°rad"), None);
    assert_eq!(rotation_parser("waoidjfa0w9tj3q0j"), None);
}

/// Parses input tuples as a specification of an aspect ratio
/// either cropping or enclosing the starting dimensions, and
/// outputs result.
fn aspect_ratio_parser(
    (a, mut b): (&str, &str),
    starting_dimensions: (i32, i32),
) -> Option<(i32, i32)> {
    let (width, height) = (starting_dimensions.0 as f64, starting_dimensions.1 as f64);
    let ends_in_plus = if b.ends_with('+') {
        b = &b[0..b.len() - 1];
        true
    } else {
        false
    };

    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);

    // Record the signs.
    let x_is_negative = (a.is_sign_negative()) ^ (starting_dimensions.0.is_negative());
    let y_is_negative = (b.is_sign_negative()) ^ (starting_dimensions.1.is_negative());
    // Sanitize them.
    let (a, b) = (a.abs(), b.abs());
    let (width, height) = (width.abs(), height.abs());

    // We now have an aspect ratio. Figure out two resolutions.
    // A smaller one that will fit within the original image snugly,
    // and a bigger one that will fit the original image within itself snugly.

    let fit_by_width = (width, (width * b) / a);

    let fit_by_height = ((height * a) / b, height);

    // True if fit_by_width is the bigger one,
    // i.e. fits the original image within itself.
    let fit_by_width_is_bigger =
        fit_by_width.0 > fit_by_height.0 || fit_by_width.1 > fit_by_height.1;

    // If we don't have a plus, then we need the smaller one.
    // If we do have a plus, then we need the bigger one.
    // Perfect situation for a XOR lol
    let fit_by_height_needed = fit_by_width_is_bigger ^ ends_in_plus;

    let wanted = if fit_by_height_needed {
        fit_by_height
    } else {
        fit_by_width
    };

    let wanted = (wanted.0.round(), wanted.1.round());

    // Resulting dimensions may not fit as u32 or be nonsense. Fail if so.
    if !wanted.0.is_finite()
        || !wanted.1.is_finite()
        || wanted.0 <= 0.0
        || wanted.0 > u32::MAX.into()
        || wanted.1 <= 0.0
        || wanted.1 > u32::MAX.into()
    {
        return None;
    }

    Some((
        wanted.0 as i32 * if x_is_negative { -1 } else { 1 },
        wanted.1 as i32 * if y_is_negative { -1 } else { 1 },
    ))
}

#[test]
fn aspect_ratio_parser_test() {
    assert_eq!(
        aspect_ratio_parser(("1", "1"), (100, 150)),
        Some((100, 100))
    );
    assert_eq!(
        aspect_ratio_parser(("1", "1+"), (100, 150)),
        Some((150, 150))
    );
    assert_eq!(
        aspect_ratio_parser(("2", "3"), (100, 150)),
        Some((100, 150))
    );
    assert_eq!(
        aspect_ratio_parser(("-2", "3"), (100, 150)),
        Some((-100, 150))
    );
    assert_eq!(
        aspect_ratio_parser(("-2", "-3"), (100, 150)),
        Some((-100, -150))
    );
    assert_eq!(
        aspect_ratio_parser(("2", "-3"), (-100, -150)),
        Some((-100, 150))
    );
}

fn single_dimension_parser(data: &str, starting: impl Into<Option<i32>>) -> Option<i32> {
    if let Some(starting) = starting.into() {
        // Check if it's a percentage.
        if let Some(percent) = data.find('%') {
            // Check for garbage after the % sign.
            if (data.len() - '%'.len_utf8()) != percent {
                return None;
            }

            if let Ok(percentage) = data[0..percent].parse::<f32>() {
                return perc_calc(percentage, starting);
            }
        }
    }

    data.parse().ok()
}

#[test]
fn single_dimension_parser_test() {
    assert_eq!(single_dimension_parser("60", None), Some(60));
    assert_eq!(single_dimension_parser("60%", None), None);
    assert_eq!(single_dimension_parser("60", Some(60)), Some(60));
    assert_eq!(single_dimension_parser("60%", Some(60)), Some(36));
    assert_eq!(single_dimension_parser("60%wasd", Some(60)), None);
    assert_eq!(single_dimension_parser("-60", Some(60)), Some(-60));
    assert_eq!(single_dimension_parser("-60%", Some(60)), Some(-36));
    assert_eq!(single_dimension_parser("60", Some(-60)), Some(60));
    assert_eq!(single_dimension_parser("60%", Some(-60)), Some(-36));
}

/// Given a percentage in `data` and starting dimensions,
/// parse and compute the percentage of those dimensions
/// and return result.
fn percentage_parser(data: &str, starting_dimensions: (i32, i32)) -> Option<(i32, i32, f32)> {
    let percent = data.find('%')?;

    // Check for garbage after the % sign.
    if (data.len() - '%'.len_utf8()) != percent {
        return None;
    }

    let percentage: f32 = data[0..percent].parse().ok()?;
    let width = perc_calc(percentage, starting_dimensions.0)?;
    let height = perc_calc(percentage, starting_dimensions.1)?;

    Some((width, height, percentage))
}

#[test]
fn percentage_parser_test() {
    assert_eq!(
        percentage_parser("100%", (100, 150)),
        Some((100, 150, 100.0))
    );
    assert_eq!(
        percentage_parser("150%", (100, 150)),
        Some((150, 225, 150.0))
    );
    assert_eq!(percentage_parser("50%", (100, 150)), Some((50, 75, 50.0)));
    assert_eq!(percentage_parser("50%woidahjsod", (100, 150)), None);
    assert_eq!(percentage_parser("6", (100, 150)), None);

    assert_eq!(
        percentage_parser("-50%", (100, 150)),
        Some((-50, -75, -50.0))
    );
    assert_eq!(
        percentage_parser("-50%", (100, -150)),
        Some((-50, 75, -50.0))
    );
}

/// Given a width and height specification, either in percentages of
/// starting dimensions or absolute values, and those starting dimensions,
/// parse, compute and return result.
fn width_height_parser(data: &str, starting_dimensions: (i32, i32)) -> Option<(i32, i32)> {
    let x = data.find('x')?;
    let w = &data[0..x];
    let h = &data[x + 1..];

    let width = if w.is_empty() {
        starting_dimensions.0
    } else {
        single_dimension_parser(w, starting_dimensions.0)?
    };
    let height = if h.is_empty() {
        starting_dimensions.1
    } else {
        single_dimension_parser(h, starting_dimensions.1)?
    };

    Some((width, height))
}
#[test]
fn width_height_parser_test() {
    // to make it shorter so that rustfmt doesn't split the asserts into many lines lol
    let the_fn = width_height_parser;

    assert_eq!(the_fn("150x150", (100, 150)), Some((150, 150)));
    assert_eq!(the_fn("150x100%", (100, 150)), Some((150, 150)));
    assert_eq!(the_fn("150%x100%", (100, 150)), Some((150, 150)));
    assert_eq!(the_fn("150x0%", (100, 150)), Some((150, 0)));

    assert_eq!(the_fn("-150x150", (100, 150)), Some((-150, 150)));
    assert_eq!(the_fn("-150x-100%", (100, 150)), Some((-150, -150)));
    assert_eq!(the_fn("-150%x100%", (100, -150)), Some((-150, -150)));
    assert_eq!(the_fn("-150x0%", (100, 150)), Some((-150, 0)));

    assert_eq!(the_fn("x150", (100, 150)), Some((100, 150)));
    assert_eq!(the_fn("50%x", (100, 150)), Some((50, 150)));
    assert_eq!(the_fn("x", (100, 150)), Some((100, 150)));
}

/// Given either a percentage or width/height specification
/// and starting dimensions, parse, compute, return output dimensions.
/// Also computes a percentage value of starting dimensions, if applicable.
fn dimensions_parser(
    data: &str,
    starting_dimensions: (i32, i32),
) -> Option<(i32, i32, Option<f32>)> {
    if let Some(x) = width_height_parser(data, starting_dimensions) {
        Some((x.0, x.1, None))
    } else {
        let result = percentage_parser(data, starting_dimensions)?;
        Some((result.0, result.1, Some(result.2)))
    }
}

/// Given a width and a height, compute the maximum factor, as a percentage (*100),
/// that can fit within a square with length side of [`MAX_OUTPUT_MEDIA_DIMENSION_SIZE`].
fn biggest_percentage_that_can_fit((width, height): (i32, i32)) -> f32 {
    // May be a bit approximate, but meh.
    let smallest_width_percent = (MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 100) / width.unsigned_abs();
    let smallest_height_percent = (MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 100) / height.unsigned_abs();

    let biggest_percent = u32::min(smallest_width_percent, smallest_height_percent);
    biggest_percent as f32
}
