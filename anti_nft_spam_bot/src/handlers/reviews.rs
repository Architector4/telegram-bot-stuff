use teloxide::{
    payloads::SendMessageSetters,
    requests::Requester,
    types::{Me, Message},
    Bot, RequestError,
};

use crate::{database::Database, CONTROL_CHAT_ID};

/// Returns true if the command was processed, or false if it was ignored.
pub async fn handle_review_command(
    bot: &Bot,
    _me: &Me,
    message: &Message,
    database: &Database,
) -> Result<bool, RequestError> {
    // Check if it's sent by a user. Otherwise, we don't care.
    let Some(user) = message.from() else {
        return Ok(false);
    };

    // Check if that user is anyone in the control chat...
    let member = bot.get_chat_member(CONTROL_CHAT_ID, user.id).await?;
    if !member.is_present() {
        // Not a member.
        // Now, facts:
        // 1. This function will only be run in context of a private chat.
        // 2. Teloxide intentionally processes messages from one chat
        //    not-concurrently; that is, if we delay now, this will delay
        //    processing all following direct messages sent by that person
        //    to this bot.
        // 3. There is no other reason to DM this bot other than to get
        //    the help message and for these reviews.
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
    let Some(review) = database.get_url_for_review().await.expect("Database died!") else {
        bot.edit_message_text(message.chat.id, message.id, "There are URLs to review.")
            .await?;
        return Ok(());
    };

    let text = format!("Got URL {}\nand IsSpam {:?}", review.0, review.1);

    bot.edit_message_text(message.chat.id, message.id, text)
        .await?;

    Ok(())
}
