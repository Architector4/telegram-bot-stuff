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
        bot_username: &'new str,
    ) -> Option<TaskParams<'new>> {
        // Let's say text here is "/someTASK@Teco_Tools_Bot amogus: 6
        let text = message.text_full()?;

        if !text.starts_with('/') {
            return None;
        }

        // Then command = "/someTASK@Teco_Tools_Bot"
        let command = text.split_whitespace().next()?;

        //let command_full_len = command.len();

        // command = "/sometask"
        let command = command.trim_end_matches(bot_username).to_lowercase();

        //let params = &text[command_full_len..].trim_start();

        Some(TaskParams {
            taskman,
            bot,
            message,
            command,
        })
    }

    pub fn make_task(self) -> Option<TaskFuture<'a>> {
        for command in COMMANDS {
            if command.is_matching_callname(&self.command) {
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
        goodbye_err!("Contact me in DMs for help!");
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

    // TODO: use Unicode graphemes for reversal instead lol
    let request_text = tp.message.text_full().unwrap();
    let first_word = request_text.split_ascii_whitespace().next().unwrap();
    let request_text = request_text[first_word.len()..].trim();
    input.push_str(request_text);

    if input.is_empty() {
        input.push_str(first_word);
    }

    let response = input.chars().rev().collect::<String>();
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

async fn resize_inner(tp: TaskParams<'_>, resize_type: ResizeType, format: ImageFormat) -> Ret {
    let photo = tp
        .message
        .get_photo_or_raster_sticker_here_or_reply_file_meta();
    let photo = match photo {
        Ok(Some(photo)) => photo,
        Ok(None) => goodbye_cancel!("can't find an image."),
        Err(()) => goodbye_cancel!("can't work with animated nor video stickers."),
    };
    let (Some(width), Some(height)) = (NonZeroU32::new(photo.0), NonZeroU32::new(photo.1)) else {
        goodbye_cancel!("media is too small.");
    };

    let task = unfail!(
        Task::default_resize(width, height, resize_type, format).parse_params(tp.get_params())
    );

    Ok(Ok(task))
}

pub const TO_STICKER: Command = Command {
    callname: "/to_sticker &lt;image&gt;",
    description: "Converts the image into a 512x512 WEBP suitable for usage as a sticker.",
    function: wrap!(to_sticker),
    hidden: false,
};
async fn to_sticker(tp: TaskParams<'_>) -> Ret {
    let photo = tp
        .message
        .get_photo_or_raster_sticker_here_or_reply_file_meta();
    let _photo = match photo {
        Ok(Some(photo)) => photo,
        Ok(None) => goodbye_cancel!("can't find an image"),
        Err(()) => goodbye_cancel!("can't work with animated nor video stickers"),
    };

    Ok(Ok(Task::default_to_sticker()))
}
pub const RESIZE: Command = Command {
    callname: concat!(
        "/resize &lt;image&gt; ",
        "[&lt;fit/stretch&gt;] ",
        "[&lt;WxH&gt; or &lt;size%&gt;] ",
        "[&lt;format&gt;]",
    ),
    description: concat!(
        "Resizes the image by fitting it under a specific resolution, ",
        "or stretching into it if \"stretch\" is specified. ",
        "By default will reduce the image's size in half on each side unless a format is specified."
    ),
    function: wrap!(resize),
    hidden: false,
};
fn resize(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    resize_inner(tp, ResizeType::Fit, ImageFormat::Preserve)
}

pub const DISTORT: Command = Command {
    callname: concat!(
        "/distort &lt;image&gt; ",
        "[&lt;WxH&gt; or &lt;size%&gt;] ",
        "[&lt;delta_x&gt;] ",
        "[&lt;rigidity&gt;] ",
    ),
    description: concat!(
        "Distorts the image using seam carving. ",
        "By default will reduce the image's size in half on each side."
    ),
    function: wrap!(distort),
    hidden: false,
};
fn distort(tp: TaskParams<'_>) -> impl Future<Output = Ret> + '_ {
    resize_inner(tp, ResizeType::default_seam_carve(), ImageFormat::Preserve)
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
