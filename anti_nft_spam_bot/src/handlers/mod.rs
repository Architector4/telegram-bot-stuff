use std::{borrow::Cow, sync::Arc};

use arch_bot_commons::{teloxide_retry, useful_methods::BotArchSendMsg};
use html_escape::encode_text;
use teloxide::{
    prelude::*,
    types::{BotCommand, ChatMember, Me, MessageEntityKind, MessageEntityRef, MessageId},
    ApiError, RequestError,
};
use url::Url;

use crate::{
    database::Database,
    parse_url_like_telegram, sender_name_prettyprint,
    types::{Domain, IsSpam, ReviewResponse},
    CONTROL_CHAT_ID,
};

pub mod reviews;
use self::reviews::handle_review_command;

/// Get a domain and a URL from this entity, if available.
fn get_entity_url_domain(entity: &MessageEntityRef) -> Option<(Url, Domain)> {
    let mut url = match entity.kind() {
        MessageEntityKind::Url | MessageEntityKind::Code | MessageEntityKind::Pre { .. } => {
            // Code and Pre because some spammers use monospace to make links unclickable
            // but undetectable.
            if let Ok(url) = parse_url_like_telegram(entity.text()) {
                url
            } else {
                if *entity.kind() == MessageEntityKind::Url {
                    // Does not parse as a URL anyway. Shouldn't happen, but eh.
                    log::warn!("Received an imparsable URL: {}", entity.text());
                }
                return None;
            }
        }
        MessageEntityKind::TextLink { url } => url.clone(),
        MessageEntityKind::Mention => {
            // Text will be like "@amogus"
            // Convert it into "https://t.me/amogus"
            let username = entity.text().trim_start_matches('@');
            let url_text = format!("https://t.me/{username}");

            if let Ok(url) = Url::parse(&url_text) {
                url
            } else {
                // Shouldn't happen, but eh.
                log::warn!(
                    "Failed to parse username \"{}\" converted to URL \"{}\"",
                    entity.text(),
                    url_text
                );
                return None;
            }
        }
        _ => {
            return None;
        }
    };
    // Some telegram spam (like telegram bots) use queries a lot,
    // especially referral links in spammed "games".
    // Strip those just from telegram URLs.
    if crate::spam_checker::is_telegram_url(&url) {
        url.set_query(None);
    }

    let Some(domain) = Domain::from_url(&url) else {
        // Does not have a domain. An IP address link?
        log::warn!("Received a URL without a domain: {}", entity.text());
        return None;
    };

    Some((url, domain))
}

/// Get a domain and a URL from this button, if available.
fn get_button_url_domain(button: &teloxide::types::InlineKeyboardButton) -> Option<(&Url, Domain)> {
    use teloxide::types::InlineKeyboardButtonKind as Kind;
    use teloxide::types::{LoginUrl, WebAppInfo};
    let url = match &button.kind {
        Kind::Url(url)
        | Kind::LoginUrl(LoginUrl { url, .. })
        | Kind::WebApp(WebAppInfo { url }) => url,
        Kind::CallbackData(..)
        | Kind::SwitchInlineQuery(..)
        | Kind::Pay(..)
        | Kind::SwitchInlineQueryCurrentChat(..)
        | Kind::CallbackGame(..) => return None,
    };

    let Some(domain) = Domain::from_url(url) else {
        // Does not have a domain. An IP address link?
        log::warn!("Received a URL in a button without a domain: {url}");
        return None;
    };

    Some((url, domain))
}

