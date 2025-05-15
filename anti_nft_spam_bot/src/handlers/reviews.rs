use std::fmt::Write;
use std::sync::Arc;

use teloxide::{
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters, SendMessageSetters},
    requests::Requester,
    types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, User},
    ApiError, Bot, RequestError,
};

use crate::{
    database::Database,
    types::{IsSpam, ReviewResponse},
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
        let name = if let Some(username) = &user.username {
            format!("@{}", username)
        } else {
            user.full_name()
        };

        log::info!(
            "Unauthorized user trying to access reviews: {} (userid {})",
            name,
            user.id
        );
        // Not a member.
        // Now, facts:
        // 1. This function will only be run in context of a private chat.
        //
        // 2. Teloxide intentionally processes messages from one chat
        //    not-concurrently; that is, if we delay now, this will delay
        //    processing all following direct messages sent by that person
        //    to this bot.
        //
        // 3. There is no pertinent reason to DM this bot other than to get
        //    the help message.
        //
        // 4. If a user is sending DMs to this bot, that means that they
        //    have already sent `/start`, and hence have already seen the
        //    help message.
        //
        // 5. Therefore, there is no harm to be done by delaying users
        //    not legible for reviews for DMs.
        //
        // 6. Bad actors may want to try and spam this bot `/review` to
        //    cause it to send the above API request many times and in turn
        //    get rate limited by telegram.
        //
        // With that in mind, delay this user from accessing this bot for 5 seconds.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    Ok(control)
}

/// Returns true if the command was processed, or false if it was ignored.
pub async fn handle_review_command(
    bot: &Bot,
    message: &Message,
    database: &Database,
) -> Result<bool, RequestError> {
    // Check if it's sent by a user. Otherwise, we don't care.
    let Some(user) = message.from() else {
        return Ok(false);
    };

    // Check if that user is anyone in the control chat...

    if !authenticate_control(bot, user).await? {
        return Ok(false);
    }

    // Spawn a review keyboard.

    let message = bot
        .send_message(message.chat.id, "Loading review keyboard...")
        .reply_to_message_id(message.id)
        .await?;

    edit_message_into_a_review(bot, database, &message).await?;

    Ok(true)
}

async fn edit_message_into_a_review(
    bot: &Bot,
    database: &Database,
    message: &Message,
) -> Result<(), RequestError> {
    // Telegram's inline keyboards only support up to 128
    // bytes long payload data. We can't hope to store the full
    // URL in that, so we store a table name, row ID, and hash of the URL
    // instead.
    let Some((url, table_name, rowid, is_spam)) =
        database.get_url_for_review().await.expect("Database died!")
    else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            "There are no more URLs to review.",
        )
        .reply_markup(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        })
        .await?;
        return Ok(());
    };

    let url_hash = crc32fast::hash(url.as_str().as_bytes());

    let title = match is_spam {
        IsSpam::Maybe => "<b>REVIEW:</b>\n\n",
        IsSpam::No | IsSpam::Yes => concat!(
            "<b>REHASHING: </b>\n",
            "There are no more URLs to review right now, ",
            "so existing entries are shown to weed out ",
            "any potential false positives.\n\n"
        ),
    };

    let considered = match is_spam {
        IsSpam::No => concat!(
            "This URL is currently <b>NOT</b> considered as spam, ",
            "but is presented for review in case it's wrong.\n\n"
        ),
        IsSpam::Yes => concat!(
            "This URL is currently <b>considered as spam</b>, ",
            "but is presented for review in case it's a false positive.\n\n"
        ),
        IsSpam::Maybe => "",
    };

    let text = format!("{}{}{}\n\nWhat is spam here?", title, considered, url);

    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                "Just the URL".to_string(),
                format!("URL_SPAM {} {} {}", table_name, rowid, url_hash),
            ),
            InlineKeyboardButton::callback(
                "Entire DOMAIN".to_string(),
                format!("DOMAIN_SPAM {} {} {}", table_name, rowid, url_hash),
            ),
        ],
        vec![
            InlineKeyboardButton::callback(
                "Not spam".to_string(),
                format!("NOT_SPAM {} {} {}", table_name, rowid, url_hash),
            ),
            InlineKeyboardButton::callback("Skip".to_string(), "SKIP".to_string()),
        ],
    ]);

    let edit_result = bot
        .edit_message_text(message.chat.id, message.id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(keyboard)
        .await;

    // If we get this error, that means that the message was modified to the
    // exact same thing as it was before. This means we're getting the same thing.
    if let Err(RequestError::Api(ApiError::MessageNotModified)) = edit_result {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            "There are no more URLs to review.",
        )
        .reply_markup(InlineKeyboardMarkup {
            inline_keyboard: Vec::new(),
        })
        .await?;
        return Ok(());
    };

    edit_result?;
    Ok(())
}

