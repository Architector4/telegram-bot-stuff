// yo dawg we heard you like imports

use std::sync::Arc;

use arch_bot_commons::useful_methods::{BotArchSendMsg, MessageStuff};
use teloxide::{
    prelude::*,
    sugar::request::{RequestLinkPreviewExt, RequestReplyExt},
    types::{BotCommand, MaybeInaccessibleMessage, Me},
    ApiError, RequestError,
};

use crate::{
    actions::{
        authenticate_control, authenticate_control_of_sender, delete_message_as_spam,
        discard_review_keyboard, edit_message_into_a_new_review_keyboard,
        insert_or_update_url_with_log, send_new_review_keyboard, send_review_header,
        send_review_keyboard,
    },
    database::{Database, InsertOrUpdateResult, SendToReviewResult},
    misc::{
        does_message_have_spam_links, get_entity_url, is_sender_admin, is_sender_admin_with_cache,
        iterate_over_all_links, sender_name_prettyprint, user_name_prettyprint,
    },
    types::{MessageDeleteReason, ReviewCallbackData, UrlDesignation},
    CONTROL_CHAT_ID,
};

/// Handler for events of new or edited messages.
pub async fn handle_message_new_or_edit(
    bot: Bot,
    me: Me,
    message: Message,
    database: Arc<Database>,
) -> Result<(), RequestError> {
    let result = handle_message_new_or_edit_raw(&bot, &me, &message, &database).await;

    match result {
        Err(RequestError::Api(ApiError::Unknown(reason)))
            if reason.contains("CHAT_ADMIN_REQUIRED") =>
        {
            // Bot is not an admin and the handler failed because of this. Be a bit more graceful
            // about it than to just send it back.

            bot.archsendmsg_no_link_preview(message.chat.id, concat!(
                    "Failed to perform an operation because this bot is not an admin in this chat.\n\n",
                    "For proper functioning, please make this bot an admin with ability to remove messages, ",
                    "or remove it from this chat."
            ), None).await?;

            Ok(())
        }
        Err(e) => {
            log::error!("Error {e} when handling message:\n{message:#?}");

            Err(e)
        }
        Ok(()) => Ok(()),
    }
}

