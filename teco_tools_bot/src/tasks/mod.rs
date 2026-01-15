pub mod completion;
pub mod parsing;
pub mod taskman;

use std::{f64::consts::TAU, fmt::Display, num::NonZeroU8, str::FromStr};

use serde::{Deserialize, Serialize};
use teloxide::{
    types::{FileMeta, Me, Message},
    Bot,
};

use crate::handlers::commands::{TaskFuture, TaskParams};

use taskman::Taskman;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub enum ResizeCurve {
    #[default]
    Constant,
    Rising,
    Falling,
    Loop,
    LoopB,
}

impl ResizeCurve {
    pub fn apply_resize_for(
        self,
        current_frame: usize,
        total_frames: u64,
        start: f64,
        end: f64,
    ) -> f64 {
        let progress = current_frame as f64 / total_frames as f64;

        let factor = match self {
            Self::Constant => 1.0,
            Self::Rising => progress,
            Self::Falling => 1.0 - progress,
            Self::Loop => f64::sin((progress - 0.25) * TAU) * 0.5 + 0.5,
            Self::LoopB => f64::sin((progress - 0.75) * TAU) * 0.5 + 0.5,
        };

        start + (end - start) * factor
    }
}

impl FromStr for ResizeCurve {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("constant") {
            Ok(Self::Constant)
        } else if s.eq_ignore_ascii_case("rising") {
            Ok(Self::Rising)
        } else if s.eq_ignore_ascii_case("falling") {
            Ok(Self::Falling)
        } else if s.eq_ignore_ascii_case("loop") {
            Ok(Self::Loop)
        } else if s.eq_ignore_ascii_case("loopb") {
            Ok(Self::LoopB)
        } else {
            Err(())
        }
    }
}

impl ResizeCurve {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Constant => "Constant",
            Self::Rising => "Rising",
            Self::Falling => "Falling",
            Self::Loop => "Loop",
            Self::LoopB => "LoopB",
        }
    }
}

impl Display for ResizeCurve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ResizeType {
    Stretch,
    Fit,
    Crop,
    ToSticker,
    ToCustomEmoji,
    ToSpoileredMedia { caption: String },
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
    pub fn strip_caption(&mut self) {
        if let ResizeType::ToSpoileredMedia { caption } = self {
            *caption = String::new();
        }
    }
}

impl FromStr for ResizeType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("fit") {
            Ok(Self::Fit)
        } else if s.eq_ignore_ascii_case("stretch") {
            Ok(Self::Stretch)
        } else if s.eq_ignore_ascii_case("crop") {
            Ok(Self::Crop)
        } else {
            Err(())
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
                writeln!(f, "<b>delta_x</b>: {delta_x}")?;
                write!(f, "<b>rigidity</b>: {rigidity}")
            }
            Self::ToSticker => write!(f, "To sticker"),
            Self::ToCustomEmoji => write!(f, "To custom emoji"),
            Self::ToSpoileredMedia { .. } => write!(f, "To a spoilered media"),
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

    /// Returns a name of a fitting ffmpeg codec for this format.
    ///
    /// # Panics
    ///
    /// Panics if [`ImageFormat::Preserve`] is sent.
    pub fn as_str_for_ffmpeg(&self) -> &'static str {
        match self {
            Self::Preserve => panic!("Tried to run as_str_for_ffmpeg for ImageFormat::Preserve"),
            Self::Webp => "webp",
            Self::Jpeg => "jpegls",
            Self::Bmp => "bmp",
            Self::Png => "png",
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
        if s.eq_ignore_ascii_case("preserve") {
            Ok(Self::Preserve)
        } else if s.eq_ignore_ascii_case("webp") {
            Ok(Self::Webp)
        } else if s.eq_ignore_ascii_case("jpeg") || s.eq_ignore_ascii_case("jpg") {
            Ok(Self::Jpeg)
        } else {
            // BMP and PNG are intentionally ignored as they're for internal purposes only
            Err(())
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
        if s.eq_ignore_ascii_case("preserve") {
            Ok(Self::Preserve)
        } else if s.eq_ignore_ascii_case("video") {
            Ok(Self::Video)
        } else if s.eq_ignore_ascii_case("gif") {
            Ok(Self::Gif)
        } else {
            Err(())
        }
    }
}

