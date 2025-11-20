use teloxide::{
    prelude::Requester,
    types::{Chat, ChatMember, InlineKeyboardButton, Message, MessageEntityRef, User},
    Bot, RequestError,
};
use url::Url;

use crate::{
    database::Database, sanitized_url::SanitizedUrl, spam_checker::is_url_spam, CONTROL_CHAT_ID,
};

/// Try to parse a string as a [`Url`] in a way that telegram parses it,
/// with allowing an implicit `https://` prefix, or as a username.
///
/// # Errors
/// Errors if it fails to parse either way.
pub fn parse_url_like_telegram(string: &str) -> Result<Url, url::ParseError> {
    if let Some(username) = string.strip_prefix('@') {
        // Probably a username like "@amogus"
        // Convert to a format like "https://t.me/amogus" then parse
        return Url::parse(&format!("https://t.me/{username}"));
    }

    match Url::parse(string) {
        Ok(url) => Ok(url),
        Err(e @ url::ParseError::RelativeUrlWithoutBase) => {
            // Try prepending https:// to it
            if let Ok(url) = Url::parse(&format!("https://{string}")) {
                Ok(url)
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

/// Tries to print the user in the prettiest way possible, with either `@username` or full name
/// that hopefully links to the user if `with_link_formatting` is `true`. Optionally allows including user ID.
#[must_use]
pub fn user_name_prettyprint(user: &User, with_id: bool, with_link_formatting: bool) -> String {
    let mut name = {
        if let Some(username) = &user.username {
            format!("@{username}")
        } else {
            if with_link_formatting {
                let mut full_name =
                    format!("<a href=\"tg://user?id={}\">{}", user.id, user.first_name);

                if let Some(last_name) = &user.last_name {
                    full_name.push(' ');
                    full_name.push_str(last_name);
                }

                full_name.push_str("</a>");

                full_name
            } else {
                user.full_name()
            }
        }
    };

    if with_id {
        use std::fmt::Write;
        write!(name, " (userid {})", user.id).expect("Writing to a String never fails");
    }

    name
}

/// Tries to print the chat name in the prettiest way possible, with either `@username` or chat
/// title or full name.
#[must_use]
pub fn chat_name_prettyprint(chat: &Chat, with_id: bool) -> String {
    let mut name = if let Some(username) = chat.username() {
        format!("@{username}")
    } else if let Some(title) = chat.title() {
        title.to_string()
    } else if let Some(first_name) = chat.first_name() {
        let mut full_name = first_name.to_string();

        if let Some(last_name) = chat.last_name() {
            full_name.push(' ');
            full_name.push_str(last_name);
        }
        full_name
    } else {
        // Shouldn't happen, but eh.
        "a private chat".to_string()
    };

    if with_id {
        use std::fmt::Write;
        write!(name, " (chatid {})", chat.id).expect("Writing to a String never fails");
    }

    name
}

/// Tries to print the name of the sender of this message, using either [`user_name_prettyprint`]
/// or [`chat_name_prettyprint`].
#[must_use]
pub fn sender_name_prettyprint(message: &Message, with_id: bool) -> String {
    if let Some(chat) = &message.sender_chat {
        chat_name_prettyprint(chat, with_id)
    } else if let Some(user) = &message.from {
        // Assume we want link formatting only if we also want the ID.
        user_name_prettyprint(user, with_id, with_id)
    } else {
        // Shouldn't happen, but eh.
        "a private sender".to_string()
    }
}

/// Get a URL from this message entity, if available.
#[must_use]
pub fn get_entity_url(entity: &MessageEntityRef) -> Option<(SanitizedUrl, Url)> {
    use teloxide::types::MessageEntityKind as Kind;

    match entity.kind() {
        Kind::Url | Kind::Code | Kind::Pre { .. } | Kind::Mention => {
            // Code and Pre because some spammers use monospace to make links clickable but
            // undetectable. Mentions are links too.
            SanitizedUrl::from_str_with_original(entity.text())
        }
        Kind::TextLink { url } => SanitizedUrl::from_url_with_original(url.clone()),
        Kind::TextMention { user } => user
            .username
            .as_ref()
            .map(|u| format!("https://t.me/{u}"))
            .and_then(|s| SanitizedUrl::from_str_with_original(&s)),
        _ => None,
    }
}

/// Get a URL from this button, if available.
#[must_use]
pub fn get_button_url(button: &InlineKeyboardButton) -> Option<(SanitizedUrl, Url)> {
    use teloxide::types::InlineKeyboardButtonKind as Kind;
    use teloxide::types::{CopyTextButton, LoginUrl, SwitchInlineQueryChosenChat, WebAppInfo};

    match &button.kind {
        Kind::Url(url)
        | Kind::LoginUrl(LoginUrl { url, .. })
        | Kind::WebApp(WebAppInfo { url }) => SanitizedUrl::from_url_with_original(url.clone()),
        Kind::SwitchInlineQuery(string)
        | Kind::CopyText(CopyTextButton { text: string })
        | Kind::SwitchInlineQueryCurrentChat(string) => {
            SanitizedUrl::from_str_with_original(string)
        }
        Kind::SwitchInlineQueryChosenChat(SwitchInlineQueryChosenChat {
            query: opt_string,
            ..
        }) => opt_string
            .as_ref()
            .and_then(|s| SanitizedUrl::from_str_with_original(s)),
        _ => None,
    }
}

/// Checks if the sender of this message is an admin. Returns `true` if this is a private chat
/// between the bot and the user.
pub async fn is_sender_admin(bot: &Bot, message: &Message) -> Result<bool, RequestError> {
    if message.chat.is_private() {
        return Ok(true);
    }

    if message.chat.id == CONTROL_CHAT_ID {
        // Everyone in the control chat is an "admin".
        return Ok(true);
    }

    // check if a chat sent this, i.e. an anonymous admin.
    // In such a case, "from()" returns @GroupAnonymousBot for backwards compatibility.
    let is_admin = if let Some(sender_chat) = &message.sender_chat {
        if sender_chat.id == message.chat.id {
            // If it's posted by the chat itself, it's probably an anonymous admin.
            true
        } else {
            // It may have been sent by the channel linked to this chat, then.
            // Check for that.
            let chat_full = bot.get_chat(message.chat.id).await?;

            chat_full.linked_chat_id() == Some(sender_chat.id.0)
        }
    } else if let Some(user) = &message.from {
        let ChatMember { kind, .. } = bot.get_chat_member(message.chat.id, user.id).await?;
        kind.is_privileged()
    } else {
        false
    };

    Ok(is_admin)
}

/// Convenience function around [`is_sender_admin`] to use with a variable that holds the result if
/// it was already computed prior.
pub async fn is_sender_admin_with_cache(
    bot: &Bot,
    message: &Message,
    cache: &mut Option<bool>,
) -> Result<bool, RequestError> {
    if let Some(cached) = cache {
        return Ok(*cached);
    }

    let result = is_sender_admin(bot, message).await?;
    *cache = Some(result);
    Ok(result)
}

/// Iterate over all links that incriminate this message. This includes message entities, buttons,
/// and, if the message is a reply to a message in another chat, all of the above of that message
/// too.
pub fn iterate_over_all_links(
    message: &Message,
) -> impl Iterator<Item = (SanitizedUrl, Url)> + Send + Sync + '_ {
    macro_rules! the_unholy_links_iterator {
        ($message: expr) => {
            $message
                .parse_entities()
                .or_else(|| message.parse_caption_entities())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|x| get_entity_url(&x))
                .chain(
                    message
                        .reply_markup()
                        .map(|x| &x.inline_keyboard)
                        .into_iter()
                        .flat_map(|x| x.iter())
                        .flat_map(|x| x.iter())
                        .filter_map(get_button_url),
                )
        };
    }

    // This message *itself* might not have bad links, but it may be a reply across chats
    // to a message that does, with a plea to click on the reply. Handle that too.
    the_unholy_links_iterator!(message).chain(
        message
            .reply_to_message()
            .filter(|reply_to| reply_to.chat.id != message.chat.id)
            .into_iter()
            .flat_map(|reply_to| the_unholy_links_iterator!(reply_to)),
    )
}

/// Returns true if any of the links in this message are spam, or if it's a reply to a message in
/// another chat that does.
pub async fn does_message_have_spam_links(message: &Message, database: &Database) -> bool {
    for (sanitized_url, _original_url) in iterate_over_all_links(message) {
        if is_url_spam(database, &sanitized_url).await {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    #[test]
    fn parsing_url_like_telegram() {
        let url = parse_url_like_telegram("https://example.com/").unwrap();
        assert_eq!(url.as_str(), "https://example.com/");
        let url = parse_url_like_telegram("example.com").unwrap();
        assert_eq!(url.as_str(), "https://example.com/");
        let url = parse_url_like_telegram("@amogus").unwrap();
        assert_eq!(url.as_str(), "https://t.me/amogus");
    }
}