/// Returns `true` if this chat is private.
async fn is_sender_admin(bot: &Bot, message: &Message) -> Result<bool, RequestError> {
    if message.chat.is_private() {
        return Ok(true);
    }

    // First check if a chat sent this, i.e. an anonymous admin.
    // In such a case, "from()" returns @GroupAnonymousBot for backwards compatibility.
    let is_admin = if let Some(sender_chat) = message.sender_chat() {
        if sender_chat.id == message.chat.id {
            // If it's posted by the chat itself, it's probably an anonymous admin.
            true
        } else {
            // It may have been sent by the channel linked to this chat, then.
            // Check for that.
            let chat_full = bot.get_chat(message.chat.id).await?;

            chat_full.linked_chat_id() == Some(sender_chat.id.0)
        }
    } else if let Some(user) = message.from() {
        let ChatMember { kind, .. } = bot.get_chat_member(message.chat.id, user.id).await?;
        kind.is_privileged()
    } else {
        false
    };

    Ok(is_admin)
}

pub async fn handle_message(
    bot: Bot,
    me: Me,
    message: Message,
    database: Arc<Database>,
) -> Result<(), RequestError> {
    handle_message_inner(&bot, &me, &message, &database, false).await?;

    // Also handle the message it's a reply to.
    if let Some(replied_to) = message.reply_to_message() {
        handle_message_inner(&bot, &me, replied_to, &database, true).await?;
    }

    Ok(())
}

/// Set `is_replied_to` to true if this message is being handled in context of being an older
/// message that was replied to and is being checked again. If so, this handler will ignore
/// commands and such.
async fn handle_message_inner(
    bot: &Bot,
    me: &Me,
    message: &Message,
    database: &Arc<Database>,
    is_replied_to: bool,
) -> Result<(), RequestError> {
    if let Some(sender) = message.from() {
        if sender.id == me.id {
            // Ignore messages sent by ourselves.
            return Ok(());
        }
    }

    let is_edited = message.edit_date().is_some();

    // First check if it's a private message.
    if message.chat.is_private() {
        if !is_replied_to && !is_edited {
            // Will try handling commands at the end of this function too.
            if !handle_command(bot, me, message, database, None).await? {
                handle_private_message(bot, message).await?;
            }
        }
        return Ok(());
    }

    // Check if it has any links we want to ban.

    // Get message "entities".
    let entities = message
        .parse_entities()
        .or_else(|| message.parse_caption_entities())
        .unwrap_or_default();

    let mut bad_links_present = false;

    let mut sus_links_present: Vec<Url> = Vec::new();

    // Two loops below iterate over links, but need to do the same thing.
    // Rather than duplicate the code inside the loops, I'm defining a macro
    // that would do this for me.
    //
    // Ideally I'd just make an iterator over all entities and then inline keyboard
    // buttons that would do this for me, but ehhhhhhhhhhhhhhhhhh
    macro_rules! check_url {
        ($url: expr, $domain: expr, $loop_to_break: tt) => {
            log::debug!("Spotted URL with domain {}", $domain);

            let Some((is_spam, from_db)) =
                crate::spam_checker::check(database, $domain, $url).await
            else {
                continue;
            };

            if is_spam == IsSpam::Maybe {
                sus_links_present.push($url.clone());

                if !from_db {
                    // Checker above marked the URL as maybe spam. Notify the squad.
                    create_review_notify(bot, database, message, std::iter::once($url), true).await;
                }
            }

            if is_spam == IsSpam::Yes {
                bad_links_present = true;
                break $loop_to_break;
            }
        };
    }

    // Scan all URLs in a message...
    'thaloop: for entity in &entities {
        let Some((url, domain)) = get_entity_url_domain(entity) else {
            continue;
        };
        check_url!(&url, &domain, 'thaloop);
    }

    // If didn't find anything, also check all the buttons on the message for links.
    if !bad_links_present {
        if let Some(markup) = message.reply_markup() {
            'outer: for row in &markup.inline_keyboard {
                for button in row {
                    let Some((url, domain)) = get_button_url_domain(button) else {
                        continue;
                    };
                    check_url!(url, &domain, 'outer);
                }
            }
        }
    }

    let is_in_control_chat = message.chat.id == CONTROL_CHAT_ID;

    // We may need to check if the sender is an admin in two different places in this function.
    // If that happens, store the result determined first and reuse.
    // Check the result now, though.
    let sent_by_admin: Option<bool> =
        if !is_in_control_chat && (bad_links_present || !sus_links_present.is_empty()) {
            Some(is_sender_admin(bot, message).await?)
        } else {
            None
        };

    let should_delete = if bad_links_present && !is_in_control_chat {
        // oh no!
        if sent_by_admin == Some(true) {
            log::debug!("Skipping deleting message from an admin.");
            false
        } else {
            // Bad links and not an admin. Buh-bye!
            true
        }
    } else {
        // No bad links, shouldn't delete.
        false
    };

    let sender = sender_name_prettyprint(message, false);

    if should_delete {
        let chatid = message.chat.id;
        let messageid = message.id;
        delete_spam_message(bot, chatid, messageid, &sender, database).await?;
    } else {
        // It's (maybe?) not spam. Do the other things, if it's not an edit nor a replied-to message
        if !is_replied_to && !is_edited {
            if sent_by_admin != Some(true) {
                // Deal with known sus links...
                for url in sus_links_present {
                    let _ = database
                        .sus_link_sighted(message, Some(&sender), &url)
                        .await;
                }
            }

            // Deal with unknown sus links...
            gather_suspicion(bot, message, sent_by_admin, database).await?;

            if handle_command(bot, me, message, database, sent_by_admin).await? {
                return Ok(());
            }

            // And, for convenience sake...
            if is_in_control_chat && bad_links_present {
                bot.archsendmsg(
                    message.chat.id,
                    "Noticed a spam link in this message.",
                    message.id,
                )
                .await?;
            }
        }
    }

    Ok(())
}