impl Display for VideoTypePreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// Serde default tags are added for when new fields are added, to ensure tasks from an older
// version of the bot still decode.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Task {
    Amogus {
        amogus: i32,
    },
    ImageResize {
        /// Signed integer to allow specifying negative resolutions
        /// as a way to signify mirroring the image.
        new_dimensions: (i32, i32),
        rotation: f64,
        percentage: Option<f32>,
        format: ImageFormat,
        resize_type: ResizeType,
        /// Between 1 and 100.
        quality: NonZeroU8,
        #[serde(default)]
        spoiler: bool,
    },
    VideoResize {
        /// Signed integer to allow specifying negative resolutions
        /// as a way to signify mirroring the image.
        new_dimensions: (i32, i32),
        rotation: f64,
        percentage: Option<f32>,
        resize_type: ResizeType,
        vibrato_hz: f64,
        vibrato_depth: f64,
        #[serde(default = "ResizeCurve::default")]
        resize_curve: ResizeCurve,
        #[serde(default = "VideoTypePreference::default")]
        type_pref: VideoTypePreference,
        /// Between 1 and 100.
        quality: NonZeroU8,
        #[serde(default)]
        spoiler: bool,
    },
    /// Optical Character Recognition, i.e. extracting text from an image
    Ocr,
    /// Transcribe speech in input media to text with Whisper AI
    Transcribe {
        temperature: f32,
        translate_to_english: bool,
    },
    AmenBreak {
        shortest: bool,
        match_length: bool,
        #[serde(default)]
        spoiler: bool,
    },
    LayerAudio {
        meta: FileMeta,
        shortest: bool,
        match_length: bool,
        #[serde(default)]
        spoiler: bool,
    },
    Reencode,
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
                resize_curve: _,
                type_pref: _,
                quality,
                spoiler,
            }
            | Task::ImageResize {
                new_dimensions,
                rotation,
                percentage,
                format: _,
                resize_type,
                quality,
                spoiler,
            } => {
                if let ResizeType::ToSticker
                | ResizeType::ToCustomEmoji
                | ResizeType::ToSpoileredMedia { .. } = resize_type
                {
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
                    write!(output, ", or {percentage}%")?;
                }
                writeln!(output)?;
                writeln!(output, "<b>Rotation</b>: {rotation}°")?;
                write_param!("Resize method", resize_type)?;

                if let Task::VideoResize {
                    vibrato_hz,
                    vibrato_depth,
                    resize_curve,
                    type_pref,
                    ..
                } = self
                {
                    write_param!("Media type", type_pref)?;
                    wp!(vibrato_hz)?;
                    wp!(vibrato_depth)?;
                    write_param!("Resize curve", resize_curve)?;
                }

                writeln!(output, "<b>Quality</b>: {quality}%")?;

                write_param!("Spoiler", spoiler)?;

                Ok(())
            }
            Task::Ocr => Ok(()),
            Task::AmenBreak {
                shortest,
                match_length,
                spoiler,
            }
            | Task::LayerAudio {
                meta: _,
                shortest,
                match_length,
                spoiler,
            } => {
                let text_shortest = if *shortest { "shortest" } else { "longest" };
                writeln!(
                    output,
                    "Make result <b>{text_shortest}</b> length between video and audio"
                )?;

                if *match_length {
                    writeln!(
                        output,
                        "<b>Don't</b> keep speed and instead match video for a perfect loop"
                    )?;
                } else {
                    writeln!(
                        output,
                        "<b>Keep speed</b>, making result not a perfect loop"
                    )?;
                }

                write_param!("Spoiler", spoiler)?;

                Ok(())
            }
            Task::Transcribe {
                temperature,
                translate_to_english,
            } => {
                if *temperature == 0.0 {
                    writeln!(output, "<b>Temperature</b>: Auto")?;
                } else {
                    writeln!(output, "<b>Temperature</b>: {temperature}")?;
                }
                write_param!("Translate to English", translate_to_english)?;
                Ok(())
            }
            Task::Reencode => Ok(()),
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
                    format!("Working on your task now... {progress}\n")
                } else {
                    String::from("Working on your task now...\n")
                }
            }
            Some(s) => format!("Task accepted. Position in queue: {s}\n"),
            None => "Task accepted. Waiting for this chat's slow mode...\n".to_string(),
        };

        self.write_params(&mut response, true, queue_size != Some(0))
            .unwrap();

        response += "\n<a href=\"https://boosty.to/architector_4\">(Consider supporting? 👉👈)</a>";
        response
    }

    /// Returns a bitmask of exclusive resources this task would clobber.
    /// Those resources can only be used by a single task, and as such we don't
    /// want to try to concurrently complete two tasks clobbering the same resource.
    ///
    /// The resources are:
    /// 1 - Whisper server
    pub fn clobbers(&self) -> u32 {
        match &self {
            Self::Transcribe { .. } => 1,
            _ => 0,
        }
    }
}

