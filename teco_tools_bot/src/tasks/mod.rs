pub mod completion;
pub mod param_parsing;
pub mod taskman;

use std::{fmt::Display, num::NonZeroU32, str::FromStr};

use html_escape::encode_text;
use serde::{Deserialize, Serialize};
use teloxide::{types::Message, Bot};

use crate::handlers::functions::{TaskFuture, TaskParams};
use param_parsing::ParamParser;

use taskman::Taskman;

pub static MAX_OUTPUT_MEDIA_DIMENSION_SIZE: u32 = 2048;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ResizeType {
    Stretch,
    Fit,
    ToSticker,
    SeamCarve { delta_x: f64, rigidity: f64 },
}

impl ResizeType {
    pub fn default_seam_carve() -> Self {
        Self::SeamCarve {
            delta_x: 1.0,
            rigidity: 0.0,
        }
    }
    pub fn is_seam_carve(&self) -> bool {
        matches!(self, Self::SeamCarve { .. })
    }
}

impl FromStr for ResizeType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "fit" => Ok(Self::Fit),
            "stretch" => Ok(Self::Stretch),
            _ => Err(()),
        }
    }
}

impl Display for ResizeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fit => write!(f, "Fit"),
            Self::Stretch => write!(f, "Stretch"),
            Self::SeamCarve { delta_x, rigidity } => {
                writeln!(f, "Seam Carving")?;
                writeln!(f, "<b>delta_x</b>: {}", delta_x)?;
                writeln!(f, "<b>rigidity</b>: {}", rigidity)
            }
            Self::ToSticker => write!(f, "To sticker"),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ImageFormat {
    Preserve,
    Webp,
    Jpeg,
    Bmp,
    Png,
}

impl ImageFormat {
    /// Returns `true` if the format supports alpha transparency.
    /// BMP doesn't count, but [`Self::Preserve`] does.
    pub fn supports_alpha_transparency(&self) -> bool {
        match self {
            Self::Preserve => true,
            Self::Webp => true,
            Self::Jpeg => false,
            Self::Bmp => false,
            Self::Png => true,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preserve => "Preserve",
            Self::Webp => "WebP",
            Self::Jpeg => "JPEG",
            Self::Bmp => "BMP",
            Self::Png => "PNG",
        }
    }
}

