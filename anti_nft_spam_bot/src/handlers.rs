use std::sync::Arc;

use teloxide::{
    prelude::*,
    types::{ChatMember, Me, MessageEntityKind, MessageEntityRef},
    RequestError,
};
use url::Url;

use crate::domains::{
    database::Database,
    types::{Domain, IsSpam},
};

/// Get a domain and a URL from this entity, if available.
fn get_entity_url_domain(entity: &MessageEntityRef) -> Option<(Url, Domain)> {
    let url = match entity.kind() {
        MessageEntityKind::Url => {
            if let Ok(url) = Domain::preparse(entity.text()) {
                url
            } else {
                // Does not parse as a URL anyway. Shouldn't happen, but eh.
                log::warn!("Received an imparsable URL: {}", entity.text());
                return None;
            }
        }
        MessageEntityKind::TextLink { url } => url.clone(),
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

pub async fn handle_message(
    bot: Bot,
    me: Me,
    message: Message,
    database: Arc<Database>,
) -> Result<(), RequestError> {
    // First check if it's a private message.
    if message.chat.is_private() {
        return handle_private_message(bot, me, message, database).await;
    }

    // Check if it has any links we want to ban.

    // Get message "entities".
    let Some(entities) = message
        .parse_entities()
        .or_else(|| message.parse_caption_entities())
    else {
        return Ok(());
    };

    let mut bad_links_present = false;

    // Scan all URLs in a message...
    for entity in &entities {
        let Some((url, domain)) = get_entity_url_domain(entity) else {
            continue;
        };
        log::debug!("Spotted URL with domain {}", domain);

        let Some(is_spam) = crate::domains::check(&database, &domain, &url).await else {
            continue;
        };

        if is_spam == IsSpam::Yes {
            bad_links_present = true;
            break;
        }
    }

    let should_delete = if bad_links_present {
        // oh no!
        // Check if this is an admin of the chat or not.

        let is_admin = {
            if let Some(user) = message.from() {
                let ChatMember { kind, .. } = bot.get_chat_member(message.chat.id, user.id).await?;
                kind.is_privileged()
            } else if let Some(chat) = message.sender_chat() {
                // If it's posted by the chat itself, it's probably an admin.
                chat.id == message.chat.id
            } else {
                false
            }
        };

        if is_admin {
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

                bot.send_message(
                    message.chat.id,
                    format!(
                        "Removed a message from {} containing a spam link.",
                        offending_user_name
                    ),
                )
                .await?;
            }
            Err(_) => {
                bot.send_message(
                    message.chat.id,
                    concat!(
                        "Tried to remove a message containing a spam link, but failed. ",
                        "Is this bot an admin with ability to remove messages?"
                    ),
                )
                .await?;
            }
        }
    } else {
        // It's not spam. Do the other things.
        gather_suspicion(&message, &database).await;
        parse_command(bot, me, message, database).await?;
    }

    Ok(())
}

/// Handler to intuit suspicious links based on them being replied to.
/// For example, if someone replies "spam" or "admin" to a message
/// with links, then those links may be spam. Send them to the database lol
async fn gather_suspicion(message: &Message, database: &Database) {
    let Some(text) = message.text() else {
        return;
    };

    let text = text.to_lowercase();

    let Some(replied_message) = message.reply_to_message() else {
        return;
    };

    if text.contains("spam") || text.contains("admin") || text.contains("begone") {
        // Replied-to message is sus. Mark its links.

        // Get message "entities".
        let Some(entities) = replied_message
            .parse_entities()
            .or_else(|| replied_message.parse_caption_entities())
        else {
            return;
        };

        for entity in &entities {
            let Some((url, domain)) = get_entity_url_domain(entity) else {
                continue;
            };

            log::debug!("Marking {} as sus...", domain);

            database
                .mark_domain_sus(&domain, Some(&url))
                .await
                .expect("Database died!");
        }
    }
}

async fn parse_command(
    _bot: Bot,
    me: Me,
    message: Message,
    _database: Arc<Database>,
) -> Result<(), RequestError> {
    // Get text of the message.
    let Some(text) = message.text() else {
        return Ok(());
    };
    // Check if it starts with "/", like how a command should.
    if !text.starts_with('/') {
        return Ok(());
    }
    // Get first word in the message, the command itself.
    let Some(command) = text.split_whitespace().next() else {
        return Ok(());
    };
    // Trim the bot's username from the command and convert to lowercase.
    let username = format!("@{}", me.username());
    let _command = command.trim_end_matches(username.as_str()).to_lowercase();
    // TODO: this lol

    //bot.send_message(message.chat.id, format!("Seen command: {}", command))
    //    .reply_to_message_id(message.id)
    //    .await?;

    Ok(())
}

pub async fn handle_private_message(
    bot: Bot,
    _me: Me,
    message: Message,
    _database: Arc<Database>,
) -> Result<(), RequestError> {
    // Telegram automatically trims preceding and following newlines, so this is fine.
    bot.send_message(
        message.chat.id,
        "
This bot is made to combat the currently ongoing wave of NFT spam experienced by chats with linked channels.

To use this bot, add it to a chat and give it administrator status with \"Remove messages\" permission.

No further setup is required. A message will be sent when spam is removed.

This bot may have more commands in the future, but not yet.",
    )
    .await?;
    Ok(())
}
