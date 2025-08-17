use std::{
    future::Future,
    io::Write,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};

use arch_bot_commons::{teloxide_retry, useful_methods::*};
use html_escape::encode_text;

use teloxide::{
    payloads::{SendAnimationSetters, SendPhotoSetters, SendVideoSetters},
    requests::Requester,
    sugar::request::RequestReplyExt,
    types::{BotCommand, InputFile, Me, Message, UserId},
    ApiError, Bot, RequestError,
};
use tempfile::NamedTempFile;

use crate::{
    tasks::{
        completion::media_processing::count_video_frames_and_framerate_and_audio_and_length,
        parsing::TaskError, taskman::Taskman, ImageFormat, ResizeType, Task, VideoTypePreference,
    },
    MAX_DOWNLOAD_SIZE_MEGABYTES, OWNER_ID,
};

pub const COMMANDS: &[Command] = &[
    START,
    HELP,
    AMENBREAK,
    AMOGUS,
    DISTORT,
    OCR,
    PICKAUDIO,
    LAYERAUDIO,
    REENCODE,
    RESIZE,
    REVERSE_TEXT,
    ROT_TEXT,
    SPOILER,
    TO_CUSTOM_EMOJI,
    TO_GIF,
    TO_STICKER,
    TO_VIDEO,
    TRANSCRIBE,
    ____SEPARATOR,
    PREMIUM,
    UNPREMIUM,
];

pub type Ret = Result<Result<Task, TaskError>, RequestError>;
pub type TaskFuture<'a> = Pin<Box<dyn Future<Output = Ret> + Send + 'a>>;

#[allow(dead_code)]
pub struct TaskParams<'a> {
    pub taskman: &'a Taskman,
    pub bot: &'a Bot,
    pub bot_me: &'a Me,
    pub message: &'a Message,
    pub message_text: &'a str,
    pub command_len: usize,
}

impl<'a> TaskParams<'a> {
    pub fn new<'new>(
        taskman: &'new Taskman,
        bot: &'new Bot,
        bot_me: &'new Me,
        message: &'new Message,
    ) -> Option<TaskParams<'new>> {
        let message_text = message.text_full()?;

        if !message_text.starts_with('/') {
            return None;
        }

        let command = message_text.split_whitespace().next()?;

        if !command.is_ascii() {
            // Telegram commands must be ASCII.
            // See https://core.telegram.org/bots/api#botcommand
            return None;
        }

        let command_len = command.len();

        Some(TaskParams {
            taskman,
            bot,
            bot_me,
            message,
            message_text,
            command_len,
        })
    }

    pub fn make_task(self) -> Option<TaskFuture<'a>> {
        // Commands shouldn't have an "@" in their callnames.
        // If the command is "/distort@Teco_Tools_Bot",
        // trim the "@" and everything after it.
        let callname = if let Some(username_start) = self.command().find('@') {
            // While we're here, also check if the username is actually ours.
            // Bot names are guaranteed ASCII, so ignore ASCII case specifically.
            if !self.command()[username_start + '@'.len_utf8()..]
                .eq_ignore_ascii_case(self.bot_me.username())
            {
                // This command is not for us. Ignore.
                return None;
            }

            &self.command()[0..username_start]
        } else {
            self.command()
        };
        for command in COMMANDS {
            if command.is_matching_callname(callname) {
                return Some((command.function)(self));
            }
        }
        // No matching command found. lol lmao
        None
    }

    /// Get text command for this task.
    ///
    /// If the input command is `/Hewwo everypony bazinga`,
    /// this will be the substring `/Hewwo`.
    #[inline]
    pub fn command(&self) -> &str {
        &self.message_text[..self.command_len]
    }

    /// Get text parameters for this task.
    ///
    /// If the input command is `/Hewwo everypony bazinga`,
    /// this will be the substring `everypony bazinga`.
    #[inline]
    pub fn get_params(&self) -> &str {
        self.message_text[self.command_len..].trim_start()
    }
}

pub struct Command {
    pub callname: &'static str,
    pub description: &'static str,
    pub function: fn(TaskParams) -> TaskFuture,
    //pub function: fn(TaskParams) -> Ret,
    hidden: bool,
}

