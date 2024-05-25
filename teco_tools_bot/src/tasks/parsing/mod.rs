pub mod tokenizer;

use super::*;
use html_escape::encode_text;
use tokenizer::{Token, Tokenizer};

pub static MAX_OUTPUT_MEDIA_DIMENSION_SIZE: u32 = 2048;

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
                            "<code>format</code>: Format to save the image in: jpeg, webp or preserve\n",
                            "<code>WxH</code>: Width and height of the output image, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "<code>rot</code>: Rotate the image by this much after distorting.\n",
                            "<code>delta_x</code>: Maximum seam transversal step. 0 means straight seams. Default is 2. ",
                            "Can't be less than -4 or bigger than 4.\n",
                            "<code>rigidity</code>: Bias for non-straight seams. Default is 0. ",
                            "Same requirements as with <code>delta_x</code>.\n",
                            "<code>format</code>: Output image format. Can be \"webp\" or \"jpg\"."
                            ),
                    ResizeType::Stretch | ResizeType::Fit | ResizeType::Crop =>
                        concat!(
                            "<code>WxH</code>: Width and height of the output image, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
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
                            "<code>WxH</code>: Width and height of the output video, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "<code>rot</code>: Rotate the video by this much after distorting.\n",
                            "<code>delta_x</code>: Maximum seam transversal step. 0 means straight seams. Default is 2. ",
                            "Can't be less than -4 or bigger than 4.\n",
                            "<code>rigidity</code>: Bias for non-straight seams. Default is 0. ",
                            "Same requirements as with <code>delta_x</code>."

                        ),
                    ResizeType::Stretch | ResizeType::Fit | ResizeType::Crop => concat!(
                            "<code>WxH</code>: Width and height of the output video, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or bigger than 2048x2048; OR\n",
                            "<code>W:H</code>: Aspect ratio cropping the original size, or expanding it if + is appended.\n",
                            "<code>rot</code>: Rotate the video by this much after resizing.\n",
                            "<code>method</code>: Resize method. Can only be \"fit\", \"stretch\" or \"crop\".\n",
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
                new_dimensions: old_dimensions,
                rotation,
                percentage: _,
                format: _,
                mut resize_type,
            }
            | Task::VideoResize {
                new_dimensions: old_dimensions,
                rotation,
                percentage: _,
                mut resize_type,
            } => {
                let (is_video, mut format) = if let Task::ImageResize { format, .. } = self {
                    (false, *format)
                } else {
                    (true, ImageFormat::Preserve)
                };

                if let ResizeType::ToSticker | ResizeType::ToCustomEmoji = resize_type {
                    return Ok(self.clone());
                }
                let mut rot = *rotation;
                // The -1.0 is a "default"; if it stays that way after parsing params,
                // then it will be autocalculated at the end of the function
                let mut new_dimensions = (old_dimensions.0, old_dimensions.1, -1.0);
                let ResizeType::SeamCarve {
                    mut delta_x,
                    mut rigidity,
                } = ResizeType::default_seam_carve()
                else {
                    unreachable!();
                };

                let dimensions_parser_err =
                    |data| dimensions_parser(data, *old_dimensions).ok_or(());

                let sanitized_f64_parser = |val: &str| -> Result<f64, ()> {
                    let result: f64 = val.parse().map_err(|_| ())?;

                    if result.is_finite() && (-4.0..=4.0).contains(&result) {
                        Ok(result)
                    } else {
                        Err(())
                    }
                };

                for param in params {
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
                        parse_keyval_param_with_parser!(param, delta_x, sanitized_f64_parser, help);
                        parse_keyval_param_with_parser!(
                            param,
                            rigidity,
                            sanitized_f64_parser,
                            help
                        );
                    } else {
                        parse_plain_param_optional!(param, resize_type, help);
                    }

                    if let Token::KeyVal(k, v) = param {
                        let v = (k, v);
                        // Try to parse it as an aspect ratio lol
                        if let Some(parse) = aspect_ratio_parser(v, *old_dimensions) {
                            new_dimensions = (parse.0, parse.1, 0.0);
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

                // Calculate if the media after any specified rescaling is too big.
                let media_too_big = new_dimensions.0.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE
                    || new_dimensions.1.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE;
                let media_too_big_2x = new_dimensions.0.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2
                    || new_dimensions.1.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2;

                fn smallest_percentage_that_can_fit(
                    (width, height): &(NonZeroU32, NonZeroU32),
                ) -> f32 {
                    // May be a bit approximate, but meh.
                    let smallest_width_percent =
                        (MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 100) / width.get();
                    let smallest_height_percent =
                        (MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 100) / height.get();

                    let smallest_percent =
                        u32::min(smallest_width_percent, smallest_height_percent);
                    smallest_percent as f32
                }

                if new_dimensions.2 == -1.0 {
                    // No width/height nor percentage was specified.
                    // Preset one.

                    let default_percentage =
                        if format == ImageFormat::Preserve || resize_type.is_seam_carve() {
                            // We aren't changing format and/or we want seam carving.
                            // Either way, this means we likely want to resize the media then. Do 50%.
                            if media_too_big_2x {
                                // Image is more than 200% big.
                                // Scalling it to 50% will still be too big. Scale down.
                                smallest_percentage_that_can_fit(old_dimensions)
                            } else {
                                50.0
                            }
                        } else if media_too_big {
                            // We want to preserve the media size, but it's too big.
                            // Scale down.
                            smallest_percentage_that_can_fit(old_dimensions)
                        } else {
                            100.0
                        };

                    if let (Some(new_width), Some(new_height)) = (
                        perc_calc(default_percentage, old_dimensions.0),
                        perc_calc(default_percentage, old_dimensions.1),
                    ) {
                        new_dimensions = (new_width, new_height, default_percentage);
                    }
                } else if media_too_big {
                    return Err(TaskError::Error(format!(
                        concat!(
                            "output size {}x{} is too big. ",
                            "This bot only allows generating media no bigger than {}x{}.\n",
                            "For reference, input media's size is {}x{}, so the output must be no bigger than {}% of it."
                        ),
                        new_dimensions.0,
                        new_dimensions.1,
                        MAX_OUTPUT_MEDIA_DIMENSION_SIZE,
                        MAX_OUTPUT_MEDIA_DIMENSION_SIZE,
                        old_dimensions.0,
                        old_dimensions.1,
                        smallest_percentage_that_can_fit(old_dimensions)
                    )));
                }

                if let ResizeType::SeamCarve {
                    delta_x: dx,
                    rigidity: rg,
                } = &mut resize_type
                {
                    *dx = delta_x;
                    *rg = rigidity;
                }
                if is_video {
                    Ok(Task::VideoResize {
                        new_dimensions: (new_dimensions.0, new_dimensions.1),
                        rotation: rot,
                        percentage: new_dimensions.2,
                        resize_type,
                    })
                } else {
                    Ok(Task::ImageResize {
                        new_dimensions: (new_dimensions.0, new_dimensions.1),
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

///////////////////////
////////// HELPER FUNCTIONS
//////////////////////

/// Given a `percentage` and a `input`, sanitize `percentage` and
/// compute a value that is that much percentage of that input.
fn perc_calc(percentage: f32, input: NonZeroU32) -> Option<NonZeroU32> {
    let factor = percentage / 100.0;

    if !factor.is_normal() || factor <= 0.0 {
        return None;
    }

    let dim = (input.get() as f32 * factor) as u32;

    dim.try_into().ok()
}

#[test]
fn perc_calc_test() {
    let input = NonZeroU32::new(144).unwrap();
    assert_eq!(perc_calc(100.0, input), Some(input));
    let result = NonZeroU32::new(72).unwrap();
    assert_eq!(perc_calc(50.0, input), Some(result));
    assert_eq!(perc_calc(f32::NAN, input), None);
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
    starting_dimensions: (NonZeroU32, NonZeroU32),
) -> Option<(NonZeroU32, NonZeroU32)> {
    let (width, height) = (
        starting_dimensions.0.get() as f64,
        starting_dimensions.1.get() as f64,
    );
    let ends_in_plus = if b.ends_with('+') {
        b = &b[0..b.len() - 1];
        true
    } else {
        false
    };

    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);

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

    // This is very unlikely to fail. It was already parsed as NonZeroU32 in the
    // first place, and the math shouldn't exceed the limits.
    // Still, maybe the user would specify insanely huge numbers for an aspect
    // ratio? lol
    Some((
        NonZeroU32::new(wanted.0 as u32)?,
        NonZeroU32::new(wanted.1 as u32)?,
    ))
}

#[test]
fn aspect_ratio_parser_test() {
    let start_dim = (NonZeroU32::new(100).unwrap(), NonZeroU32::new(150).unwrap());
    let crop_1_1 = (NonZeroU32::new(100).unwrap(), NonZeroU32::new(100).unwrap());
    let extend_1_1 = (NonZeroU32::new(150).unwrap(), NonZeroU32::new(150).unwrap());

    assert_eq!(aspect_ratio_parser(("1", "1"), start_dim), Some(crop_1_1));
    assert_eq!(
        aspect_ratio_parser(("1", "1+"), start_dim),
        Some(extend_1_1)
    );
    assert_eq!(aspect_ratio_parser(("2", "3"), start_dim), Some(start_dim));
}

fn single_dimension_parser(
    data: &str,
    starting: impl Into<Option<NonZeroU32>>,
) -> Option<NonZeroU32> {
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
    let woot: NonZeroU32 = data.parse().ok()?;
    Some(woot)
}

#[test]
fn single_dimension_parser_test() {
    let nzu32_60 = NonZeroU32::new(60).unwrap();
    let nzu32_36 = NonZeroU32::new(36).unwrap();
    assert_eq!(single_dimension_parser("60", None), Some(nzu32_60));
    assert_eq!(single_dimension_parser("60%", None), None);
    assert_eq!(
        single_dimension_parser("60", Some(nzu32_60)),
        Some(nzu32_60)
    );
    assert_eq!(
        single_dimension_parser("60%", Some(nzu32_60)),
        Some(nzu32_36)
    );
    assert_eq!(single_dimension_parser("60%wasd", Some(nzu32_60)), None);
}

/// Given a percentage in `data` and starting dimensions,
/// parse and compute the percentage of those dimensions
/// and return result.
fn percentage_parser(
    data: &str,
    starting_dimensions: (NonZeroU32, NonZeroU32),
) -> Option<(NonZeroU32, NonZeroU32, f32)> {
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
    let start_dim = (NonZeroU32::new(100).unwrap(), NonZeroU32::new(150).unwrap());
    let dim_150percent = (
        NonZeroU32::new(150).unwrap(),
        NonZeroU32::new(225).unwrap(),
        150.0,
    );
    let dim_50percent = (
        NonZeroU32::new(50).unwrap(),
        NonZeroU32::new(75).unwrap(),
        50.0,
    );
    assert_eq!(
        percentage_parser("100%", start_dim),
        Some((start_dim.0, start_dim.1, 100.0))
    );
    assert_eq!(percentage_parser("150%", start_dim), Some(dim_150percent));
    assert_eq!(percentage_parser("50%", start_dim), Some(dim_50percent));
    assert_eq!(percentage_parser("50%woidahjsod", start_dim), None);
    assert_eq!(percentage_parser("6", start_dim), None);
}

/// Given a width and height specification, either in percentages of
/// starting dimensions or absolute values, and those starting dimensions,
/// parse, compute and return result.
fn width_height_parser(
    data: &str,
    starting_dimensions: (NonZeroU32, NonZeroU32),
) -> Option<(NonZeroU32, NonZeroU32)> {
    let x = data.find('x')?;
    let w = &data[0..x];
    let h = &data[x + 1..];
    // It's width and height.
    // Try in pixels...
    if let Some(width) = single_dimension_parser(w, starting_dimensions.0) {
        if let Some(height) = single_dimension_parser(h, starting_dimensions.1) {
            return Some((width, height));
        }
    }
    let width = single_dimension_parser(w, starting_dimensions.0)?;
    let height = single_dimension_parser(h, starting_dimensions.1)?;

    Some((width, height))
}
#[test]
fn width_height_parser_test() {
    let start_dim = (NonZeroU32::new(100).unwrap(), NonZeroU32::new(150).unwrap());
    let dim_150x150 = (NonZeroU32::new(150).unwrap(), NonZeroU32::new(150).unwrap());

    // to make it shorter so that rustfmt doesn't split the asserts into many lines lol
    let the_fn = width_height_parser;

    assert_eq!(the_fn("150x150", start_dim), Some(dim_150x150));
    assert_eq!(the_fn("150x100%", start_dim), Some(dim_150x150));
    assert_eq!(the_fn("150%x100%", start_dim), Some(dim_150x150));
    assert_eq!(the_fn("150x0%", start_dim), None);
}

/// Given either a percentage or width/height specification
/// and starting dimensions, parse, compute, return output dimensions.
/// Also computes a percentage value, if applicable, else returns 0.0 for it.
fn dimensions_parser(
    data: &str,
    starting_dimensions: (NonZeroU32, NonZeroU32),
) -> Option<(NonZeroU32, NonZeroU32, f32)> {
    if let Some(x) = width_height_parser(data, starting_dimensions) {
        Some((x.0, x.1, 0.0))
    } else {
        percentage_parser(data, starting_dimensions)
    }
}