pub async fn delete_spam_message(
    bot: &Bot,
    chatid: ChatId,
    messageid: MessageId,
    offending_user_name: &str,
    database: &Database,
) -> Result<(), RequestError> {
    // Try up to 3 times in case a fail happens lol
    for _ in 0..3 {
        match bot.delete_message(chatid, messageid).await {
            Ok(_) => {
                // Make a string, either a @username or full name,
                // describing the offending user.
                if !database
                    .get_hide_deletes(chatid)
                    .await
                    .expect("Database died!")
                {
                    bot.archsendmsg(
                        chatid,
                        format!(
                            "Removed a message from <code>{}</code> containing a spam link.",
                            encode_text(&offending_user_name)
                        )
                        .as_str(),
                        None,
                    )
                    .await?;
                }
                break;
            }
            Err(RequestError::Api(
                ApiError::MessageIdInvalid | ApiError::MessageToDeleteNotFound,
            )) => {
                // Someone else probably has already deleted it. That's fine.
                break;
            }
            Err(RequestError::Api(ApiError::MessageCantBeDeleted)) => {
                // No rights?
                bot.archsendmsg(
                    chatid,
                    concat!(
                        "Tried to remove a message containing a spam link, but failed. ",
                        "Is this bot an admin with ability to remove messages?\n\n",
                        "If so, this may also be a Telegram bug, and an admin ",
                        "has to remove the message manually."
                    ),
                    None,
                )
                .await?;
                break;
            }
            Err(_) => {
                // Random network error or whatever, possibly.
                // Try again by letting the loop roll.
            }
        }
    }

    Ok(())
}