impl Command {
    pub fn is_matching_callname(&self, command: &str) -> bool {
        self.callname
            .split_ascii_whitespace()
            .next()
            .is_some_and(|x| x.eq_ignore_ascii_case(command))
    }

    pub fn get_help(&self, mut output: impl std::fmt::Write) -> Result<(), std::fmt::Error> {
        // useful for the separator "command"
        if self.callname.is_empty() && self.description.is_empty() {
            return Ok(());
        }

        output.write_str(self.callname)?;
        if !self.description.is_empty() {
            output.write_str(" - ")?;
            output.write_str(self.description)?;
        }

        Ok(())
    }

    pub fn generate_help() -> String {
        // there's probably a more elegant way to do this but i'm not braining rn lol
        let mut response = String::from(concat!("HELP:\n\n",
        "Send <code>/command help</code> for detailed help with all parameters on <code>/command</code>.\n\n"));
        for command in COMMANDS {
            if command.hidden {
                continue;
            }
            command.get_help(&mut response).unwrap();
            response += "\n\n";
        }
        response.pop();
        response.pop();
        response
    }

    pub fn generate_bot_commands() -> Vec<BotCommand> {
        let mut output = Vec::new();

        for command in COMMANDS {
            if command.hidden {
                continue;
            }
            let Some(callname) = command.callname.split_ascii_whitespace().next() else {
                continue;
            };

            // Cut off the /
            let callname = callname[1..].trim().to_string();
            let description = command
                .description
                .replace("&lt;", "<")
                .replace("&gt;", ">");

            output.push(BotCommand {
                command: callname,
                description,
            });
        }

        output
    }
}

/// Returns true if this string is "help" or a variation of.
pub fn request_for_help(text: &str) -> bool {
    matches!(text.trim(), "help" | "-help" | "--help" | "-h" | "--h")
}

///////////////////////////////////////
/////////////////COMMAND DEFINITIONS
///////////////////////////////////////

/// Wraps the function's return value in a pinning closure.
macro_rules! wrap {
    ($thing:expr) => {
        |tp| Box::pin($thing(tp))
    };
}

#[allow(unused_macros)]
macro_rules! respond {
    ($stuff:expr, $text:expr) => {
        $stuff
            .bot
            .send_message($stuff.message.chat.id, $text)
            .reply_to($stuff.message.id)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await?
    };
}

macro_rules! goodbye {
    ($err:expr) => {
        return Ok(Err($err))
    };
    () => {
        return Ok(Err(TaskError::Error(String::new())));
    };
}

macro_rules! goodbye_err {
    ($err:expr) => {
        goodbye!(TaskError::Error($err.to_string()))
    };
}

macro_rules! goodbye_desc {
    ($err:expr) => {
        goodbye!(TaskError::Descriptory($err.to_string()))
    };
}

macro_rules! goodbye_cancel {
    ($err:expr) => {{
        goodbye!(TaskError::Cancel($err.to_string()));
    }};
}

macro_rules! check_too_large {
    ($media:expr) => {{
        if $media.size > MAX_DOWNLOAD_SIZE_MEGABYTES * 1000 * 1000 {
            goodbye_cancel!(format!(
                "media is too large. The limit is {}MB.",
                MAX_DOWNLOAD_SIZE_MEGABYTES
            )
            .as_str());
        }
    }};
}

macro_rules! unfail {
    ($item:expr) => {
        match $item {
            Ok(x) => x,
            Err(e) => {
                goodbye!(e);
            }
        }
    };
}

#[allow(unused_macros)]
macro_rules! error {
    ($text:literal) => {
        goodbye!(concat!("Error: ", $text));
    };
}

/// Check if the input parameters is someone asking for help, and if so,
/// print it for this type of task.
macro_rules! print_help {
    ($stuff: expr, $task: expr) => {
        if request_for_help($stuff.get_params()) {
            let mut help = $task.param_help();
            // Some tasks have empty help.
            if help.is_empty() {
                help = "This command has no parameters.";
            }
            goodbye_desc!(help);
        }
    };
}

