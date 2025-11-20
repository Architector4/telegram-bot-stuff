use arch_bot_commons::{teloxide_retry, useful_methods::BotArchSendMsg};
use futures_util::TryStreamExt;
use html_escape::encode_text;
use teloxide::{
    payloads::{EditMessageTextSetters, SendMessageSetters},
    prelude::Requester,
    sugar::request::{RequestLinkPreviewExt, RequestReplyExt},
    types::{ChatId, InlineKeyboardMarkup, MediaGroupId, Message, MessageId, User},
    ApiError, Bot, RequestError,
};
use url::Url;

use crate::{
    database::{Database, InsertOrUpdateResult, UrlInfoFull},
    misc::{chat_name_prettyprint, sender_name_prettyprint, user_name_prettyprint},
    sanitized_url::SanitizedUrl,
    types::{MessageDeleteReason, ReviewCallbackData, UrlDesignation},
    CONTROL_CHAT_ID, REVIEW_LOG_CHANNEL_ID,
};

/// Check if this user is in the control chat and can do reviews, and
/// delay their requests if appropriate.
pub async fn authenticate_control(bot: &Bot, user: &User) -> Result<bool, RequestError> {
    let control = bot
        .get_chat_member(CONTROL_CHAT_ID, user.id)
        .await?
        .is_present();
    if !control {
        let username = user_name_prettyprint(user, true, false);
        log::info!("Unauthorized user trying to access reviews: {username}");

        // Not a member.
        //
        // Now, facts:
        // 1. This function will only be run in context of a private chat.
        //
        // 2. Teloxide intentionally processes messages from one chat not-concurrently; that is, if
        //    we delay now, this will delay processing all following direct messages sent by that
        //    person to this bot.
        //
        // 3. There is no pertinent reason to DM this bot other than to get the help message or for
        //    authenticated user's purposes.
        //
        // 4. If a user is sending DMs to this bot, that means that they have already sent
        //    `/start`, and hence have already seen the help message.
        //
        // 5. Therefore, there is no harm to be done by delaying users not legible for reviews for
        //    DMs.
        //
        // 6. Bad actors may want to try and spam this bot `/review` to cause it to send the above
        //    API request many times and in turn get rate limited by telegram.
        //
        // With that in mind, delay this user from accessing this bot for 5 seconds.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    Ok(control)
}

/// Convenience function over [`authenticate_control`].
pub async fn authenticate_control_of_sender(
    bot: &Bot,
    message: &Message,
) -> Result<bool, RequestError> {
    if message.chat.id == CONTROL_CHAT_ID {
        return Ok(true);
    }

    let Some(sender) = &message.from else {
        return Ok(false);
    };

    authenticate_control(bot, sender).await
}

/// Delete the message and maybe say in the chat that it was deleted, if they don't have
/// `/hide_deletes` enabled.
///
/// # Panics
///
/// Panics if the database dies lol
pub async fn delete_message_as_spam_raw(
    bot: &Bot,
    database: &Database,
    chat_id: ChatId,
    message_id: MessageId,
    album_id: Option<&MediaGroupId>,
    sender_name: &str,
    reason: MessageDeleteReason,
) -> Result<(), RequestError> {
    match teloxide_retry!(bot.delete_message(chat_id, message_id).await) {
        Ok(_) => {
            if let Some(album_id) = album_id {
                // Not *too* important if this fails lol
                let _ = database.set_last_deleted_album_id(chat_id, album_id).await;
            }

            // Now we shall notify. Should we show the reason?
            if let Some(reason) = reason.to_str() {
                // We should. Do we *need* to?
                let deletes_hidden = database
                    .get_hide_deletes(chat_id)
                    .await
                    .expect("Database died!");

                if !deletes_hidden {
                    // We should.

                    // Message or album?
                    let deleted_thing_type = if album_id.is_some() {
                        "an album"
                    } else {
                        "a message"
                    };

                    bot.archsendmsg_no_link_preview(
                        chat_id,
                        format!(
                            "Deleted {} from <code>{}</code> {}.",
                            deleted_thing_type,
                            encode_text(sender_name),
                            reason
                        )
                        .as_str(),
                        None,
                    )
                    .await?;
                }
            }
            Ok(())
        }
        Err(RequestError::Api(ApiError::MessageIdInvalid | ApiError::MessageToDeleteNotFound)) => {
            // Someone else probably has already deleted it. That's fine.
            Ok(())
        }

        Err(RequestError::Api(ApiError::MessageCantBeDeleted)) => {
            // No rights? Older than 48 hours?
            bot.archsendmsg_no_link_preview(
                chat_id,
                concat!(
                    "Tried to delete a spam message, but failed. ",
                    "This might be because this bot is not an admin with ability to ",
                    "delete messages, or the message is older than 48 hours.",
                ),
                None,
            )
            .await?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Convenience function around [`delete_message_as_spam_raw`].
pub async fn delete_message_as_spam(
    bot: &Bot,
    database: &Database,
    message: &Message,
    sender_name: Option<&str>,
    reason: MessageDeleteReason,
) -> Result<(), RequestError> {
    let mut tmp = String::new();
    let sender_name = sender_name.unwrap_or_else(|| {
        tmp = sender_name_prettyprint(message, false);
        &tmp
    });

    delete_message_as_spam_raw(
        bot,
        database,
        message.chat.id,
        message.id,
        message.media_group_id(),
        sender_name,
        reason,
    )
    .await
}

/// Inserts/updates this URL in the database with incoming info, logs if necessary, and removes
/// entries from the review queue.
///
/// Set `review_name` to [`None`] to indicate that this review has been done automatically by the
/// spam checker.
///
/// If the result is [`InsertOrUpdateResult::NoChange`], no logging is done, but entries in review
/// queue will still be deleted.
pub async fn insert_or_update_url_with_log(
    bot: &Bot,
    database: &Database,
    reviewer_name: Option<&str>,
    sanitized_url: &SanitizedUrl,
    original_url: &Url,
    designation: UrlDesignation,
) -> Result<InsertOrUpdateResult, RequestError> {
    let manually_reviewed = reviewer_name.is_some();
    let reviewer_name = reviewer_name.unwrap_or("[AUTO] Spam checker");

    let result = database
        .insert_or_update_url(sanitized_url, original_url, designation, manually_reviewed)
        .await
        .expect("Database died!");

    if let InsertOrUpdateResult::NoChange { .. } = result {
        // We did nothing. Not worth logging.
        return Ok(result);
    }

    // Change was enacted. Log it.

    let command = match designation {
        UrlDesignation::Spam => "/mark_spam",
        UrlDesignation::NotSpam => "/mark_not_spam",
        UrlDesignation::Aggregator => "/mark_aggregator",
    };

    let mut log_message = format!("{reviewer_name}\n{command}\n{sanitized_url}");
    if sanitized_url.as_str() != original_url.as_str() {
        log_message.push_str("\n\nOriginal URL: ");
        log_message.push_str(original_url.as_str());
    }

    // Now to discard URLs on review matching this new rule.
    // This requires also discarding all keyboards and sightings pertaining it.

    while let Some(review_entry_id) = database
        .find_one_matching_review_queue_entry(result.id())
        .await
        .expect("Database died!")
    {
        // After deleting keyboards/sightings but before deleting the review URL,
        // someone might add a new keyboard/sighting, and fail deleting it.
        // So, basically, retry until we get it lol
        loop {
            // Discard review keyboards.
            let mut keyboards = database.pop_review_keyboards(review_entry_id);
            while let Some((chat_id, message_id)) =
                keyboards.try_next().await.expect("Database died!")
            {
                // No biggie if this fails.
                let _ = discard_review_keyboard(
                    bot,
                    chat_id,
                    message_id,
                    reviewer_name,
                    designation,
                    sanitized_url,
                )
                .await;
            }

            let mut sightings = database.pop_review_link_sightings(review_entry_id);
            while let Some((chat_id, message_id, sender_name)) =
                sightings.try_next().await.expect("Database died!")
            {
                if designation == UrlDesignation::Spam {
                    // Not *really* a biggie if this fails.
                    let _ = delete_message_as_spam_raw(
                        bot,
                        database,
                        chat_id,
                        message_id,
                        None,
                        &sender_name,
                        MessageDeleteReason::ContainsSpamLink,
                    )
                    .await;
                }
            }

            // And now try getting rid of it.
            match database.delete_from_review(review_entry_id).await {
                Err(sqlx::Error::Database(e))
                    if e.kind() == sqlx::error::ErrorKind::ForeignKeyViolation =>
                {
                    // Oops! New review/keyboard was made.
                    // Loop over and remove it.
                }
                Ok(()) => break,
                Err(e) => panic!("Database died!: {e:?}"),
            }
        }
    }

    bot.archsendmsg_no_link_preview(REVIEW_LOG_CHANNEL_ID, log_message.as_str(), None)
        .await?;

    Ok(result)
}

/// Make a header in control chat. describing upcoming review keyboards. Should typically be
/// followed with messages containing said review keyboards.
///
/// `reported` message is the one with the spam links in question.
///
/// `reporting` message is the one that initiated this spam report; typically starts with "/spam"
/// or such.
///
/// For the above two parameters, the same message can be passed twice.
///
/// `sender_name` should be formatted from the `reporting` message, or set to [`None`] if this review is
/// initiated automatically by the spam checker.
pub async fn send_review_header(
    bot: &Bot,
    reported: &Message,
    reporting: &Message,
    sender_name: Option<&str>,
) -> Result<(), RequestError> {
    let sender_name = sender_name.unwrap_or("automatic check");
    let chat_name = chat_name_prettyprint(&reporting.chat, true);

    let notify_text =
        format!("New link(s) were added to review pool by {sender_name} in {chat_name}");

    let same_chat = reported.chat.id == reporting.chat.id;
    let same_message = same_chat && reported.id == reporting.id;

    if reporting.chat.id != CONTROL_CHAT_ID {
        // Forward the relevant message(s) first, if they're not in control chat already.
        //
        // It's a good nicety, but it could fail: the messages may be protected from forwarding, or
        // an admin might have deleted them in just the right moment, or telegram goes funny again.
        // So, honestly, just ignore it failing.
        if same_message {
            // Just forward it.
            let _ = bot
                .forward_message(CONTROL_CHAT_ID, reporting.chat.id, reporting.id)
                .await;
        } else if same_chat {
            // In the same chat. Forward them both with this call.
            let _ = bot
                .forward_messages(
                    CONTROL_CHAT_ID,
                    reporting.chat.id,
                    [reported.id, reporting.id],
                )
                .await;
        } else {
            // Two different messages in two different chats. Forward individually.
            let _ = bot
                .forward_message(CONTROL_CHAT_ID, reported.chat.id, reported.id)
                .await;
            let _ = bot
                .forward_message(CONTROL_CHAT_ID, reporting.chat.id, reporting.id)
                .await;
        }
    }

    if teloxide_retry!(
        bot.send_message(CONTROL_CHAT_ID, &notify_text)
            .parse_mode(teloxide::types::ParseMode::Html)
            .await
    )
    .is_err()
    {
        log::error!("Failed notifying control chat of new marked sus link!\n{notify_text}");
    }

    Ok(())
}

/// Assuming `review_entry_id`, `sanitized_url` and `original_url` correspond to a review queue entry,
/// edits the message specified by `chat_id` and `message_id` into a new review keyboard.
pub async fn edit_message_into_a_review_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    review_entry_id: i64,
    sanitized_url: &SanitizedUrl,
    original_url: &Url,
    database: &Database,
) -> Result<(), RequestError> {
    let best_match = database
        .get_url_full(sanitized_url)
        .await
        .expect("Database died!");

    let best_match_url = best_match.as_ref().map(UrlInfoFull::sanitized_url);

    let (text, buttons) = ReviewCallbackData::produce_review_keyboard_text_buttons(
        review_entry_id,
        sanitized_url,
        original_url,
        best_match_url,
    );

    let edit_result = bot
        .edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(buttons)
        .disable_link_preview(true)
        .await;

    // If we get this error, that means that the message was modified to the
    // exact same thing as it was before. This means we're getting the same thing.
    if let Err(RequestError::Api(ApiError::MessageNotModified)) = edit_result {
        bot.edit_message_text(chat_id, message_id, "There are no more URLs to review.")
            .reply_markup(InlineKeyboardMarkup {
                inline_keyboard: Vec::new(),
            })
            .await?;
        return Ok(());
    }

    edit_result?;

    database
        .review_keyboard_made(chat_id, message_id, review_entry_id)
        .await
        .expect("Database died!");

    Ok(())
}

/// Fetches one URL from the review queue and edits the message specified by `chat_id` and
/// `message_id` into a new review keyboard.
pub async fn edit_message_into_a_new_review_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    database: &Database,
) -> Result<(), RequestError> {
    let Some((review_entry_id, sanitized_url, original_url)) =
        database.get_url_for_review().await.expect("Database died!")
    else {
        bot.edit_message_text(chat_id, message_id, "There are no more URLs to review.")
            .reply_markup(InlineKeyboardMarkup {
                inline_keyboard: Vec::new(),
            })
            .await?;
        return Ok(());
    };

    edit_message_into_a_review_keyboard(
        bot,
        chat_id,
        message_id,
        review_entry_id,
        &sanitized_url,
        &original_url,
        database,
    )
    .await
}

/// Assuming `review_entry_id`, `sanitized_url` and `original_url` correspond to a review queue entry,
/// creates a review keyboard in specified `chat_id`, optionally replying to a message.
pub async fn send_review_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    reply_to_message_id: Option<MessageId>,
    review_entry_id: i64,
    sanitized_url: &SanitizedUrl,
    original_url: &Url,
    database: &Database,
) -> Result<Message, RequestError> {
    let best_match = database
        .get_url_full(sanitized_url)
        .await
        .expect("Database died!");

    let best_match_url = best_match.as_ref().map(UrlInfoFull::sanitized_url);

    let (text, buttons) = ReviewCallbackData::produce_review_keyboard_text_buttons(
        review_entry_id,
        sanitized_url,
        original_url,
        best_match_url,
    );

    let mut request = bot
        .send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(buttons)
        .disable_link_preview(true);

    if let Some(reply_to) = reply_to_message_id {
        request = request.reply_to(reply_to);
    }

    let result = request.await;

    if let Ok(message) = result.as_ref() {
        database
            .review_keyboard_made(message.chat.id, message.id, review_entry_id)
            .await
            .expect("Database died!");
    }

    result
}

/// Fetches one URL from the review queue and creates a review keyboard in specified `chat_id`,
/// optionally replying to a message.
pub async fn send_new_review_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    reply_to_message_id: Option<MessageId>,
    database: &Database,
) -> Result<Message, RequestError> {
    if let Some((review_entry_id, sanitized_url, original_url)) =
        database.get_url_for_review().await.expect("Database died!")
    {
        send_review_keyboard(
            bot,
            chat_id,
            reply_to_message_id,
            review_entry_id,
            &sanitized_url,
            &original_url,
            database,
        )
        .await
    } else {
        let mut request = bot.send_message(chat_id, "There are no more URLs to review.");
        if let Some(reply_to) = reply_to_message_id {
            request = request.reply_to(reply_to);
        }

        request.await
    }
}