////////////////////////////
////////////// DEFAULT IMPLEMENTATIONS
////////////////////////////

impl Task {
    pub fn default_to_sticker() -> Task {
        Task::ImageResize {
            new_dimensions: (512, 512),
            rotation: 0.0,
            percentage: None,
            format: ImageFormat::Webp,
            resize_type: ResizeType::ToSticker,
            quality: NonZeroU8::new(92).unwrap(),
            spoiler: false,
        }
    }
    pub fn default_to_custom_emoji() -> Task {
        Task::ImageResize {
            new_dimensions: (100, 100),
            rotation: 0.0,
            percentage: None,
            format: ImageFormat::Webp,
            resize_type: ResizeType::ToCustomEmoji,
            quality: NonZeroU8::new(92).unwrap(),
            spoiler: false,
        }
    }
    pub fn default_to_spoilered_image(width: i32, height: i32, caption: String) -> Task {
        Task::ImageResize {
            new_dimensions: (width, height),
            rotation: 0.0,
            percentage: None,
            format: ImageFormat::Jpeg,
            resize_type: ResizeType::ToSpoileredMedia { caption },
            quality: NonZeroU8::new(92).unwrap(),
            spoiler: false,
        }
    }
    pub fn default_to_spoilered_video(width: i32, height: i32, caption: String) -> Task {
        Task::default_video_resize(
            width,
            height,
            ResizeType::ToSpoileredMedia { caption },
            VideoTypePreference::Preserve,
        )
    }
    pub fn default_amogus() -> Task {
        Task::Amogus { amogus: 1 }
    }
    pub fn default_image_resize(
        width: i32,
        height: i32,
        resize_type: ResizeType,
        format: ImageFormat,
    ) -> Task {
        Task::ImageResize {
            new_dimensions: (width, height),
            rotation: 0.0,
            percentage: Some(100.0),
            format,
            resize_type,
            quality: NonZeroU8::new(92).unwrap(),
            spoiler: false,
        }
    }
    pub fn default_video_resize(
        width: i32,
        height: i32,
        resize_type: ResizeType,
        type_pref: VideoTypePreference,
    ) -> Task {
        Task::VideoResize {
            new_dimensions: (width, height),
            rotation: 0.0,
            percentage: Some(100.0),
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
            resize_type,
            resize_curve: ResizeCurve::default(),
            type_pref,
            quality: NonZeroU8::new(100).unwrap(),
            spoiler: false,
        }
    }
    pub fn default_ocr() -> Task {
        Task::Ocr
    }
    pub fn default_transcribe() -> Task {
        Task::Transcribe {
            temperature: 0.0,
            translate_to_english: false,
        }
    }
    pub fn default_amenbreak() -> Task {
        Task::AmenBreak {
            shortest: false,
            match_length: true,
            spoiler: false,
        }
    }
    pub fn default_layer_audio(meta: FileMeta) -> Task {
        Task::LayerAudio {
            meta,
            shortest: false,
            match_length: true,
            spoiler: false,
        }
    }
    pub fn default_reencode() -> Task {
        Task::Reencode
    }
}