/// Handler for events of new or edited messages.
/// Assumes the bot is admin and might return an error otherwise.
async fn handle_message_new_or_edit_raw(
    bot: &Bot,
    me: &Me,
    message: &Message,
    database: &Database,
) -> Result<(), RequestError> {
    if let Some(sender) = &message.from {
        if sender.id == me.id {
            // Ignore messages sent by ourselves.
            // Probably won't appear ever, but eh.
            return Ok(());
        }
    }

    let sender_name = sender_name_prettyprint(message, false);

    // Whether or not the sender of this message is an admin, if checked.
    // This variable is used to have to check this at most once across this function.
    let mut sent_by_admin_cache: Option<bool> = None;

    // This block below handles checking for spam and deleting the message, either now, or later due
    // to a review of one of the URLs contained within.
    if !message.chat.is_private() {
        let mut deleted_this_message = false;

        // This message might be in an album that we want to delete.
        if let Some(album_id) = message.media_group_id() {
            let last_deleted = database
                .get_last_deleted_album_id(message.chat.id)
                .await
                .expect("Database died!");

            if last_deleted.as_ref() == Some(album_id) {
                // Matches album ID of last deleted spam message. Delete this too.
                delete_message_as_spam(
                    bot,
                    database,
                    message,
                    Some(&sender_name),
                    MessageDeleteReason::OfAlbumWithSpamMessage,
                )
                .await?;

                deleted_this_message = true;
            }
        }

        if !deleted_this_message && does_message_have_spam_links(message, database).await {
            if is_sender_admin_with_cache(bot, message, &mut sent_by_admin_cache).await? {
                if message.chat.id != CONTROL_CHAT_ID {
                    bot.archsendmsg_no_link_preview(
                        message.chat.id,
                        "Skipping deleting a message from an admin containing a spam link.",
                        None,
                    )
                    .await?;
                }
            } else {
                delete_message_as_spam(
                    bot,
                    database,
                    message,
                    Some(&sender_name),
                    MessageDeleteReason::ContainsSpamLink,
                )
                .await?;
                deleted_this_message = true;
            }
        }

        if let Some(reply_to) = message.reply_to_message() {
            // If this is a reply to another message within the same chat, check that message for
            // spam links too. Probably a second time over. A link in that message might have been
            // marked as spam since then.
            if message.chat.id == reply_to.chat.id
                && does_message_have_spam_links(reply_to, database).await
                && !is_sender_admin(bot, reply_to).await?
            {
                delete_message_as_spam(
                    bot,
                    database,
                    reply_to,
                    None,
                    MessageDeleteReason::ContainsSpamLink,
                )
                .await?;
            }
        }

        if deleted_this_message {
            // This message was deleted as spam. No need for any further handling.
            return Ok(());
        }

        // Above passed. This message was not deleted as spam.
        //
        // However, some of the links might be on review, and later might be marked as spam.
        // In such case, we want to retroactively delete all messages where that link was seen that
        // are elegible for deletion.
        //
        // Therefore, tell the database about the links.
        for (sanitized_url, _original_url) in iterate_over_all_links(message) {
            // Only if this is *not* sent by a chat admin. We don't want to delete admin messages
            // containing spam links.
            if !is_sender_admin_with_cache(bot, message, &mut sent_by_admin_cache).await? {
                if let Err(e) = database
                    .link_sighted(message.chat.id, message.id, &sender_name, &sanitized_url)
                    .await
                {
                    log::error!("Database failed to sight link {sanitized_url}!\n{e:?}");
                }
            }
        }
    }

    // Above passed. This message was not deleted as spam and links were sighted.
    let is_edited = message.edit_date().is_some();

    if !is_edited {
        // Handle commands, potentially.
        handle_command(
            bot,
            me,
            message,
            database,
            &sender_name,
            sent_by_admin_cache,
        )
        .await?;
    }

    Ok(())
}