pub const START: Command = Command {
    callname: "/start",
    description: "",
    function: wrap!(start),
    hidden: true,
};
async fn start(tp: TaskParams<'_>) -> Ret {
    if !tp.message.chat.is_private() {
        goodbye!();
    }
    let response = "hewo

This is a multitool tool telegram bot designed to do things because i wanted to make one lol

For a full list of commands, send /help

For complaints or questions poke @Architector_4

<a href=\"https://boosty.to/architector_4\">(Consider supporting? 👉👈)</a>
";

    goodbye_desc!(response);
}

pub const HELP: Command = Command {
    callname: "/help",
    description: "Show this help.",
    function: wrap!(help),
    hidden: false,
};
async fn help(tp: TaskParams<'_>) -> Ret {
    use std::fmt::Write;

    let mut params = tp.get_params().split_whitespace();
    if let Some(cmdname) = params.next() {
        // If a second parameter is present, just print normal help with code below.
        if params.next().is_none() {
            let cmdname = cmdname.trim_start_matches('/');
            // Find the command...
            let Some(cmd) = COMMANDS.iter().find(|x| {
                x.callname
                    .split_ascii_whitespace()
                    .next()
                    .is_some_and(|x| x.trim_start_matches('/').eq_ignore_ascii_case(cmdname))
            }) else {
                goodbye_desc!(&format!(
                    "I don't know of a command named <code>/{}</code>.",
                    encode_text(cmdname)
                ));
            };

            let mut output = String::new();
            cmd.get_help(&mut output).unwrap();

            write!(
                &mut output,
                "\n\nSend <code>/{cmdname} help</code> for more info on parameters, if any."
            )
            .unwrap();

            goodbye_desc!(output);
        }
    }

    if !tp.message.chat.is_private() {
        goodbye_desc!("Contact me in DMs for help!");
    }
    let help = Command::generate_help();
    goodbye_desc!(help);
}

pub const ____SEPARATOR: Command = Command {
    callname: "",
    description: "",
    function: wrap!(do_nothing),
    hidden: false,
};
async fn do_nothing(_: TaskParams<'_>) -> Ret {
    goodbye_err!("")
}

pub const REVERSE_TEXT: Command = Command {
    callname: "/reverse_text &lt;text&gt;",
    description: "Reverses text.",
    function: wrap!(reverse_text),
    hidden: false,
};
#[allow(clippy::no_effect_underscore_binding)]
async fn reverse_text(tp: TaskParams<'_>) -> Ret {
    let mut input = String::new();
    // Check for replied-to message
    if let Some(repliee_text) = tp.message.reply_to_message().and_then(|x| x.text_full()) {
        input.reserve_exact(repliee_text.len() + 2 + tp.message.text_full().unwrap().len());
        input.push_str(repliee_text);
    }

    let request_text = tp.message.text_full().unwrap();
    // Exclude first word - the whole command invocation.
    let request_text = request_text[tp.command_len..].trim();
    if !request_text.is_empty() {
        input.push_str("\n\n");
        input.push_str(request_text);
    }

    if input.is_empty() {
        // Nothing to reverse...
        // Include the command invocation then lol
        input.push_str(tp.command());
    }

    use unicode_segmentation::UnicodeSegmentation;

    let response = input.graphemes(true).rev().collect::<String>();
    let response = encode_text(&response);

    goodbye_desc!(response);
}

pub const AMOGUS: Command = Command {
    callname: "/amogus &lt;amogus&gt;",
    description: "amogus",
    function: wrap!(amogus),
    hidden: false,
};
async fn amogus(tp: TaskParams<'_>) -> Ret {
    let task = Task::default_amogus();
    print_help!(tp, task);
    let task = unfail!(task.parse_params(&tp));

    Ok(Ok(task))
}

