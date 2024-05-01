use std::{future::Future, num::NonZeroU32, pin::Pin};

use arch_bot_commons::useful_methods::*;
use html_escape::encode_text;

use teloxide::{
    types::{BotCommand, Message, UserId},
    Bot, RequestError,
};

use crate::{
    tasks::{
        param_parsing::ParamParser, taskman::Taskman, ImageFormat, ResizeType, Task, TaskError,
    },
    OWNER_ID,
};

pub const COMMANDS: &[Command] = &[
    START,
    HELP,
    AMOGUS,
    DISTORT,
    RESIZE,
    REVERSE_TEXT,
    TO_STICKER,
    ____SEPARATOR,
    PREMIUM,
    UNPREMIUM,
];

pub type Ret = Result<Result<Task, TaskError>, RequestError>;
pub type TaskFuture<'a> = Pin<Box<dyn Future<Output = Ret> + Send + 'a>>;

#[allow(dead_code)]
pub struct TaskParams<'a> {
    taskman: &'a Taskman,
    bot: &'a Bot,
    message: &'a Message,
    command: String,
}

impl<'a> TaskParams<'a> {
    pub fn new<'new>(
        taskman: &'new Taskman,
        bot: &'new Bot,
        message: &'new Message,
    ) -> Option<TaskParams<'new>> {
        let text = message.text_full()?;

        if !text.starts_with('/') {
            return None;
        }

        let command = text.split_whitespace().next()?.to_lowercase();

        Some(TaskParams {
            taskman,
            bot,
            message,
            command,
        })
    }

    pub fn make_task(self) -> Option<TaskFuture<'a>> {
        // Commands shouldn't have an "@" in their callnames.
        // If the command is "/distort@Teco_Tools_Bot",
        // trim the "@" and everything after it.
        let callname = if let Some(username_start) = self.command.find('@') {
            &self.command[0..username_start]
        } else {
            &self.command
        };
        for command in COMMANDS {
            if command.is_matching_callname(callname) {
                return Some((command.function)(self));
            }
        }
        // No matching command found. lol lmao
        None
    }

    fn get_text_params(&self) -> &str {
        // SAFETY: this type can only be constructed if the message
        // has non-empty text in it.
        let text = self.message.text_full().unwrap();
        let command_full_len = self.command.len();

        text[command_full_len..].trim_start()
    }

    fn get_params(&self) -> ParamParser {
        ParamParser::new(self.get_text_params())
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
            .is_some_and(|x| x == command)
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
        let mut response = String::from("HELP:\n\n");
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
            .reply_to_message_id($stuff.message.id)
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

#[allow(unused_macros)]
macro_rules! goodbye_cancel {
    ($err:expr) => {{
        goodbye!(TaskError::Cancel($err.to_string()));
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

For complaints or questions poke @Architector_4";

    goodbye_desc!(response);
}

pub const HELP: Command = Command {
    callname: "/help",
    description: "Show this help.",
    function: wrap!(help),
    hidden: false,
};
async fn help(tp: TaskParams<'_>) -> Ret {
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
        input.push_str("\n\n");
    }

    let request_text = tp.message.text_full().unwrap();
    // Exclude first word - the whole command invocation.
    let request_text = request_text[tp.command.len()..].trim();
    input.push_str(request_text);

    if input.is_empty() {
        // Nothing to reverse...
        // Include the command invocation then lol
        input.push_str(&tp.command);
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
    let task = unfail!(Task::default_amogus().parse_params(tp.get_params()));
    Ok(Ok(task))
}

async fn resize_inner(tp: TaskParams<'_>, resize_type: ResizeType) -> Ret {
    let media = tp.message.get_media_info();
    let media = match media {
        Some(media) => {
            if media.is_vector_sticker {
                goodbye_cancel!("can't work with animated stickers.");
            }
            if media.file.size > 20 * 1000 * 1000 {
                goodbye_cancel!("media is too large.");
            }
            media
        }
        None => goodbye_cancel!("can't find a video or an image."),
    };
    let (Some(width), Some(height)) = (NonZeroU32::new(media.width), NonZeroU32::new(media.height))
    else {
        goodbye_cancel!("media is too small.");
    };

    let task = if media.is_image() {
        unfail!(
            Task::default_image_resize(width, height, resize_type, ImageFormat::Preserve)
                .parse_params(tp.get_params())
        )
    } else {
        unfail!(Task::default_video_resize(width, height, resize_type).parse_params(tp.get_params()))
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
    let photo = tp.message.get_media_info();
    let _photo = match photo {
        Some(photo) => {
            if !photo.is_image() {
                goodbye_cancel!("can't work with video nor animated nor video stickers.");
            }
            if photo.file.size > 20 * 1000 * 1000 {
                goodbye_cancel!("media is too large.");
            }
            photo
        }
        None => goodbye_cancel!("can't find an image."),
    };

    Ok(Ok(Task::default_to_sticker()))
}
pub const RESIZE: Command = Command {
    callname: concat!(
        "/resize &lt;image&gt; ",
        "[&lt;fit/stretch/crop&gt;] ",
        "[&lt;WxH&gt; or &lt;size%&gt;] ",
        "[&lt;format&gt;] ",
        "[&lt;rot&gt;]",
    ),
    description: concat!(
        "Resizes a media by fitting (default), stretching or cropping it ",
        "to specified resolution (or by default to 50% of original), and rotating by \"rot\" degrees. ",
        "By default will reduce the image's size in half on each side unless a format is specified."
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
    ),
    description: concat!(
        "Distorts the image using seam carving and rotates it by \"rot\" degrees. ",
        "By default will reduce the media's size in half on each side."
    ),
    function: wrap!(distort),
    hidden: false,
};
fn distort(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    resize_inner(tp, ResizeType::default_seam_carve())
}

async fn premium_inner(tp: TaskParams<'_>, premium: bool) -> Ret {
    if tp.message.from().map(|x| x.id) != Some(OWNER_ID) {
        goodbye_desc!("");
    }
    let params = tp.get_text_params();

    let mut response = String::with_capacity(params.len());

    for thing in tp.get_text_params().split_whitespace() {
        use std::fmt::Write;
        let Ok(woot): Result<u64, _> = thing.parse() else {
            writeln!(response, "wtf is {}", thing).expect("no");
            continue;
        };

        if let Err(e) = tp.taskman.db.set_premium(UserId(woot), premium).await {
            writeln!(response, "OH SHIT: {:#?}", e).expect("no");
            break;
        }

        if premium {
            writeln!(response, "{} is premium now", thing).expect("no");
        } else {
            writeln!(response, "{} is not premium now", thing).expect("no");
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
}