/// Check if this message is a command, and if so, handle it.
///
/// # Panics
///
/// Panics if the database dies lol
pub async fn handle_command(
    bot: &Bot,
    me: &Me,
    message: &Message,
    database: &Database,
    sender_name: &str,
    mut sent_by_admin_cache: Option<bool>,
) -> Result<(), RequestError> {
    let Some(mut text) = message.text_full() else {
        // shrug
        return Ok(());
    };

    let is_private = message.chat.is_private();

    // Special case: nag for cash money if someone says "good bot" uwu
    static GOOD_BOT: &str = "good bot";
    if let Some((maybe, _)) = text.split_at_checked(GOOD_BOT.len()) {
        if maybe.eq_ignore_ascii_case(GOOD_BOT)
            && (is_private
                || message
                    .reply_to_message()
                    .and_then(|x| x.from.as_ref())
                    .is_some_and(|x| x.id == me.id))
        {
            static NAG: &str =
                "<a href=\"https://boosty.to/architector_4\">(Consider supporting? 👉👈)</a>";
            bot.send_message(message.chat.id, NAG)
                .reply_to(message.id)
                .parse_mode(teloxide::types::ParseMode::Html)
                .disable_link_preview(true)
                .await?;
            return Ok(());
        }
    }

    // Commands start with a forward slash.
    if !text.starts_with('/') {
        // Not a command. Is this in DMs to the bot? If so, think of this as "/start" lol
        if is_private {
            text = "/start";
        } else {
            return Ok(());
        }
    }

    // Get first word in message, the command itself.
    let Some(mut command) = text.split_whitespace().next() else {
        return Ok(());
    };

    let command_full_len = command.len();

    // Strip bot username from the end such that a command like "/spam@Anti_NFT_Spam_Bot" would
    // become just "/spam"
    if let Some(command_no_username) = command.strip_suffix(me.username()) {
        if let Some(command_no_username_and_at) = command_no_username.strip_suffix('@') {
            command = command_no_username_and_at;
        }
    }

    let _params = &text[command_full_len..].trim_start();

    // Lowercase, if needed.
    let tmp;
    if !command.chars().map(char::is_lowercase).any(|x| !x) {
        tmp = command.to_lowercase();
        command = tmp.as_str();
    }

    match command {
        "/start" | "/help" if is_private => {
            bot.archsendmsg_no_link_preview(
                message.chat.id, concat!(
"This bot is made to combat various types of spam experienced by chats across Telegram.\n\n",
"To use this bot, add it to a chat and give it administrator status with \"Remove messages\" permission.\n\n",
"No further setup is required. A message will be sent when spam is removed.\n\n",
"For available commands, type / into the message text box below and see the previews.\n\n"
),
                message.id,
            )
            .await?;

            let is_reviewer = authenticate_control_of_sender(bot, message).await?;

            if is_reviewer {
                // Also note super special super secret commands.
                bot.archsendmsg_no_link_preview(message.chat.id, concat!(
"Super special reviewer commands:\n\n",

"/mark_spam &lt;URL&gt;, /mark_url_spam &lt;URL&gt; - Insert or update an entry for a URL as spam.\n\n",

"/mark_domain_spam &lt;URL&gt; - Remove path and query parameters from the URL then insert/update an ",
"entry for the resulting host-only URL as spam.\n\n",

"/mark_not_spam &lt;URL&gt; - Insert or update an entry for a URL as not spam.\n\n",

"/mark_aggregator &lt;URL&gt; - Insert or update an entry for a URL as a link aggregator. ",
"A link aggregator is considered to be not spam itself, but URLs below it will be automatically checked.\n\n",

"/review - Initiate a review keyboard.\n\n",

"/info &lt;URL&gt; - Find a database entry that matches this URL and print its contents.\n\n",
), message.id)
                    .await?;
            }

            Ok(())
        }
        "/hide_deletes" | "/show_deletes" => {
            handle_command_show_hide_deletes(
                bot,
                message,
                database,
                &mut sent_by_admin_cache,
                command,
            )
            .await
        }
        "/spam" | "/scam" => handle_command_spam(bot, message, database, sent_by_admin_cache).await,
        "/mark_spam" | "/mark_url_spam" | "/mark_domain_spam" | "/mark_not_spam"
        | "/mark_aggregator" => {
            handle_command_mark(bot, message, database, sender_name, command).await
        }
        "/review" => handle_command_review(bot, message, database).await,
        "/info" => handle_command_info(bot, message, database).await,
        // NOTE: When adding new commands, also add them to `generate_bot_commands` function below.
        _ if is_private => {
            bot.archsendmsg_no_link_preview(
                message.chat.id,
                concat!(
                    "Unknown command. Try /start for bot info, or use bot commands button ",
                    "on your message entry bar to see commands."
                ),
                message.id,
            )
            .await?;
            Ok(())
        }
        _ => {
            // Woop.
            Ok(())
        }
    }
}

/// Generate a list of bot commands to be shown as available by the bot.
#[must_use]
pub fn generate_bot_commands() -> Vec<BotCommand> {
    vec![
        BotCommand::new("/spam", "Mark links in a message for review as spam."),
        BotCommand::new("/hide_deletes", "Hide spam deletion notification messages."),
        BotCommand::new(
            "/show_deletes",
            "Don't hide spam deletion notification messages.",
        ),
    ]
}

/// Handle this message assuming it's the command `/review`.
async fn handle_command_review(
    bot: &Bot,
    message: &Message,
    database: &Database,
) -> Result<(), RequestError> {
    if !authenticate_control_of_sender(bot, message).await? {
        return Ok(());
    }

    send_new_review_keyboard(bot, message.chat.id, Some(message.id), database).await?;

    Ok(())
}

