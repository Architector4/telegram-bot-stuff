mod database;
mod entry;
mod handlers;
mod spam_checker;
mod types;

pub use entry::*;

use teloxide::types::{ChatId, Message};
use url::Url;

/// An ID of a private chat with the developers of the bot,
/// as well as volunteers who partake in manual review of links for spam.
pub static CONTROL_CHAT_ID: ChatId = ChatId(-1002065680710);

/// An ID of a private channel used for logging manual reviews of URLs.
/// This is primarily to spot abuse and to note which URLs the bot
/// could have caught automatically but did not.
pub static REVIEW_LOG_CHANNEL_ID: ChatId = ChatId(-1002128704357);

/// Try to parse a string as a [`Url`] in a way that telegram parses it,
/// with allowing an implicit `http://` prefix.
///
/// # Errors
/// Errors if it fails to parse either way.
pub fn parse_url_like_telegram(string: &str) -> Result<Url, url::ParseError> {
    match Url::parse(string) {
        Ok(url) => Ok(url),
        Err(e) => {
            // We want to return this original error if the next step fails.
            if let Ok(url) = Url::parse(&format!("http://{string}")) {
                Ok(url)
            } else {
                Err(e)
            }
        }
    }
}

pub fn sender_name_prettyprint(message: &Message, with_id: bool) -> String {
    let mut userid = None;
    let mut chatid = None;
    let mut name = if let Some(user) = message.from() {
        userid = Some(user.id);
        if let Some(username) = &user.username {
            format!("@{username}")
        } else {
            user.full_name().to_string()
        }
    } else if let Some(chat) = message.sender_chat() {
        chatid = Some(chat.id);
        if let Some(username) = chat.username() {
            format!("@{} (chatid {})", username, chat.id)
        } else if let Some(title) = chat.title() {
            title.to_string()
        } else {
            // Shouldn't happen, but eh.
            "a private user".to_string()
        }
    } else {
        // Shouldn't happen either, but eh.
        "a private user".to_string()
    };

    if with_id {
        use std::fmt::Write;
        if let Some(userid) = userid {
            let _ = write!(name, " (userid {userid})");
        }
        if let Some(chatid) = chatid {
            let _ = write!(name, " (chatid {chatid})");
        }
    }

    name
}