impl Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ImageFormat {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "preserve" => Ok(Self::Preserve),
            "webp" => Ok(Self::Webp),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            // BMP and PNG are intentionally ignored as they're for internal purposes only
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Task {
    Amogus {
        amogus: i32,
    },
    Resize {
        new_dimensions: (NonZeroU32, NonZeroU32),
        percentage: f32,
        format: ImageFormat,
        resize_type: ResizeType,
    },
}

impl Task {
    pub fn parse_task<'a>(
        taskman: &'a Taskman,
        bot: &'a Bot,
        message: &'a Message,
    ) -> Option<TaskFuture<'a>> {
        let task_params = TaskParams::new(taskman, bot, message)?;
        task_params.make_task()
    }

    pub fn param_help(&self) -> &'static str {
        match self {
            Task::Amogus { .. } => {
                "<code>amogus</code>: How much amogus. Negative numbers are allowed."
            }
            Task::Resize { resize_type, ..} => {
                match resize_type {
                    ResizeType::ToSticker => "",
                    ResizeType::SeamCarve { .. } =>
                        concat!(
                            "<code>format</code>: Format to save the image in: png, jpeg or preserve\n",
                            "<code>WxH</code>: Width and height of the output image, can't be 0 or too big; OR\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or too big\n",
                            "<code>delta_x</code>: Maximum seam transversal step. 0 means straight seams. Default is 1. ",
                            "Can't be less than -4 or bigger than 4.\n",
                            "<code>rigidity</code>: Bias for non-straight seams. Default is 0. ",
                            "Same requirements as with <code>delta_x</code>."
                            ),
                    ResizeType::Stretch | ResizeType::Fit =>
                        concat!(
                            "<code>WxH</code>: Width and height of the output image, can't be 0 or too big; OR\n",
                            "<code>method</code>: Resize method. Can only be \"fit\" or \"stretch\".\n",
                            "<code>size%</code>: Percentage of the original size, can't be 0 or too big\n",
                            ),
                }
            }
        }
    }

    pub fn write_params(
        &self,
        mut output: impl std::fmt::Write,
        header: bool,
        editable: bool,
    ) -> Result<(), std::fmt::Error> {
        let mut wrote_first_param = false;
        macro_rules! write_param {
            ($name:expr, $value:expr) => {{
                #[allow(unused_assignments)]
                if header && !wrote_first_param {
                    write!(output, "Parameters")?;
                    if editable {
                        write!(output, " (edit message to change)")?;
                    }
                    writeln!(output, ":")?;
                    wrote_first_param = true;
                }
                writeln!(output, "<b>{}</b>: {}", $name, $value.to_string())
            }};
        }

        macro_rules! wp {
            ($name:expr) => {
                write_param!(stringify!($name), $name)
            };
        }

        match self {
            Task::Amogus { amogus } => wp!(amogus),
            Task::Resize {
                new_dimensions,
                percentage,
                format,
                resize_type,
            } => {
                if *resize_type == ResizeType::ToSticker {
                    return Ok(());
                }
                write_param!("Format", format)?;
                if *resize_type == ResizeType::Fit {
                    write!(
                        output,
                        "<b>Size to fit</b>: {}x{}",
                        new_dimensions.0, new_dimensions.1
                    )?;
                } else {
                    write!(
                        output,
                        "<b>Size</b>: {}x{}",
                        new_dimensions.0, new_dimensions.1
                    )?;
                }
                if *percentage != 0.0 {
                    write!(output, ", or {}%", percentage)?;
                }
                writeln!(output)?;
                write_param!("Resize type", resize_type)?;
                Ok(())
            }
        }
    }

    pub fn produce_queue_message(&self, queue_size: u32) -> String {
        let mut response = if queue_size == 0 {
            String::from("Working on your task now...\n")
        } else {
            format!("Task accepted. Position in queue: {}\n", queue_size)
        };
        self.write_params(&mut response, true, queue_size != 0)
            .unwrap();
        response
    }
}

////////////////////////////
////////////// DEFAULT IMPLEMENTATIONS
////////////////////////////

impl Task {
    pub fn default_to_sticker() -> Task {
        Task::Resize {
            new_dimensions: (NonZeroU32::new(512).unwrap(), NonZeroU32::new(512).unwrap()),
            percentage: 100.0,
            format: ImageFormat::Webp,
            resize_type: ResizeType::ToSticker,
        }
    }
    pub fn default_amogus() -> Task {
        Task::Amogus { amogus: 1 }
    }
    pub fn default_resize(
        width: NonZeroU32,
        height: NonZeroU32,
        resize_type: ResizeType,
        format: ImageFormat,
    ) -> Task {
        Task::Resize {
            new_dimensions: (width, height),
            percentage: 100.0,
            format,
            resize_type,
        }
    }
}