/// Handle this message assuming it's the command `/show_deletes` or `/hide_deletes`.
async fn handle_command_show_hide_deletes(
    bot: &Bot,
    message: &Message,
    database: &Database,
    sent_by_admin_cache: &mut Option<bool>,
    command: &str,
) -> Result<(), RequestError> {
    if message.chat.is_private()
        || !is_sender_admin_with_cache(bot, message, sent_by_admin_cache).await?
    {
        bot.archsendmsg_no_link_preview(
            message.chat.id,
            "This command can only be used by admins in group chats.",
            message.id,
        )
        .await?;
        return Ok(());
    }

    let new_state = command == "/hide_deletes";

    let old_state = database
        .set_hide_deletes(message.chat.id, new_state)
        .await
        .expect("Database died!");

    let response = match (old_state, new_state) {
        (false, false) => "This chat doesn't hide spam deletion notifications already.",
        (false, true) => concat!(
            "I will no longer notify about messages being deleted. ",
            "Note that this may lead to confusion in case I delete a ",
            "message with a legitimate link due to a false positive. "
        ),
        (true, false) => "From now on I will notify about spam messages being deleted.",
        (true, true) => "This chat has spam delete notifications hidden already.",
    };

    bot.archsendmsg_no_link_preview(message.chat.id, response, message.id)
        .await?;
    Ok(())
}

/// Handle this message assuming it's the command `/hide`.
async fn handle_command_info(
    bot: &Bot,
    message: &Message,
    database: &Database,
) -> Result<(), RequestError> {
    if !(message.chat.is_private() || message.chat.id == CONTROL_CHAT_ID) {
        // Not an appropriate chat for this.
        return Ok(());
    }

    if !authenticate_control_of_sender(bot, message).await? {
        // Not someone who should be able to query stuff.
        return Ok(());
    }

    let mut response = String::with_capacity(64);

    for (sanitized_url, _original_url) in iterate_over_all_links(message).chain(
        message
            .reply_to_message()
            .into_iter()
            .flat_map(iterate_over_all_links),
    ) {
        use std::fmt::Write;
        writeln!(response, "<b>For URL</b> {sanitized_url}:")
            .expect("Writing to a String never fails");

        let short = match database.get_url(&sanitized_url, false).await {
            Err(e) => {
                write!(response, "DATABASE ERROR ON SHORT: {e:#?}\n\n")
                    .expect("Writing to a String never fails");
                continue;
            }
            Ok(Some(short)) => short,
            Ok(None) => {
                response.push_str("No result.\n\n");
                continue;
            }
        };

        let long = match database.get_url_by_id_full(short.id()).await {
            Err(e) => {
                write!(response, "DATABASE ERROR ON LONG: {e:#?}\n\n")
                    .expect("Writing to a String never fails");
                continue;
            }
            Ok(Some(short)) => short,
            Ok(None) => {
                response.push_str("Result has vanished.\n\n");
                continue;
            }
        };

        write!(response, "{long}\n\n").expect("Writing to a String never fails");
    }

    let response = if response.is_empty() {
        "No URLs found to find info on."
    } else {
        &response
    };

    bot.archsendmsg_no_link_preview(message.chat.id, response, message.id)
        .await?;

    Ok(())
}

