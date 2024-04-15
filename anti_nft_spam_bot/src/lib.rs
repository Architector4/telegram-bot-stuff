mod database;
mod domain_checker;
mod entry;
mod handlers;
mod types;

pub use entry::*;

use teloxide::types::ChatId;
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
            if let Ok(url) = Url::parse(&format!("http://{}", string)) {
                Ok(url)
            } else {
                Err(e)
            }
        }
    }
}