pub async fn parse_callback_query(
    bot: Bot,
    query: CallbackQuery,
    db: Arc<Database>,
) -> Result<(), RequestError> {
    macro_rules! goodbye {
        ($text:expr) => {
            bot.answer_callback_query(query.id).text($text).await?;
            return Ok(());
        };
        () => {
            bot.answer_callback_query(query.id).await?;
            return Ok(());
        };
    }

    let Some(query_data) = query.data else {
        goodbye!("No query data.");
    };

    let user = query.from;

    let responses =
        match ReviewResponse::from_str(query_data.as_str(), &db, query.message.as_ref()).await {
            Ok(r) => r,
            Err(e) => {
                goodbye!(&format!("Invalid query data: {}", e));
            }
        };

    if responses.is_empty() {
        goodbye!("Nothing to mark here...???");
    }

    for response in &responses {
        if !apply_review(&bot, &user, &db, response).await? {
            goodbye!("Access denied.");
        }
    }

    let Some(message) = query.message else {
        // May happen if the message is too old
        goodbye!("Review taken. Please send /review to perform more reviews.");
    };

    // Avoid editing the message into reviews it's not in private i.e. in work chat
    if message.chat.is_private() {
        edit_message_into_a_review(&bot, &db, &message).await?;
    } else {
        // It's a notification about newly marked URLs that was just reviewed on.
        // Edit it to get rid of the buttons and stuff.

        let name = if let Some(username) = &user.username {
            format!("@{}", username)
        } else {
            user.full_name()
        };

        let mut text = format!("Handled by {} (userid {}):\n", name, user.id);
        for response in &responses {
            let _ = writeln!(&mut text, "{}", response);
        }

        if let Some(msgtext) = message.text() {
            let _ = write!(&mut text, "\nOriginal message text:\n{}", msgtext);
        }

        bot.edit_message_text(message.chat.id, message.id, text)
            .disable_web_page_preview(true)
            .await?;
    }
    goodbye!();
}

/// Apply this review response as coming from this user.
///
/// Returns true if succeeded, false if the user is not in control chat.
///
/// If `verify_user` is set to `false`, it will always return true.
pub async fn apply_review(
    bot: &Bot,
    user: &User,
    db: &Database,
    response: &ReviewResponse,
) -> Result<bool, RequestError> {
    if !authenticate_control(bot, user).await? {
        return Ok(false);
    }

    apply_review_unverified(bot, user, db, response).await?;
    Ok(true)
}

/// Apply this review response as coming from this user.
///
/// Will not check if this user actually is in control chat.
pub async fn apply_review_unverified(
    bot: &Bot,
    user: &User,
    db: &Database,
    response: &ReviewResponse,
) -> Result<(), RequestError> {
    // See if it should be written into the log...
    let should_be_logged = response
        .conflicts_with_db(db)
        .await
        .expect("Database died!");

    // Ingest it into the database...
    db.read_review_response(response)
        .await
        .expect("Database died!");

    // Write it to the log...
    if should_be_logged {
        // Something wasn't marked as spam, but now will be.
        // This warrants logging.

        let name = if let Some(username) = &user.username {
            format!("@{}", username)
        } else {
            user.full_name()
        };

        let log_message = format!("{} (userid {})\n{}", name, user.id, response);

        bot.send_message(REVIEW_LOG_CHANNEL_ID, log_message)
            .disable_web_page_preview(true)
            .await?;
    }
    Ok(())
}