/// Handle this message assuming it's the command `/mark_spam`, `/mark_url_spam`, `/mark_domain_spam`,
/// `/mark_not_spam`, or `/mark_aggregator`.
///
/// # Panics
/// Panics if `command` is not one of those.
async fn handle_command_mark(
    bot: &Bot,
    message: &Message,
    database: &Database,
    sender_name: &str,
    command: &str,
) -> Result<(), RequestError> {
    if !(message.chat.is_private() || message.chat.id == CONTROL_CHAT_ID) {
        // Not an appropriate chat for this.
        return Ok(());
    }

    if !authenticate_control_of_sender(bot, message).await? {
        // Not someone who can review stuff.
        return Ok(());
    }

    let (designation, header) = match command {
        "/mark_spam" | "/mark_url_spam" | "/mark_domain_spam" => {
            (UrlDesignation::Spam, "Marked these URLs as spam:\n")
        }
        "/mark_not_spam" => (UrlDesignation::NotSpam, "Marked as not spam:\n"),
        "/mark_aggregator" => (UrlDesignation::Aggregator, "Marked as a link aggregator:\n"),
        _ => unreachable!(),
    };

    let mut response = String::with_capacity(64);

    // True if this has changed at least one link in the database.
    let mut had_links_changed = false;
    // True if this tried to write a link that was already written as is in the database.
    let mut had_links_unchanged = false;

    for (mut sanitized_url, mut original_url) in iterate_over_all_links(message) {
        if command == "/mark_domain_spam" {
            original_url.set_fragment(None);
            original_url.set_query(None);
            original_url.set_path("");
            sanitized_url.remove_all_but_host();
        }

        let result = insert_or_update_url_with_log(
            bot,
            database,
            Some(sender_name),
            &sanitized_url,
            &original_url,
            designation,
        )
        .await?;

        match result {
            InsertOrUpdateResult::Inserted { .. } | InsertOrUpdateResult::Updated { .. } => {
                if !had_links_changed {
                    response.push_str(header);
                    had_links_changed = true;
                }

                match result {
                    InsertOrUpdateResult::Inserted { .. } => {
                        response.push_str("<b>New: </b>");
                    }
                    InsertOrUpdateResult::Updated { .. } => {
                        response.push_str("<b>Updated: </b>");
                    }
                    InsertOrUpdateResult::NoChange { .. } => unreachable!(),
                }
                response.push_str(sanitized_url.as_str());
                response.push('\n');
            }
            InsertOrUpdateResult::NoChange { .. } => {
                had_links_unchanged = true;
            }
        }
    }

    let footer = match (had_links_unchanged, had_links_changed) {
        (false, false) => "\nPlease specify links. Replies don't count to avoid accidents.",
        (true, false) => "\nThese URLs are already marked as such.",
        (true, true) => "\nSome URLs are skipped as they are already marked as such.",
        (false, true) => "",
    };

    response.push_str(footer);

    bot.archsendmsg_no_link_preview(message.chat.id, response.as_str(), message.id)
        .await?;

    Ok(())
}