////////////////////////////
////////////// PARAMETER PARSERS
////////////////////////////

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
        if let Err(value) = $input {
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
        if let Err(value) = $input {
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
        let Ok((key, value)) = $input else {
            return Err(TaskError::Error(format!(
                "can't parse <code>{}</code> as a parameter.{}{}",
                encode_text($input.unwrap_err()),
                PARAM_HELP,
                $help
            )));
        };

        if key == stringify!($name).to_lowercase() {
            parse_plain_param_with_parser_mandatory!(Err::<(), _>(value), $name, $parser, $help);
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
            Ok((key, val)) => format!(
                "unexpected parameter <code>{}</code> with value <code>{}</code>{}{}",
                encode_text(key),
                encode_text(val),
                PARAM_HELP,
                $help
            ),
            Err(plain) => format!(
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
    pub fn parse_params(&self, params: ParamParser) -> Result<Task, TaskError> {
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
            Task::Resize {
                new_dimensions: old_dimensions,
                percentage: _,
                mut format,
                mut resize_type,
            } => {
                if resize_type == ResizeType::ToSticker {
                    return Ok(self.clone());
                }
                // This is named "percentage" purely to make my life easier with
                // the parser logic lol
                //
                // The -1.0 is a "default"; if it stays that way after parsing params,
                // then it'll be set to default
                let mut percentage = (old_dimensions.0, old_dimensions.1, -1.0);
                let ResizeType::SeamCarve {
                    mut delta_x,
                    mut rigidity,
                } = ResizeType::default_seam_carve()
                else {
                    unreachable!();
                };

                fn single_dimension_parser(data: &str) -> Option<NonZeroU32> {
                    let woot: NonZeroU32 = data.parse().ok()?;
                    Some(woot)
                }
                fn percentage_calculator(
                    percentage: f32,
                    starting_dimensions: (NonZeroU32, NonZeroU32),
                ) -> Option<(NonZeroU32, NonZeroU32, f32)> {
                    let factor = percentage / 100.0;

                    if !factor.is_normal() || factor <= 0.0 {
                        return None;
                    }

                    let width = (starting_dimensions.0.get() as f32 * factor) as u32;
                    let height = (starting_dimensions.1.get() as f32 * factor) as u32;

                    Some((width.try_into().ok()?, height.try_into().ok()?, percentage))
                }

                fn percentage_parser(
                    data: &str,
                    starting_dimensions: (NonZeroU32, NonZeroU32),
                ) -> Option<(NonZeroU32, NonZeroU32, f32)> {
                    let percent = data.find('%')?;

                    let percentage: f32 = data[0..percent].parse().ok()?;
                    percentage_calculator(percentage, starting_dimensions)
                }

                fn width_height_parser(
                    data: &str,
                    starting_dimensions: (NonZeroU32, NonZeroU32),
                ) -> Option<(NonZeroU32, NonZeroU32)> {
                    let x = data.find('x')?;
                    let w = &data[0..x];
                    let h = &data[x + 1..];
                    // It's width and height.
                    // Try in pixels...
                    if let Some(width) = single_dimension_parser(w) {
                        if let Some(height) = single_dimension_parser(h) {
                            return Some((width, height));
                        }
                    }

                    // Maybe it's in percentages?
                    // Cobble it together lol
                    let (width, _, _) = percentage_parser(w, starting_dimensions)?;
                    let (_, height, _) = percentage_parser(h, starting_dimensions)?;

                    Some((width, height))
                }

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
                    parse_plain_param_optional!(param, format, help);
                    parse_plain_param_with_parser_optional!(
                        param,
                        percentage,
                        dimensions_parser_err,
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
                    parse_keyval_param!(param, format, help);
                    parse_keyval_param_with_parser!(param, percentage, dimensions_parser_err, help);
                    parse_stop!(param, help);
                }

                // Calculate if the image after any specified rescaling is too big.
                let image_too_big = percentage.0.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE
                    || percentage.1.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE;
                let image_too_big_2x = percentage.0.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2
                    || percentage.1.get() > MAX_OUTPUT_MEDIA_DIMENSION_SIZE * 2;

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

                if percentage.2 == -1.0 {
                    // No width/height nor percentage was specified.
                    // Preset one.

                    let default_percentage =
                        if format == ImageFormat::Preserve || resize_type.is_seam_carve() {
                            // We aren't changing format and/or we want seam carving.
                            // Either way, this means we likely want to resize the image then. Do 50%.
                            if image_too_big_2x {
                                // Image is more than 200% big.
                                // Scalling it to 50% will still be too big. Scale down.
                                smallest_percentage_that_can_fit(old_dimensions)
                            } else {
                                50.0
                            }
                        } else if image_too_big {
                            // We want to preserve the image size, but it's too big.
                            // Scale down.
                            smallest_percentage_that_can_fit(old_dimensions)
                        } else {
                            100.0
                        };

                    if let Some(parsed) = percentage_calculator(default_percentage, *old_dimensions)
                    {
                        percentage = parsed;
                    }
                } else if image_too_big {
                    // Error that image is too big.
                    return Err(TaskError::Error(format!(
                        concat!(
                            "output size {}x{} is too big. ",
                            "This bot only allows generating images no bigger than {}x{}.\n",
                            "For reference, input image's size is {}x{}, so the output must be no bigger than {}% of it."
                        ),
                        percentage.0,
                        percentage.1,
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
                Ok(Task::Resize {
                    new_dimensions: (percentage.0, percentage.1),
                    percentage: percentage.2,
                    resize_type,
                    format,
                })
            }
        }
    }
}
