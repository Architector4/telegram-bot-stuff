//! Source code for Anti NFT Spam Bot, aka `@Anti_NFT_Spam_Bot` on Telegram.

/// Sanitized URL type. Probably should go into types lol
mod sanitized_url;

/// Various types used throughout.
mod types;

/// The database.
mod database;

/// Miscellaneous functions.
mod misc;

/// Functions that perform stuff via the bot.
mod actions;

/// Functions that handle events from Telegram.
mod handlers;

/// Spam checker functionality.
mod spam_checker;

/// Entry function that starts the bot.
mod entry;
pub use entry::*;

use teloxide::types::ChatId;

/// An ID of a private chat with the developers of the bot,
/// as well as volunteers who partake in manual review of links for spam.
pub static CONTROL_CHAT_ID: ChatId = ChatId(-1002065680710);

/// An ID of a private channel used for logging manual reviews of URLs.
/// This is primarily to spot abuse and to note which URLs the bot
/// could have caught automatically but did not.
pub static REVIEW_LOG_CHANNEL_ID: ChatId = ChatId(-1002128704357);