/// Handle this message assuming it's the command `/spam` or `/scam`.
async fn handle_command_spam(
    bot: &Bot,
    message: &Message,
    database: &Database,
    mut sent_by_admin_cache: Option<bool>,
) -> Result<(), RequestError> {
    let mut replied_to_sent_by_admin_cache: Option<bool> = None;

    // This or replied-to message may have sus links. Time to do stuff!

    // First, check and skip if this is a reply to a post by
    // the channel that is linked to this chat.
    // This is worth doing because such messages are sanctioned by
    // the chat's admins, and are most often just a misclick of
    // the blue /spam command in comments to channel posts.
    //
    // The check for above is accomplished by is_sender_admin function,
    // but there are other corner cases to consider too.

    // If this scope runs to the end (i.e. doesn't hit any break), reject this as a reply to an admin.
    'reject_from_admin: {
        let Some(reply_to) = message.reply_to_message() else {
            // This isn't a reply to anything.
            break 'reject_from_admin;
        };

        if message
            .from
            .as_ref()
            .is_some_and(|x| Some(x.id) == reply_to.from.as_ref().map(|x| x.id))
            || message
                .sender_chat
                .as_ref()
                .is_some_and(|x| Some(x.id) == reply_to.sender_chat.as_ref().map(|x| x.id))
        {
            // The sender is replying to themselves and knows what they're doing.
            break 'reject_from_admin;
        }

        if !is_sender_admin_with_cache(bot, reply_to, &mut replied_to_sent_by_admin_cache).await? {
            // The sender of the replied-to message isn't an admin.
            break 'reject_from_admin;
        }

        if is_sender_admin_with_cache(bot, message, &mut sent_by_admin_cache).await? {
            // The sender of this message *is* an admin.
            break 'reject_from_admin;
        }

        // The two checks above will return true for private chats without extra
        // Telegram API queries.

        // This is an applicable situation.
        let response = concat!(
            "Sorry, but the message you're replying to is posted by an admin of this chat, ",
            "so it is ignored. If you believe it should be marked, ",
            "DM this bot with the command <code>/spam badlink.com</code> to submit it anyway."
        );
        bot.archsendmsg_no_link_preview(message.chat.id, response, message.id)
            .await?;
        return Ok(());
    }

    // Nah, they fr. We actually have to do things now.

    // Get sender names appropriate for notifications.
    let sender_name_with_id = sender_name_prettyprint(message, true);
    let replied_to_sender_name_with_id = message
        .reply_to_message()
        .map(|x| sender_name_prettyprint(x, false))
        .unwrap_or_default();

    // Keep track which categories of links have we seen.
    let mut some_marked = false;
    let mut some_already_on_review = false;
    let mut some_already_marked_spam = false;
    let mut some_already_manually_reviewed_as_not_spam = false;

    // The big iterator over all links in this message as well as the message it's a reply to.
    // Additionally, for every link, it includes the message it's from, and which by_admin cache to
    // use (false for this message's, true for replied to message's).
    let the_big_iterator = iterate_over_all_links(message)
        .map(|(s, u)| (s, u, message, false, &sender_name_with_id))
        .chain(
            message
                .reply_to_message()
                .into_iter()
                .flat_map(iterate_over_all_links)
                .map(|(sanitized_url, original_url)| {
                    (
                        sanitized_url,
                        original_url,
                        message
                            .reply_to_message()
                            .expect("If this is run, then replied-to message must exist."),
                        true,
                        &replied_to_sender_name_with_id,
                    )
                }),
        );

    let mut review_header_posted = false;

    // Iterate over all the links ever.
    for (
        sanitized_url,
        original_url,
        reported_message,
        admin_cache_is_of_replied_to,
        sender_name,
    ) in the_big_iterator
    {
        let this_sent_by_admin_cache = if admin_cache_is_of_replied_to {
            &mut replied_to_sent_by_admin_cache
        } else {
            &mut sent_by_admin_cache
        };

        let sent_by_admin =
            is_sender_admin_with_cache(bot, reported_message, this_sent_by_admin_cache).await?;

        let result = database
            .send_to_review(&sanitized_url, &original_url)
            .await
            .expect("Database died!");

        match result {
            SendToReviewResult::Sent { review_entry_id } => {
                if !sent_by_admin {
                    database
                        .link_sighted(
                            reported_message.chat.id,
                            reported_message.id,
                            sender_name,
                            &sanitized_url,
                        )
                        .await
                        .expect("Database died!");
                }
                some_marked = true;

                if !review_header_posted {
                    send_review_header(bot, reported_message, message, Some(&sender_name_with_id))
                        .await?;
                    review_header_posted = true;
                }

                send_review_keyboard(
                    bot,
                    CONTROL_CHAT_ID,
                    None,
                    review_entry_id,
                    &sanitized_url,
                    &original_url,
                    database,
                )
                .await?;
            }
            SendToReviewResult::AlreadyOnReview => some_already_on_review = true,
            SendToReviewResult::AlreadyInDatabase(info) => match info.designation() {
                UrlDesignation::Spam => some_already_marked_spam = true,
                UrlDesignation::NotSpam | UrlDesignation::Aggregator => {
                    some_already_manually_reviewed_as_not_spam = true;
                }
            },
        }
    }

    // All links marked as appropriate. Tell the user about it.
    let response = if some_marked {
        "Thank you, links in this message will be reviewed for spam."
    } else if some_already_on_review {
        "Thank you, but the links in this message are already marked for review."
    } else if some_already_marked_spam {
        "Thank you, but links in this message are already marked as spam."
    } else if some_already_manually_reviewed_as_not_spam {
        "Thank you, but the links in this message were manually reviewed and were determined to be not spam."
    } else {
        // No links at all.
        concat!(
            "Sorry, but I could not find any links in ",
            "your message or the message you replied to, if any. ",
            "This bot only blocks messages with usernames, links, and buttons with links."
        )
    };

    bot.archsendmsg_no_link_preview(message.chat.id, response, message.id)
        .await?;

    Ok(())
}