/// Handler to intuit suspicious links based on them being replied to.
/// For example, if someone replies "spam" or "admin" to a message
/// with links, then those links may be spam. Send them to the database lol
async fn gather_suspicion(
    bot: &Bot,
    message: &Message,
    mut sent_by_admin: Option<bool>,
    database: &Database,
) -> Result<(), RequestError> {
    let Some(text) = message.text() else {
        return Ok(());
    };

    let text = text.to_lowercase();

    let mut replied_to_sent_by_admin = None;

    // Old check that captured links in a wider net.
    // Now our review queue is too big, so this is scaled
    // down to only handling the explicit /spam and /scam commands.

    //if text.contains("spam")
    //    || text.contains("scam")
    //    || text.contains("admin")
    //    || text.contains("begone")
    if text.starts_with("/spam") | text.starts_with("/scam") {
        // This or replied-to message may have sus links.

        // First, check and skip if this is a reply to a post by
        // the channel that is linked to this chat.
        // This is worth doing because such messages are sanctioned by
        // the chat's admins, and are most often just a misclick of
        // the blue /spam command in comments to channel posts.
        //
        // The check for above is accomplished by is_sender_admin function,
        // but there are other corner cases to consider too.

        'reject_from_admin: {
            let Some(reply_to) = message.reply_to_message() else {
                // This isn't a reply to anything.
                break 'reject_from_admin;
            };

            if message
                .from()
                .is_some_and(|x| Some(x.id) == reply_to.from().map(|x| x.id))
                || message
                    .sender_chat()
                    .is_some_and(|x| Some(x.id) == reply_to.sender_chat().map(|x| x.id))
            {
                // The sender is replying to themselves and knows what they're doing.
                break 'reject_from_admin;
            }

            if replied_to_sent_by_admin.is_none() {
                replied_to_sent_by_admin = Some(is_sender_admin(bot, reply_to).await?);
            }
            if replied_to_sent_by_admin != Some(true) {
                // The sender of the replied-to message isn't an admin.
                break 'reject_from_admin;
            };

            if sent_by_admin.is_none() {
                sent_by_admin = Some(is_sender_admin(bot, message).await?);
            };

            if sent_by_admin == Some(true) {
                // The sender of this message *is* an admin.
                break 'reject_from_admin;
            };

            // The two checks above will return true for private chats without extra
            // Telegram API queries.

            // This is an applicable situation.
            let response = concat!(
                "Sorry, but the message you're replying to is posted by an admin of this chat, ",
                "so it is ignored. If you believe it should be marked, ",
                "DM this bot with the command <code>/spam badlink.com</code> to submit it anyway."
            );
            bot.archsendmsg(message.chat.id, response, message.id)
                .await?;
            return Ok(());
        }

        // Find and tag the sus links.

        let mut had_links = false;

        let mut already_marked_sus_count = 0u32;
        let mut already_marked_spam_count = 0u32;
        let mut manually_reviewed_not_spam_count = 0u32;

        let mut links_marked: Vec<Cow<Url>> = Vec::new();

        let sendername = sender_name_prettyprint(message, false);

        macro_rules! marksus {
            ($offending_message: expr, $sent_by_admin: expr, $sendername: expr, $url: expr, $domain: expr) => {{
                let woot: Cow<Url> = $url;
                log::debug!("Marking {} and its domain as sus...", woot);

                had_links = true;

                let result = database
                    .mark_sus(woot.as_ref(), Some($domain))
                    .await
                    .expect("Database died!");

                {
                    use crate::types::MarkSusResult::*;

                    match result {
                        Marked => {
                            // Log it, if need be...
                            if $sent_by_admin != Some(true) {
                                database
                                    .sus_link_sighted($offending_message, Some($sendername), &woot)
                                    .await
                                    .expect("Database died!");
                            }
                            links_marked.push(woot);
                        }
                        AlreadyMarkedSus => already_marked_sus_count += 1,
                        AlreadyMarkedSpam => already_marked_spam_count += 1,
                        ManuallyReviewedNotSpam => manually_reviewed_not_spam_count += 1,
                    }
                }
            }};
        }

        if let Some(entities) = message
            .parse_entities()
            .or_else(|| message.parse_caption_entities())
        {
            for entity in &entities {
                //let Some((url, domain)) = get_entity_url_domain(entity) else {
                //    continue;
                //};
                match get_entity_url_domain(entity) {
                    Some((url, domain)) => {
                        if sent_by_admin.is_none() {
                            sent_by_admin = Some(is_sender_admin(bot, message).await?);
                        }
                        marksus!(
                            &message,
                            sent_by_admin,
                            &sendername,
                            Cow::Owned(url),
                            &domain
                        );
                    }
                    None => {
                        continue;
                    }
                }
            }
        };

        // Get replied-to message "entities", if any.
        if let Some(replied_message) = message.reply_to_message() {
            let replied_to_sender_name = sender_name_prettyprint(replied_message, false);
            if let Some(replied_entities) = replied_message
                .parse_entities()
                .or_else(|| replied_message.parse_caption_entities())
            {
                for entity in &replied_entities {
                    let Some((url, domain)) = get_entity_url_domain(entity) else {
                        continue;
                    };

                    if replied_to_sent_by_admin.is_none() {
                        replied_to_sent_by_admin =
                            Some(is_sender_admin(bot, replied_message).await?);
                    }

                    marksus!(
                        &replied_message,
                        replied_to_sent_by_admin,
                        &replied_to_sender_name,
                        Cow::Owned(url),
                        &domain
                    );
                }
            }

            // While we're here, check for links in buttons on the replied-to message.
            if let Some(markup) = replied_message.reply_markup() {
                for row in &markup.inline_keyboard {
                    for button in row {
                        let Some((url, domain)) = get_button_url_domain(button) else {
                            continue;
                        };

                        if replied_to_sent_by_admin.is_none() {
                            replied_to_sent_by_admin =
                                Some(is_sender_admin(bot, replied_message).await?);
                        }
                        marksus!(
                            &replied_message,
                            replied_to_sent_by_admin,
                            &replied_to_sender_name,
                            Cow::Borrowed(url),
                            &domain
                        );
                    }
                }
            }
        }

        // We assume there would be no buttons on the /spam message we're
        // working for that we need to check. That'd be kind of ridiculous lol

        let mut response;
        let marked = !links_marked.is_empty();
        let already_marked_sus = already_marked_sus_count > 0;
        let already_marked_spam = already_marked_spam_count > 0;
        let manually_reviewed_not_spam = manually_reviewed_not_spam_count > 0;

        //if text.starts_with("/spam") | text.starts_with("/scam")
        // This condition is moved to the top of the function now.
        {
            // That's the bot command, most likely. Users like indication that it does things.

            // Purposefully ambiguous message wording, where "this" both refers to the
            // message we're replying to and the message they replied to lol

            let response = if marked {
                "Thank you, links in this message will be reviewed for spam."
            } else if had_links {
                // Didn't mark anything as sus, but the message had links.

                response = String::from("Thank you, but the links in this message are ");

                if already_marked_sus {
                    response.push_str("already marked for review");
                    if already_marked_spam | manually_reviewed_not_spam {
                        response.push_str(", and some are ");
                    }
                }

                if already_marked_spam {
                    response.push_str("already marked for spam");
                    if manually_reviewed_not_spam {
                        response.push_str(", and some are ");
                    }
                }

                if manually_reviewed_not_spam {
                    response.push_str("manually reviewed and were determined to be not spam");
                }

                response.push('.');

                response.as_str()
            } else {
                // No links at all.
                concat!(
                    "Sorry, but I could not find any links in ",
                    "your message or the message you replied to, if any. ",
                    "This bot only blocks messages with usernames, links, and buttons with links."
                )
            };
            bot.archsendmsg(message.chat.id, response, message.id)
                .await?;
        }

        if marked {
            // We marked something. In this case, notify reviewers to review.
            create_review_notify(
                bot,
                database,
                message,
                links_marked.iter().map(|x| x.as_ref()),
                false,
            )
            .await;
        }
    }

    Ok(())
}

