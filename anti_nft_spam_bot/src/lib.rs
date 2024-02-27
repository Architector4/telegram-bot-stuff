mod entry;
pub use entry::*;
use teloxide::types::ChatId;

mod handlers;

mod domains;

/// An ID of a private chat with the developers of the bot,
/// as well as volunteers who partake in manual review of links for spam.
pub static CONTROL_CHAT_ID: ChatId = ChatId(-1002065680710);