/// Assuming the `chat_id` and `message_id` point at a review keyboard with given `sanitized_url`,
/// discard it as handled by a user of name `handled_by_name`.
pub async fn discard_review_keyboard(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    handled_by_name: &str,
    designation: UrlDesignation,
    sanitized_url: &SanitizedUrl,
) -> Result<(), RequestError> {
    let text = format!("Handled by {handled_by_name}:\n<b>{designation}</b>\n{sanitized_url}",);

    bot.edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        })
        .disable_link_preview(true)
        .await?;

    Ok(())
}

/// Launches an ever-running loop that reminds people in the control chat every 24 hours about
/// unreviewed URLs.
pub async fn remind_about_reviews_spinloop(bot: Bot, database: std::sync::Weak<Database>) {
    use tokio::time::{sleep, Duration};
    loop {
        let Some(database) = database.upgrade() else {
            // No more database!
            return;
        };

        let review_count = match database.get_review_count().await {
            Ok(r) => r,
            Err(e) => {
                // Database died!
                log::error!("Database error! {e:?}");
                return;
            }
        };

        if review_count > 0 {
            // No biggie if this fails, honestly.
            let _ = teloxide_retry!(
                bot.send_message(
                    CONTROL_CHAT_ID,
                    format!(
                        concat!(
                            "There are {} URLs awaiting review. ",
                            "DM this bot /review to review."
                        ),
                        review_count
                    )
                )
                .await
            );
        }

        // Drop the upgraded database.
        drop(database);
        // Sleep for a day lol
        sleep(Duration::from_hours(24)).await;
    }
}
