pub mod completion;
pub mod parsing;
pub mod taskman;

use std::{fmt::Display, num::NonZeroI32, str::FromStr};

use serde::{Deserialize, Serialize};
use teloxide::{
    types::{Me, Message},
    Bot,
};

use crate::handlers::commands::{TaskFuture, TaskParams};

use taskman::Taskman;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ResizeType {
    Stretch,
    Fit,
    Crop,
    ToSticker,
    ToCustomEmoji,
    SeamCarve { delta_x: f64, rigidity: f64 },
}

impl ResizeType {
    pub fn default_seam_carve() -> Self {
        Self::SeamCarve {
            delta_x: 2.0,
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
            "crop" => Ok(Self::Crop),
            _ => Err(()),
        }
    }
}

impl Display for ResizeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fit => write!(f, "Fit"),
            Self::Stretch => write!(f, "Stretch"),
            Self::Crop => write!(f, "Crop"),
            Self::SeamCarve { delta_x, rigidity } => {
                writeln!(f, "Seam Carving")?;
                writeln!(f, "<b>delta_x</b>: {}", delta_x)?;
                write!(f, "<b>rigidity</b>: {}", rigidity)
            }
            Self::ToSticker => write!(f, "To sticker"),
            Self::ToCustomEmoji => write!(f, "To custom emoji"),
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
pub enum VideoTypePreference {
    #[default]
    Preserve,
    Video,
    Gif,
    //VideoSticker // Maybe in the future lol
}

impl VideoTypePreference {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Preserve => "Preserve",
            Self::Video => "Video",
            Self::Gif => "GIF",
        }
    }
}

impl FromStr for VideoTypePreference {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "preserve" => Ok(Self::Preserve),
            "video" => Ok(Self::Video),
            "gif" => Ok(Self::Gif),
            _ => Err(()),
        }
    }
}

impl Display for VideoTypePreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Task {
    Amogus {
        amogus: i32,
    },
    ImageResize {
        /// Signed integer to allow specifying negative resolutions
        /// as a way to signify mirroring the image.
        new_dimensions: (NonZeroI32, NonZeroI32),
        rotation: f64,
        percentage: Option<f32>,
        format: ImageFormat,
        resize_type: ResizeType,
    },
    VideoResize {
        /// Signed integer to allow specifying negative resolutions
        /// as a way to signify mirroring the image.
        new_dimensions: (NonZeroI32, NonZeroI32),
        rotation: f64,
        percentage: Option<f32>,
        resize_type: ResizeType,
        vibrato_hz: f64,
        vibrato_depth: f64,
        #[serde(default = "VideoTypePreference::default")]
        type_pref: VideoTypePreference,
    },
    /// Optical Character Recognition, i.e. extracting text from an image
    Ocr,
}