/// Returns `true` if a command was parsed and responded to.
async fn handle_command(
    bot: &Bot,
    me: &Me,
    message: &Message,
    database: &Arc<Database>,
    mut sent_by_admin: Option<bool>,
) -> Result<bool, RequestError> {
    if message.edit_date().is_some() {
        // Ignore message edits here.
        return Ok(false);
    }

    macro_rules! byadmin {
        () => {{
            if sent_by_admin.is_none() {
                sent_by_admin = Some(is_sender_admin(bot, message).await?);
            }
            sent_by_admin.unwrap()
        }};
    }
    macro_rules! respond {
        ($text:expr) => {
            bot.archsendmsg(message.chat.id, $text, message.id).await?;
        };
    }

    macro_rules! goodbye {
        ($text:expr) => {{
            respond!($text);
            return Ok(true);
        }};
    }
    let is_private = message.chat.is_private();

    let Some(text) = message.text() else {
        return Ok(false);
    };
    // Check if it starts with "/", like how a command should.
    if !text.starts_with('/') {
        return Ok(false);
    }
    // Get first word in the message, the command itself.
    let Some(command) = text.split_whitespace().next() else {
        return Ok(false);
    };

    let command_full_len = command.len();

    // Trim the bot's username from the command and convert to lowercase.
    let username = format!("@{}", me.username());
    let command = command.trim_end_matches(username.as_str()).to_lowercase();
    let _params = &text[command_full_len..].trim_start();

    let command_processed: bool = match command.as_str() {
        "/review" if is_private => handle_review_command(bot, message, database).await?,
        "/spam" | "/scam" if is_private => {
            // This is a private messages only handler. This is already run for public messages
            // differently, to catch non-command suspicions, so running it here would run it twice.
            gather_suspicion(bot, message, sent_by_admin, database).await?;
            true
        }
        "/hide_deletes" | "/show_deletes" => {
            if message.chat.is_private() || !byadmin!() {
                goodbye!("This command can only be used by admins in group chats.");
            }

            let new_state = command.as_str() == "/hide_deletes";

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

            goodbye!(response);
        }
        "/mark_not_spam" | "/mark_url_spam" | "/mark_domain_spam" => {
            // If it's not a private/control chat, or no sender, or they're not
            // in control chat, pretend we do not see it.
            if !(message.chat.is_private() || message.chat.id == CONTROL_CHAT_ID) {
                return Ok(false);
            }
            let Some(sender) = message.from() else {
                return Ok(false);
            };
            if !reviews::authenticate_control(bot, sender).await? {
                return Ok(false);
            }

            // Get message "entities".
            let Some(entities) = message
                .parse_entities()
                .or_else(|| message.parse_caption_entities())
            else {
                goodbye!("Please specify links. Replies don't count to avoid accidents.");
            };

            let mut response = String::new();
            let mut wrote_header = false;

            // Scan all URLs in the message...
            for entity in &entities {
                let Some((mut url, domain)) = get_entity_url_domain(entity) else {
                    continue;
                };

                match command.as_str() {
                    "/mark_not_spam" => {
                        let action = ReviewResponse::NotSpam(Some(domain), url);
                        reviews::apply_review_unverified(bot, sender, database, &action).await?;
                        // Get the URL back lol
                        url = action.deconstruct().unwrap().1;

                        if !wrote_header {
                            response.push_str("Marked as not spam:\n");
                            wrote_header = true;
                        }
                    }
                    "/mark_url_spam" => {
                        let action = ReviewResponse::UrlSpam(Some(domain), url);
                        reviews::apply_review_unverified(bot, sender, database, &action).await?;
                        // Get the URL back lol
                        url = action.deconstruct().unwrap().1;

                        if !wrote_header {
                            response.push_str("Marked these URLs as spam:\n");
                            wrote_header = true;
                        }
                    }
                    "/mark_domain_spam" => {
                        let action = ReviewResponse::DomainSpam(domain, url);
                        reviews::apply_review_unverified(bot, sender, database, &action).await?;
                        // Get the URL back lol
                        url = action.deconstruct().unwrap().1;

                        if !wrote_header {
                            response.push_str("Marked domains of these URLs as spam:\n");
                            wrote_header = true;
                        }
                    }
                    _ => unreachable!(),
                }

                response.push_str(url.as_str());
                response.push('\n');
            }

            if response.is_empty() {
                goodbye!("Please specify links. Replies don't count to avoid accidents.");
            }

            goodbye!(response.as_str());
        }
        // Any kind of "/start", "/help" commands would yield false and
        // hence cause the help message to be printed if this is a private chat.
        // See definition of handle_private_message.
        _ => false,
    };

    Ok(command_processed)
}