/// Handle a callback query, assuming it's from a review keyboard.
pub async fn handle_callback_query(
    bot: Bot,
    query: CallbackQuery,
    database: Arc<Database>,
) -> Result<(), RequestError> {
    macro_rules! goodbye {
        ($text:expr) => {{
            bot.answer_callback_query(query.id).text($text).await?;
            return Ok(());
        }};
        () => {{
            bot.answer_callback_query(query.id).await?;
            return Ok(());
        }};
    }

    let message = match query.message {
        Some(MaybeInaccessibleMessage::Regular(message)) => message,
        Some(MaybeInaccessibleMessage::Inaccessible(_)) | None => {
            goodbye!("Sorry, this review message is too old.");
        }
    };

    if !(message.chat.is_private() || message.chat.id == CONTROL_CHAT_ID) {
        // In case someone is messing around and somehow sending this event from a random group
        // chat or something.
        goodbye!("huhh???   ?????   guh??");
    }

    let Some(query_data) = query
        .data
        .and_then(|d| ReviewCallbackData::deserialize_from_str(&d))
    else {
        goodbye!("Failed to parse query data.");
    };

    let user = query.from;

    if !authenticate_control(&bot, &user).await? {
        goodbye!("Access denied.");
    }

    let entities = message
        .parse_entities()
        .or_else(|| message.parse_caption_entities())
        .unwrap_or_default();

    let Some((sanitized_url, original_url)) = entities.iter().find_map(|x| get_entity_url(x))
    else {
        goodbye!("Review message without a URL to review. How peculiar!");
    };

    if query_data.url_crc32 != crc32fast::hash(sanitized_url.as_str().as_bytes()) {
        goodbye!("CRC32 hash mismatch! Something is fucky. Report to bot developer lmao");
    }

    let Some(sanitized_url) = sanitized_url.destructure_to_number(query_data.destructure) else {
        goodbye!("Too much URL destructuring. Something is fucky. Report to bot developer lmao");
    };

    // This message is a review keyboard that was just used.
    // Remove it from the database and handle it by ourselves afterward.

    database
        .review_keyboard_removed(message.chat.id, message.id)
        .await
        .expect("Database died!");

    insert_or_update_url_with_log(
        &bot,
        &database,
        Some(&user_name_prettyprint(&user, true, true)),
        &sanitized_url,
        &original_url,
        query_data.designation,
    )
    .await?;

    // We did the funny. Now for the aftermatch.

    if message.chat.is_private() {
        // If this is in a private chat, then this is probably a /review keyboard.
        edit_message_into_a_new_review_keyboard(&bot, message.chat.id, message.id, &database)
            .await?;
    } else {
        // If this is in the control chat (or whereever else??), edit to remove the keyboard.
        discard_review_keyboard(
            &bot,
            message.chat.id,
            message.id,
            &user_name_prettyprint(&user, true, true),
            query_data.designation,
            &sanitized_url,
        )
        .await?;
    }

    Ok(())
}
