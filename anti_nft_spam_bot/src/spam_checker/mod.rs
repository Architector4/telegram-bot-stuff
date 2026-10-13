use teloxide::{types::Message, Bot};
use url::Url;

use crate::{
    actions::insert_or_update_url_with_log,
    database::Database,
    misc::iterate_over_all_links,
    sanitized_url::SanitizedUrl,
    types::{MessageDeleteReason, UrlDesignation},
};

/// Checks if this message is classified as spam. Doesn't check if it's sent by an admin or in a
/// private chat or somesuch. Returns a delete reason, if applicable.
pub async fn is_message_spam(
    bot: &Bot,
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

    if let Some(delete_reason) = check_links_in_message(bot, message, database).await {
        return Some(delete_reason);
    }

    if let Some(text) = message.text() {
        // temporary hack code until i arse myself to make proper text matching stuff
        if text.contains("HTTPS:// LUNASO . APP") {
            return Some(MessageDeleteReason::ContainsSpamLink);
        }
    }

    None
}

/// Check all links in this message for spam. If it finds one, a delete reason is returned.
///
/// This also includes converting a sticker's sticker pack into a link and checking it.
pub async fn check_links_in_message(
    bot: &Bot,
    message: &Message,
    database: &Database,
) -> Option<MessageDeleteReason> {
    for (sanitized_url, original_url) in iterate_over_all_links(message) {
        if is_url_spam(bot, database, &original_url, &sanitized_url).await {
            // This feels hacky, but it works lol
            if sanitized_url.host_str() == "t.me"
                && sanitized_url.path().starts_with("/addstickers/")
            {
                return Some(MessageDeleteReason::ContainsSpamSticker);
            } else {
                return Some(MessageDeleteReason::ContainsSpamLink);
            }
        }
    }

    None
}

/// Checks if the provided URL is spam via the database or checking.
///
/// `original_url` and `sanitized_url` must correspond to the same link.
///
/// # Panics
///
/// Panics if the database dies lol
pub async fn is_url_spam(
    bot: &Bot,
    database: &Database,
    original_url: &Url,
    sanitized_url: &SanitizedUrl,
) -> bool {
    // NOTE: This function technically has a TOCTOU issue.
    // First, it checks the database for a spam designation. Then, if that doesn't return anything,
    // it uses the checker, then tries to write its result into the database.
    //
    // A designation for this URL might come in right inbetween these two events. It might be a
    // manually reviewed designation, or maybe this same function on another thread.
    //
    // This is fine. Running the checker twice or thrice in a row is no big deal, and the call to
    // insert into the database specifies that this is an automatic designation, so it won't
    // overwrite a manually made one.

    if let Some(info) = database
        .get_url_full(sanitized_url, false)
        .await
        .expect("Database died!")
    {
        match info.designation() {
            UrlDesignation::NotSpam => return false,
            UrlDesignation::Spam => return true,
            UrlDesignation::Aggregator => {
                // The matching entry is an aggregator.

                // TODO: adjust database.get_url to somehow indicate if it was an exact match or
                // not. This would allow using that instead of get_url_full here.

                if info.sanitized_url() == sanitized_url {
                    // If it's this link specifically, then we know it's not spam because we know
                    // it's an aggregator.
                    return false;
                }

                // Else, as per aggregator's definition, fall over to the checker.
            }
        }
    }

    // No entry in the database found, or is under an aggregator. Gotta run the checker.

    if let Some(is_spam) = check_url_for_spam(original_url, sanitized_url) {
        // Checker got something! Write this to the database.

        let new_designation = if is_spam {
            UrlDesignation::Spam
        } else {
            UrlDesignation::NotSpam
        };
        insert_or_update_url_with_log(
            bot,
            database,
            None,
            sanitized_url,
            original_url,
            new_designation,
        )
        .await
        .expect("Database died!");

        return is_spam;
    }

    // Checker has no clue either. Assume not spam.
    false
}

/// Checks the supplied URL for spam, maybe by visiting it.
///
/// Provided `Url` and `SanitizedUrl` arguments must correspond to the same link.
pub fn check_url_for_spam(_original_url: &Url, sanitized_url: &SanitizedUrl) -> Option<bool> {
    // They keep cycling those, so it's kind of easier to just put this here lol
    if sanitized_url.host_str() == "telegra.ph"
        && sanitized_url
            .path()
            .starts_with("/aktualnaya-ssylka-na-nashego-bota")
    {
        return Some(true);
    }

    // Also an annoying case. It's all random porn.
    if sanitized_url.host_str() == "t.me"
        && sanitized_url
            .path()
            .strip_circumfix("/addstickers/erotic_", "stickers_by_stickerio_robot")
            .is_some()
    {
        return Some(true);
    }

    None
}