pub fn generate_bot_commands() -> Vec<BotCommand> {
    vec![
        BotCommand::new("/hide_deletes", "Hide spam deletion notification messages."),
        BotCommand::new(
            "/show_deletes",
            "Don't hide spam deletion notification messages.",
        ),
        BotCommand::new("/spam", "Mark links in a message for review as spam."),
    ]
}

pub async fn handle_private_message(bot: &Bot, message: &Message) -> Result<(), RequestError> {
    if message.edit_date().is_some() {
        // Ignore message edits here.
        return Ok(());
    }
    // Nothing much to do here lol

    bot.send_message(
        message.chat.id,
        "
This bot is made to combat the currently ongoing wave of NFT spam experienced by chats with linked channels.

To use this bot, add it to a chat and give it administrator status with \"Remove messages\" permission.

No further setup is required. A message will be sent when spam is removed.

For available commands, type / into the message text box below and see the previews.

If you're in the group for volunteers to manually review chats, you can also use commands here in private chat:

/mark_not_spam, /mark_url_spam and /mark_domain_spam"
    )
    .await?;
    Ok(())
}

/// Set `automatic` to true if this review notify was automatically
/// decided by the bot.
pub async fn create_review_notify(
    bot: &Bot,
    database: &Database,
    message: &Message,
    links_marked: impl Iterator<Item = &Url>,
    automatic: bool,
) {
    let to_review = database.get_review_count().await.expect("Database died!");
    let username_string: String;
    let username: &str = if automatic {
        "automatic check"
    } else {
        username_string = sender_name_prettyprint(message, true);
        &username_string
    };

    let chatname = if let Some(username) = message.chat.username() {
        format!("@{} (chatid <code>{}</code>)", username, message.chat.id)
    } else if let Some(title) = message.chat.title() {
        format!("{} (chatid <code>{}</code>)", title, message.chat.id)
    } else {
        format!("Unknown (chatid <code>{}</code>)", message.chat.id)
    };

    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    // Also create a keyboard for review buttons...
    let keyboard = InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback(
                "Mark URLs spam".to_string(),
                "URL_SPAM derive".to_string(),
            ),
            InlineKeyboardButton::callback(
                "Mark DOMAINS spam".to_string(),
                "DOMAIN_SPAM derive".to_string(),
            ),
        ],
        vec![InlineKeyboardButton::callback(
            "Not spam".to_string(),
            "NOT_SPAM derive".to_string(),
        )],
    ]);

    let mut notify_text =
        format!("New link(s) were added to review pool by {username} in {chatname}:\n");

    use std::fmt::Write;

    for url in links_marked {
        let _ = writeln!(notify_text, "URL: {url}\n");
    }

    let _ = writeln!(notify_text, "There are {to_review} links to review.");

    if teloxide_retry!(
        bot.send_message(CONTROL_CHAT_ID, &notify_text)
            .parse_mode(teloxide::types::ParseMode::Html)
            .reply_markup(keyboard.clone())
            .disable_web_page_preview(true)
            .await
    )
    .is_err()
    {
        log::error!("Failed notifying control chat of new marked sus link!\n{notify_text}");
    };
}
