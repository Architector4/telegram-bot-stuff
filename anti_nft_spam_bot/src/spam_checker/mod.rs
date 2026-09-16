use teloxide::types::Message;

use crate::{
    database::Database,
    misc::iterate_over_all_links,
    sanitized_url::SanitizedUrl,
    types::{MessageDeleteReason, UrlDesignation},
};

/// Checks if this message is classified as spam. Doesn't check if it's sent by an admin or in a
/// private chat or somesuch. Returns a delete reason, if applicable.
pub async fn is_message_spam(
    message: &Message,
    database: &Database,
) -> Option<MessageDeleteReason> {
    // This message might be in an album that we want to delete.
    if let Some(album_id) = message.media_group_id() {
        let last_deleted = database
            .get_last_deleted_album_id(message.chat.id)
            .await
            .expect("Database died!");

        if last_deleted.as_ref() == Some(album_id) {
            // Matches album ID of last deleted spam message. Delete this too.
            return Some(MessageDeleteReason::OfAlbumWithSpamMessage);
        }
    }

    if does_message_have_spam_links(message, database).await {
        return Some(MessageDeleteReason::ContainsSpamLink);
    }

    if let Some(text) = message.text() {
        // temporary hack code until i arse myself to make proper text matching stuff
        if text.contains("HTTPS:// LUNASO . APP") {
            return Some(MessageDeleteReason::ContainsSpamLink);
        }
    }

    None
}

/// Returns true if the message is classified as spam, i.e. contains spam links or similar.
pub async fn does_message_have_spam_links(message: &Message, database: &Database) -> bool {
    for (sanitized_url, _original_url) in iterate_over_all_links(message) {
        if is_url_spam(database, &sanitized_url).await {
            return true;
        }
    }

    false
}

/// # Panics
///
/// Panics if the database dies lol
pub async fn is_url_spam(database: &Database, url: &SanitizedUrl) -> bool {
    if let Some(info) = database.get_url(url, false).await.expect("Database died!") {
        return info.designation() == UrlDesignation::Spam;
    }

    // No entry in the database found.

    // They keep cycling those, so it's kind of easier to just put this here lol
    if url.host_str() == "telegra.ph"
        && url.path().starts_with("/aktualnaya-ssylka-na-nashego-bota")
    {
        return true;
    }

    // TODO: automatic checking.

    false
}