impl Task {
    pub fn parse_task<'a>(
        taskman: &'a Taskman,
        bot: &'a Bot,
        bot_me: &'a Me,
        message: &'a Message,
    ) -> Option<TaskFuture<'a>> {
        let task_params = TaskParams::new(taskman, bot, bot_me, message)?;
        task_params.make_task()
    }

    pub fn write_params(
        &self,
        mut output: impl std::fmt::Write,
        header: bool,
        editable: bool,
    ) -> Result<(), std::fmt::Error> {
        macro_rules! write_header {
            () => {{
                #[allow(unused_assignments)]
                if header {
                    write!(output, "Parameters")?;
                    if editable {
                        write!(output, " (edit message to change)")?;
                    }
                    writeln!(output, ":")?;
                }
            }};
        }
        macro_rules! write_param {
            ($name:expr, $value:expr) => {{
                writeln!(output, "<b>{}</b>: {}", $name, $value.to_string())
            }};
        }

        macro_rules! wp {
            ($name:expr) => {
                write_param!(stringify!($name), $name)
            };
        }

        match self {
            Task::Amogus { amogus } => {
                write_header!();
                wp!(amogus)
            }
            Task::VideoResize {
                new_dimensions,
                rotation,
                percentage,
                resize_type,
                vibrato_hz: _,
                vibrato_depth: _,
                type_pref: _,
            }
            | Task::ImageResize {
                new_dimensions,
                rotation,
                percentage,
                format: _,
                resize_type,
            } => {
                if let ResizeType::ToSticker | ResizeType::ToCustomEmoji = resize_type {
                    return Ok(());
                }

                write_header!();

                if let Task::ImageResize { format, .. } = self {
                    write_param!("Format", format)?;
                }
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
                if let Some(percentage) = percentage {
                    write!(output, ", or {}%", percentage)?;
                }
                writeln!(output)?;
                writeln!(output, "<b>Rotation</b>: {}°", rotation)?;
                write_param!("Resize method", resize_type)?;

                if let Task::VideoResize {
                    vibrato_hz,
                    vibrato_depth,
                    type_pref,
                    ..
                } = self
                {
                    write_param!("Media type", type_pref)?;
                    wp!(vibrato_hz)?;
                    wp!(vibrato_depth)?;
                };
                Ok(())
            }
            Task::Ocr => Ok(()),
        }
    }

    /// Specifying `queue_size` as `None` produces a message about
    /// the task being delayed instead.
    pub fn produce_queue_message(
        &self,
        queue_size: Option<u32>,
        progress_info: Option<&str>,
    ) -> String {
        //let mut response = if queue_size == 0 {
        //    if let Some(progress) = progress_info {
        //        format!("Working on your task now... {}\n", progress)
        //    } else {
        //        String::from("Working on your task now...\n")
        //    }
        //} else {
        //    format!("Task accepted. Position in queue: {}\n", queue_size)
        //};

        let mut response = match queue_size {
            Some(0) => {
                if let Some(progress) = progress_info {
                    format!("Working on your task now... {}\n", progress)
                } else {
                    String::from("Working on your task now...\n")
                }
            }
            Some(s) => format!("Task accepted. Position in queue: {}\n", s),
            None => "Task accepted. Waiting for this chat's slow mode...\n".to_string(),
        };

        self.write_params(&mut response, true, queue_size != Some(0))
            .unwrap();

        response += "\n<a href=\"https://boosty.to/architector_4\">(Consider supporting? 👉👈)</a>";
        response
    }
}

////////////////////////////
////////////// DEFAULT IMPLEMENTATIONS
////////////////////////////

impl Task {
    pub fn default_to_sticker() -> Task {
        Task::ImageResize {
            new_dimensions: (NonZeroI32::new(512).unwrap(), NonZeroI32::new(512).unwrap()),
            rotation: 0.0,
            percentage: None,
            format: ImageFormat::Webp,
            resize_type: ResizeType::ToSticker,
        }
    }
    pub fn default_to_custom_emoji() -> Task {
        Task::ImageResize {
            new_dimensions: (NonZeroI32::new(100).unwrap(), NonZeroI32::new(100).unwrap()),
            rotation: 0.0,
            percentage: None,
            format: ImageFormat::Webp,
            resize_type: ResizeType::ToCustomEmoji,
        }
    }
    pub fn default_amogus() -> Task {
        Task::Amogus { amogus: 1 }
    }
    pub fn default_image_resize(
        width: NonZeroI32,
        height: NonZeroI32,
        resize_type: ResizeType,
        format: ImageFormat,
    ) -> Task {
        Task::ImageResize {
            new_dimensions: (width, height),
            rotation: 0.0,
            percentage: Some(100.0),
            format,
            resize_type,
        }
    }
    pub fn default_video_resize(
        width: NonZeroI32,
        height: NonZeroI32,
        resize_type: ResizeType,
        type_pref: VideoTypePreference,
    ) -> Task {
        Task::VideoResize {
            new_dimensions: (width, height),
            rotation: 0.0,
            percentage: Some(100.0),
            resize_type,
            vibrato_hz: if resize_type.is_seam_carve() {
                7.0
            } else {
                0.0
            },
            vibrato_depth: if resize_type.is_seam_carve() {
                1.0
            } else {
                0.0
            },
            type_pref,
        }
    }
    pub fn default_ocr() -> Task {
        Task::Ocr
    }
}