async fn resize_inner(tp: TaskParams<'_>, resize_type: ResizeType) -> Ret {
    // Image and video resize should have the same help.
    let temp_task = Task::default_image_resize(1, 1, resize_type.clone(), ImageFormat::Preserve);
    print_help!(tp, temp_task);

    let media = tp.message.get_media_info();
    let media = match media {
        Some(media) => {
            if !media.is_raster() {
                goodbye_cancel!("can't work with animated stickers nor voice messages.");
            }
            check_too_large!(media.file);
            media
        }
        None => goodbye_cancel!(concat!(
            "can't find a video or an image. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    if media.width < 1 || media.height < 1 {
        goodbye_cancel!("media is too small.");
    }

    let task = if media.is_image() {
        unfail!(Task::default_image_resize(
            media.width as i32,
            media.height as i32,
            resize_type,
            ImageFormat::Preserve
        )
        .parse_params(&tp))
    } else {
        unfail!(Task::default_video_resize(
            media.width as i32,
            media.height as i32,
            resize_type,
            VideoTypePreference::Preserve
        )
        .parse_params(&tp))
    };

    Ok(Ok(task))
}

pub const TO_STICKER: Command = Command {
    callname: "/to_sticker &lt;image&gt;",
    description: "Converts the image into a 512x512 WEBP suitable for usage as a sticker.",
    function: wrap!(to_sticker),
    hidden: false,
};
async fn to_sticker(tp: TaskParams<'_>) -> Ret {
    let task = Task::default_to_sticker();
    print_help!(tp, task);
    let photo = tp.message.get_media_info();
    let _photo = match photo {
        Some(photo) => {
            if !photo.is_image() {
                goodbye_cancel!("can't work with video nor animated nor video stickers.");
            }
            check_too_large!(photo.file);
            photo
        }
        None => goodbye_cancel!(concat!(
            "can't find an image. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    Ok(Ok(task))
}

pub const TO_CUSTOM_EMOJI: Command = Command {
    callname: "/to_custom_emoji &lt;image&gt;",
    description: "Converts the image into a 100x100 WEBP suitable for usage as a custom emoji.",
    function: wrap!(to_custom_emoji),
    hidden: false,
};
async fn to_custom_emoji(tp: TaskParams<'_>) -> Ret {
    let task = Task::default_to_custom_emoji();
    print_help!(tp, task);
    let photo = tp.message.get_media_info();
    let _photo = match photo {
        Some(photo) => {
            if !photo.is_image() {
                goodbye_cancel!("can't work with video nor animated nor video stickers.");
            }
            check_too_large!(photo.file);
            photo
        }
        None => goodbye_cancel!(concat!(
            "can't find an image. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    Ok(Ok(task))
}

pub const RESIZE: Command = Command {
    callname: concat!(
        "/resize &lt;image&gt; ",
        "[&lt;fit/stretch/crop&gt;] ",
        "[&lt;WxH&gt; or &lt;size%&gt;] ",
        "[&lt;format&gt;] ",
        "[&lt;rot&gt;] ",
        "[...]",
    ),
    description: concat!(
        "Resizes a media by fitting (default), stretching or cropping it ",
        "to specified resolution, and rotating by \"rot\" degrees. ",
        "By default will reduce the image/video's size in half on each side unless any options are specified."
    ),
    function: wrap!(resize),
    hidden: false,
};
fn resize(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    resize_inner(tp, ResizeType::Fit)
}

pub const DISTORT: Command = Command {
    callname: concat!(
        "/distort &lt;image&gt; ",
        "[&lt;WxH&gt; or &lt;size%&gt;] ",
        "[&lt;delta_x&gt;] ",
        "[&lt;rigidity&gt;] ",
        "[...]",
    ),
    description: concat!(
        "Distorts the media using seam carving and rotates it by \"rot\" degrees. ",
        "By default will reduce the image/video's size in half on each side."
    ),
    function: wrap!(distort),
    hidden: false,
};
fn distort(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    resize_inner(tp, ResizeType::default_seam_carve())
}

pub const OCR: Command = Command {
    callname: "/ocr",
    description: concat!(
        "Try to extract text from an image using Optical Character Recognition. ",
        "This uses the Tesseract OCR engine."
    ),
    function: wrap!(ocr),
    hidden: false,
};
async fn ocr(tp: TaskParams<'_>) -> Ret {
    let task = Task::default_ocr();
    print_help!(tp, task);
    let photo = tp.message.get_media_info();
    let _photo = match photo {
        Some(photo) => {
            if !photo.is_image() {
                goodbye_cancel!("can't work with video nor animated nor video stickers.");
            }
            check_too_large!(photo.file);
            photo
        }
        None => goodbye_cancel!(concat!(
            "can't find an image. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    Ok(Ok(task))
}

async fn to_video_or_gif_inner(tp: TaskParams<'_>, to_gif: bool) -> Ret {
    let temp_task = Task::default_video_resize(
        1,
        1,
        ResizeType::ToSticker,
        if to_gif {
            VideoTypePreference::Gif
        } else {
            VideoTypePreference::Video
        },
    );
    print_help!(tp, temp_task);

    let _ = tp.bot.typing(tp.message.chat.id).await;

    let video = tp.message.get_media_info();
    let video = match video {
        Some(video) => {
            if !video.is_raster() {
                goodbye_cancel!("can't work with animated stickers.");
            }
            if video.is_image() {
                goodbye_cancel!("can't work with non-video images.");
            }
            check_too_large!(video.file);
            video
        }
        None => goodbye_cancel!(concat!(
            "can't find a video. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    // First try to send it over directly.
    // Video stickers are excluded from this because they are VP9 WEBM, while
    // video files should preferably be H.264 MP4.
    if !video.is_sticker {
        let mut buf = Vec::new();
        teloxide_retry!(tp.bot.download_file_to_vec(video.file, &mut buf).await)?;
        let should_send_directly = if to_gif {
            // If we need to send it as a gif, we need to ensure the input has
            // no sound. If it does, then Telegram will make it a video instead.

            // Define the check as a closure.
            // This makes error handling here easier with the "?" operator.
            let has_audio_closure = || {
                let mut tempfile = NamedTempFile::new()?;
                tempfile.write_all(&buf)?;
                tempfile.flush()?;
                let has_audio =
                    count_video_frames_and_framerate_and_audio_and_length(tempfile.path(), false)?
                        .2;
                Ok::<_, std::io::Error>(has_audio)
            };

            // If failed, assume it has audio, just in case.
            let has_audio = has_audio_closure().unwrap_or(true);

            !has_audio
        } else {
            // We're sending as video, so audio doesn't matter.
            true
        };

        if should_send_directly {
            let file = InputFile::memory(buf);
            let send_direct_result = if to_gif {
                let file = file.file_name("amogus.mp4");
                // Sending as an "animation" requires that the file has a filename, else
                // it somehow ends up being a file document instead.
                teloxide_retry!(
                    tp.bot
                        .send_animation(tp.message.chat.id, file.clone(),)
                        .reply_to(tp.message.id)
                        .await
                )
            } else {
                teloxide_retry!(
                    tp.bot
                        .send_video(tp.message.chat.id, file.clone())
                        .reply_to(tp.message.id)
                        .await
                )
            };

            match send_direct_result {
                Ok(_) => return Ok(Err(TaskError::Descriptory(String::new()))),
                Err(e) => {
                    let _ = tp
                        .bot
                        .archsendmsg(
                            OWNER_ID,
                            format!("Failed directly uploading a video: {e:#?}").as_str(),
                            None,
                        )
                        .await;
                }
            }
        }
    }

    // Failed to send it directly. Let's do it the funny way around then.
    if video.width < 1 || video.height < 1 {
        goodbye_cancel!("video is too small.");
    }

    Ok(Ok(Task::default_video_resize(
        video.width as i32,
        video.height as i32,
        ResizeType::ToSticker,
        if to_gif {
            VideoTypePreference::Gif
        } else {
            VideoTypePreference::Video
        },
    )))
}
pub const TO_VIDEO: Command = Command {
    callname: "/to_video",
    description: "Turn a GIF or a video sticker into a video.",
    function: wrap!(to_video),
    hidden: false,
};
fn to_video(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    to_video_or_gif_inner(tp, false)
}

pub const TO_GIF: Command = Command {
    callname: "/to_gif",
    description: "Turn a video into a GIF.",
    function: wrap!(to_gif),
    hidden: false,
};
fn to_gif(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    to_video_or_gif_inner(tp, true)
}

async fn premium_inner(tp: TaskParams<'_>, premium: bool) -> Ret {
    if tp.message.from.as_ref().map(|x| x.id) != Some(OWNER_ID) {
        goodbye_desc!("");
    }
    let params = tp.get_params();

    let mut response = String::with_capacity(params.len());

    for thing in tp.get_params().split_whitespace() {
        use std::fmt::Write;
        let Ok(woot): Result<u64, _> = thing.parse() else {
            writeln!(response, "wtf is {thing}").expect("no");
            continue;
        };

        if let Err(e) = tp.taskman.db.set_premium(UserId(woot), premium).await {
            writeln!(response, "OH SHIT: {e:#?}").expect("no");
            break;
        }

        if premium {
            writeln!(response, "{thing} is premium now").expect("no");
        } else {
            writeln!(response, "{thing} is not premium now").expect("no");
        }
    }

    goodbye_desc!(response);
}
pub const PREMIUM: Command = Command {
    callname: "/premium &lt;userid(s)&gt;",
    description: "premium",
    function: wrap!(premium),
    hidden: true,
};
fn premium(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    premium_inner(tp, true)
}
pub const UNPREMIUM: Command = Command {
    callname: "/unpremium &lt;userid(s)&gt;",
    description: "unpremium",
    function: wrap!(unpremium),
    hidden: true,
};
fn unpremium(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    premium_inner(tp, false)
}

pub const AMENBREAK: Command = Command {
    callname: "/amenbreak",
    description: "Replace a video/gif's audio with an amen break.",
    function: wrap!(amenbreak),
    hidden: false,
};
async fn amenbreak(tp: TaskParams<'_>) -> Ret {
    let temp_task = Task::default_amenbreak();
    print_help!(tp, temp_task);
    let media = tp.message.get_media_info();
    let _media = match media {
        Some(media) => {
            if media.is_vector_sticker {
                goodbye_cancel!("can't work with animated stickers.");
            }
            if media.is_sound {
                goodbye_cancel!("can't work with audio messages.");
            }
            check_too_large!(media.file);
            media
        }
        None => goodbye_cancel!(concat!(
            "can't find a video or a photo. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    Ok(Ok(temp_task))
}

pub const TRANSCRIBE: Command = Command {
    callname: "/transcribe",
    description: "Transcribe speech in input media to text with Whisper AI.",
    function: wrap!(transcribe),
    hidden: false,
};
async fn transcribe(tp: TaskParams<'_>) -> Ret {
    let temp_task = Task::default_transcribe();
    print_help!(tp, temp_task);
    let media = tp.message.get_media_info();
    let _media = match media {
        Some(media) => {
            if !(media.is_video || media.is_sound || media.is_voice_or_video_note) {
                goodbye_cancel!("input media doesn't have sound.");
            }
            check_too_large!(media.file);
            media
        }
        None => goodbye_cancel!(concat!(
            "can't find a media with audio. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    let task = unfail!(temp_task.parse_params(&tp));

    Ok(Ok(task))
}
pub const ROT_TEXT: Command = Command {
    callname: "/rot &lt;count&gt; &lt;text&gt;",
    description: concat!(
        "Shifts Unicode codepoint values in &lt;text&gt; by &lt;count&gt; times. ",
        "If &lt;count&gt; is not specified, it's defaulted to 13."
    ),
    function: wrap!(rot_text),
    hidden: false,
};
async fn rot_text(tp: TaskParams<'_>) -> Ret {
    let request_text = tp.message.text_full().unwrap();
    // Exclude first word - the whole command invocation.
    let mut request_text = request_text[tp.command_len..].trim();

    let count = if let Some(count_txt) = request_text.split_whitespace().next() {
        if let Ok(count) = count_txt.parse::<i32>() {
            request_text = request_text[count_txt.len()..].trim_start();
            count
        } else {
            if count_txt.chars().all(|x| x.is_ascii_digit()) {
                // User likely intended to specify a count but it failed lol
                goodbye_err!(concat!(
                    "Failed to parse count. ",
                    "It needs to be more than -2147483649 but less than 2147483648."
                ));
            }
            13
        }
    } else {
        13
    };

    let mut input = String::new();

    // Check for replied-to message
    if let Some(repliee_text) = tp.message.reply_to_message().and_then(|x| x.text_full()) {
        input.reserve_exact(repliee_text.len() + 2 + tp.message.text_full().unwrap().len());
        input.push_str(repliee_text);
    }

    if !request_text.is_empty() {
        input.push_str("\n\n");
        input.push_str(request_text);
    }

    if input.is_empty() {
        // Nothing to rotate...
        // Include the command invocation then lol
        input.push_str(tp.command());
    }

    let response = input
        .chars()
        .map(|x| {
            let rotated = u32::from(x).wrapping_add_signed(count);
            char::from_u32(rotated).unwrap_or('�')
        })
        .collect::<String>();

    let response = response.trim();

    if response.is_empty() {
        goodbye_err!("Sorry, resulting text is empty.");
    }

    // Avoid typical message sending code,
    // because it sets parse mode as HTML, which breaks all lol
    match tp
        .bot
        .send_message(tp.message.chat.id, response)
        .reply_to(tp.message.id)
        .await
    {
        Ok(_) => (),
        Err(RequestError::Api(ApiError::MessageTextIsEmpty)) => {
            goodbye_err!("Sorry, resulting text is empty.")
        }
        Err(e) => Err(e)?,
    };
    goodbye!();
}

pub const SPOILER: Command = Command {
    callname: "/spoiler &lt;caption&gt;",
    description: "Hide a media within a spoiler with a specified caption, if any.",
    function: wrap!(spoiler),
    hidden: false,
};
async fn spoiler(tp: TaskParams<'_>) -> Ret {
    let found_nothing = AtomicBool::new(false);
    // Try to find and spoiler a media in this message.
    let spoiler_for_message = async |message: &Message| -> Ret {
        if let Some(photo) = message.find_biggest_photo() {
            tp.bot
                .send_photo(
                    tp.message.chat.id,
                    InputFile::file_id(photo.file.id.clone()),
                )
                .reply_to(tp.message.id)
                .has_spoiler(true)
                .caption(tp.get_params())
                .await?;
            goodbye!();
        }

        if let Some(sticker) = message.sticker() {
            if sticker.is_animated() {
                goodbye_err!("Cannot work with animated stickers.");
            }
            if sticker.is_video() {
                return Ok(Ok(Task::default_to_spoilered_video(
                    sticker.width.into(),
                    sticker.height.into(),
                    tp.get_params().to_string(),
                )));
            } else {
                return Ok(Ok(Task::default_to_spoilered_image(
                    sticker.width.into(),
                    sticker.height.into(),
                    tp.get_params().to_string(),
                )));
            }
        }

        if message.voice().is_some() || message.audio().is_some() {
            goodbye_err!("Audio messages are unsupported.");
        }

        if message.document().is_some() {
            goodbye_err!("Files are unsupported.");
        }

        if let Some(video) = message
            .video()
            .map(|x| &x.file)
            .or_else(|| message.video_note().map(|x| &x.file))
        {
            tp.bot
                .send_video(tp.message.chat.id, InputFile::file_id(video.id.clone()))
                .reply_to(tp.message.id)
                .has_spoiler(true)
                .caption(tp.get_params())
                .await?;
            goodbye!();
        }

        if let Some(gif) = message.animation() {
            tp.bot
                .send_animation(tp.message.chat.id, InputFile::file_id(gif.file.id.clone()))
                .reply_to(tp.message.id)
                .has_spoiler(true)
                .caption(tp.get_params())
                .await?;
            goodbye!();
        }

        found_nothing.store(true, Ordering::Relaxed);
        goodbye_err!(concat!(
            "can't find a video or an image. ",
            "This command needs to be used as either a reply or caption to one."
        ));
    };

    let result = spoiler_for_message(tp.message).await;

    if found_nothing.load(Ordering::Relaxed) {
        if let Some(reply_to) = tp.message.reply_to_message() {
            return spoiler_for_message(reply_to).await;
        }
    }

    result
}

pub const REENCODE: Command = Command {
    callname: "/reencode",
    description: "Reencode an image/gif/video/audio to a format Telegram can show conveniently.",
    function: wrap!(reencode),
    hidden: false,
};
async fn reencode(tp: TaskParams<'_>) -> Ret {
    let temp_task = Task::default_reencode();
    print_help!(tp, temp_task);
    let media = tp.message.get_media_info();
    let file = match media {
        Some(media) => {
            if media.is_vector_sticker {
                goodbye_cancel!("can't work with animated stickers.");
            }
            media.file
        }
        None => {
            let Some(document) = tp
                .message
                .document()
                .or_else(|| tp.message.reply_to_message().and_then(|x| x.document()))
            else {
                goodbye_cancel!(concat!(
                    "can't find a media. ",
                    "This command needs to be used as either a reply or caption to one."
                ));
            };
            &document.file
        }
    };

    check_too_large!(file);

    Ok(Ok(temp_task))
}

pub const PICKAUDIO: Command = Command {
    callname: "/pickaudio",
    description: "Pick an audio file for use with /layeraudio",
    function: wrap!(pickaudio),
    hidden: false,
};
async fn pickaudio(tp: TaskParams<'_>) -> Ret {
    let id = if let Some(chat) = &tp.message.sender_chat {
        chat.id.0
    } else if let Some(user) = &tp.message.from {
        user.id.0 as i64
    } else {
        goodbye_err!("Cannot determine the sender of this message");
    };

    let media = tp.message.get_media_info();
    let file = match media {
        Some(media) => {
            if !media.is_sound {
                goodbye_cancel!("only audio files can be tagged.");
            }
            check_too_large!(media.file);
            media.file
        }
        None => {
            let Some(document) = tp
                .message
                .document()
                .or_else(|| tp.message.reply_to_message().and_then(|x| x.document()))
            else {
                goodbye_cancel!("only audio files can be tagged.");
            };
            check_too_large!(document.file);
            &document.file
        }
    };

    tp.taskman
        .db
        .pick_audio(id, file)
        .await
        .expect("Database died!");

    goodbye_desc!("Marked audio. Now use /layeraudio on a media to apply it.");
}

pub const LAYERAUDIO: Command = Command {
    callname: "/layeraudio",
    description: "Layer audio (previously selected with /pickaudio) over an image or a video",
    function: wrap!(layeraudio),
    hidden: false,
};
async fn layeraudio(tp: TaskParams<'_>) -> Ret {
    let id = if let Some(chat) = &tp.message.sender_chat {
        chat.id.0
    } else if let Some(user) = &tp.message.from {
        user.id.0 as i64
    } else {
        goodbye_err!("Cannot determine the sender of this message");
    };

    let Some(filemeta) = tp
        .taskman
        .db
        .get_picked_audio(id)
        .await
        .expect("Database died!")
    else {
        goodbye_err!("You have no audio picked. Pick some audio with /pickaudio first.");
    };

    let temp_task = Task::default_layer_audio(filemeta);
    print_help!(tp, temp_task);
    let media = tp.message.get_media_info();
    let _media = match media {
        Some(media) => {
            if media.is_vector_sticker {
                goodbye_cancel!("can't work with animated stickers.");
            }
            if media.is_sound {
                goodbye_cancel!("can't work with audio messages.");
            }
            check_too_large!(media.file);
            media
        }
        None => goodbye_cancel!(concat!(
            "can't find a video or a photo. ",
            "This command needs to be used as either a reply or caption to one."
        )),
    };

    Ok(Ok(temp_task))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Validate that bot commands match requirements by Telegram's Bot API
    fn validate_bot_commands() {
        let commands = Command::generate_bot_commands();
        // "At most 100 commands can be specified"
        // - https://core.telegram.org/bots/api#setmycommands
        assert!(commands.len() <= 100);
        #[allow(clippy::len_zero)] // It's clearer here this way in context lol
        for command in commands {
            // Everything here is from https://core.telegram.org/bots/api#botcommand
            // "Text of the command; 1-32 characters."

            // Just in case, this code assumes length is measured in UTF-8 bytes.
            assert!(command.command.len() >= 1);
            assert!(command.command.len() <= 32);

            // "Can contain only lowercase English letters, digits and underscores."
            // Assuming "English letters" is Latin letters...
            for chr in command.command.chars() {
                // "ASCII Alphabetic" means all Latin letters, so filter by lowercase too.
                let is_lowercase_latin = chr.is_ascii_alphabetic() && chr.is_ascii_lowercase();
                // Assuming only ASCII digits are allowed...
                let is_digit = "0123456789".contains(chr);
                let is_underscore = chr == '_';

                assert!(is_lowercase_latin || is_digit || is_underscore);
            }

            // "Description of the command; 1-256 characters."
            assert!(command.description.len() >= 1);
            assert!(command.description.len() <= 256);
        }
    }

    #[test]
    fn validate_command_html_parsing() {
        for command in COMMANDS {
            let tag_chars = &['<', '>'];
            assert!(
                !(command.callname.contains(tag_chars) || command.description.contains(tag_chars)),
                "Command {} contains invalid characters",
                command.callname
            );
        }
    }
}
