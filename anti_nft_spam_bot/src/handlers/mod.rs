use std::sync::Arc;

use arch_bot_commons::useful_methods::BotArchSendMsg;
use html_escape::encode_text;
use teloxide::{
    prelude::*,
    types::{BotCommand, ChatMember, Me, MessageEntityKind, MessageEntityRef},
    ApiError, RequestError,
};
use url::Url;

use crate::{
    database::Database,
    parse_url_like_telegram,
    types::{Domain, IsSpam, ReviewResponse},
};

pub mod reviews;
use self::reviews::handle_review_command;

/// Get a domain and a URL from this entity, if available.
fn get_entity_url_domain(entity: &MessageEntityRef) -> Option<(Url, Domain)> {
    let url = match entity.kind() {
        MessageEntityKind::Url => {
            if let Ok(url) = parse_url_like_telegram(entity.text()) {
                url
            } else {
                // Does not parse as a URL anyway. Shouldn't happen, but eh.
                log::warn!("Received an imparsable URL: {}", entity.text());
                return None;
            }
        }
        MessageEntityKind::TextLink { url } => url.clone(),
        MessageEntityKind::Mention => {
            // Text will be like "@amogus"
            // Convert it into "https://t.me/amogus"
            let username = entity.text().trim_start_matches('@');
            let url_text = format!("https://t.me/{}", username);

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

    let Some(domain) = Domain::from_url(&url) else {
        // Does not have a domain. An IP address link?
        log::warn!("Received a URL without a domain: {}", entity.text());
        return None;
    };

    Some((url, domain))
}

/// Returns `true` if this chat is private.
async fn is_sender_admin(bot: &Bot, message: &Message) -> Result<bool, RequestError> {
    if message.chat.is_private() {
        return Ok(true);
    }
    let is_admin = if let Some(user) = message.from() {
        let ChatMember { kind, .. } = bot.get_chat_member(message.chat.id, user.id).await?;
        kind.is_privileged()
    } else if let Some(chat) = message.sender_chat() {
        // If it's posted by the chat itself, it's probably an admin.
        chat.id == message.chat.id
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

    // Scan all URLs in a message...
    for entity in &entities {
        let Some((url, domain)) = get_entity_url_domain(entity) else {
            continue;
        };
        log::debug!("Spotted URL with domain {}", domain);

        let Some(is_spam) = crate::spam_checker::check(database, &domain, &url).await else {
            continue;
        };

        if is_spam == IsSpam::Yes {
            bad_links_present = true;
            break;
        }
    }

    // Check all the buttons on the message for links.
    if let Some(markup) = message.reply_markup() {
        for row in &markup.inline_keyboard {
            for button in row {
                use teloxide::types::InlineKeyboardButtonKind as Kind;
                use teloxide::types::{LoginUrl, WebAppInfo};
                match &button.kind {
                    Kind::Url(url)
                    | Kind::LoginUrl(LoginUrl { url, .. })
                    | Kind::WebApp(WebAppInfo { url }) => {
                        let Some(domain) = Domain::from_url(url) else {
                            continue;
                        };

                        log::debug!("Spotted button URL with domain {}", domain);

                        let Some(is_spam) =
                            crate::spam_checker::check(database, &domain, url).await
                        else {
                            continue;
                        };

                        if is_spam == IsSpam::Yes {
                            bad_links_present = true;
                            break;
                        }
                    }
                    Kind::CallbackData(..)
                    | Kind::SwitchInlineQuery(..)
                    | Kind::Pay(..)
                    | Kind::SwitchInlineQueryCurrentChat(..)
                    | Kind::CallbackGame(..) => {}
                }
            }
        }
    }

    // We may need to check if the sender is an admin in two different places in this function.
    // If that happens, store the result determined first and reuse.
    let mut sent_by_admin: Option<bool> = None;

    let should_delete = if bad_links_present {
        // oh no!
        // Check if this is an admin of the chat or not.

        sent_by_admin = Some(is_sender_admin(bot, message).await?);

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

    if should_delete {
        // Try up to 3 times in case a fail happens lol
        for _ in 0..3 {
            match bot.delete_message(message.chat.id, message.id).await {
                Ok(_) => {
                    // Make a string, either a @username or full name,
                    // describing the offending user.
                    let offending_user_name = {
                        if let Some(user) = message.from() {
                            if let Some(username) = &user.username {
                                format!("@{}", username)
                            } else {
                                user.full_name()
                            }
                        } else if let Some(chat) = message.sender_chat() {
                            if let Some(username) = chat.username() {
                                format!("@{}", username)
                            } else if let Some(title) = chat.title() {
                                title.to_string()
                            } else {
                                // Shouldn't happen, but eh.
                                "a private user".to_string()
                            }
                        } else {
                            // Shouldn't happen either, but eh.
                            "a private user".to_string()
                        }
                    };

                    if !database
                        .get_hide_deletes(message.chat.id)
                        .await
                        .expect("Database died!")
                    {
                        bot.archsendmsg(
                            message.chat.id,
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
                        message.chat.id,
                        concat!(
                            "Tried to remove a message containing a spam link, but failed. ",
                            "Is this bot an admin with ability to remove messages?"
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
    } else {
        // It's not spam. Do the other things, if it's not an edit nor a replied-to message
        if !is_replied_to && !is_edited {
            gather_suspicion(bot, message, database).await?;

            if handle_command(bot, me, message, database, sent_by_admin).await? {
                return Ok(());
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
    database: &Database,
) -> Result<(), RequestError> {
    let Some(text) = message.text() else {
        return Ok(());
    };

    let text = text.to_lowercase();

    if text.contains("spam")
        || text.contains("scam")
        || text.contains("admin")
        || text.contains("begone")
    {
        // This or replied-to message has sus links.
        // Tag them.

        // Get this message "entities".
        let Some(mut entities) = message
            .parse_entities()
            .or_else(|| message.parse_caption_entities())
        else {
            return Ok(());
        };

        // Get replied-to message "entities", if any.
        if let Some(replied_message) = message.reply_to_message() {
            if let Some(replied_entities) = replied_message
                .parse_entities()
                .or_else(|| replied_message.parse_caption_entities())
            {
                entities.extend(replied_entities);
            }
        }

        let mut had_links = false;

        let mut marked_count = 0u32;
        let mut already_marked_sus_count = 0u32;
        let mut already_marked_spam_count = 0u32;
        let mut manually_reviewed_not_spam_count = 0u32;

        for entity in &entities {
            let Some((url, domain)) = get_entity_url_domain(entity) else {
                continue;
            };

            log::debug!("Marking {} and its domain as sus...", url);

            had_links = true;

            let result = database
                .mark_sus(&url, Some(&domain))
                .await
                .expect("Database died!");

            {
                use crate::types::MarkSusResult::*;

                match result {
                    Marked => marked_count += 1,
                    AlreadyMarkedSus => already_marked_sus_count += 1,
                    AlreadyMarkedSpam => already_marked_spam_count += 1,
                    ManuallyReviewedNotSpam => manually_reviewed_not_spam_count += 1,
                }
            }
        }

        let mut response;
        let marked = marked_count > 0;
        let already_marked_sus = already_marked_sus_count > 0;
        let already_marked_spam = already_marked_spam_count > 0;
        let manually_reviewed_not_spam = manually_reviewed_not_spam_count > 0;

        if text.starts_with("/spam") | text.starts_with("/scam") {
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
                "Sorry, but I could not find any links."
            };
            bot.archsendmsg(message.chat.id, response, message.id)
                .await?;
        }
    }

    Ok(())
}

/// Returns `true` if a command was parsed and responded to.
async fn handle_command(
    bot: &Bot,
    me: &Me,
    message: &Message,
    database: &Database,
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
            gather_suspicion(bot, message, database).await?;
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
            // If it's not a private chat, or no sender,or they're not
            // in control chat, pretend we do not see it.
            if !message.chat.is_private() {
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
