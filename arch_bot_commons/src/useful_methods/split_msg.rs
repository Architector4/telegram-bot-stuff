use std::time::Duration;

use futures::Future;
use teloxide::{
    payloads::SendMessageSetters,
    requests::Requester,
    types::{Message, MessageId, Recipient},
    Bot, RequestError,
};

pub trait BotArchSendMsg {
    /// Opinionated method to send a message, with HTML markup,
    /// and retries due to flood waiting or any other issues.
    /// Also splits the message into many if it's longer than
    /// the character limit.
    fn archsendmsg<'a>(
        &'a self,
        to_where: impl Into<Recipient> + Send,
        text: impl Into<&'a str> + Send,
        reply_to: impl Into<Option<MessageId>> + Send,
    ) -> impl Future<Output = Result<Vec<Message>, RequestError>> + Send;
}

impl BotArchSendMsg for Bot {
    async fn archsendmsg<'a>(
        &'a self,
        to_where: impl Into<Recipient> + Send,
        text: impl Into<&'a str> + Send,
        reply_to: impl Into<Option<MessageId>> + Send,
    ) -> Result<Vec<Message>, RequestError> {
        let to_where: Recipient = to_where.into();
        let text = text.into();
        let reply_to = reply_to.into();
        let mut sent_messages = Vec::new();

        let iter = SplitOverLengthTokens::new(text, 4096);

        for text in iter {
            // Try up to 3 times lol
            let mut looped: u8 = 0;
            let result = loop {
                looped += 1;
                let mut request = self
                    .send_message(to_where.clone(), text)
                    .parse_mode(teloxide::types::ParseMode::Html);
                if let Some(reply_to) = reply_to {
                    request = request.reply_to_message_id(reply_to);
                }
                let result = request.await;

                if let Err(RequestError::RetryAfter(duration)) = result {
                    tokio::time::sleep(duration).await;
                } else {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }

                if result.is_ok() || looped >= 3 {
                    break result;
                }
            };

            match result {
                Ok(message) => sent_messages.push(message),
                Err(e) => return Err(e),
            }
        }

        Ok(sent_messages)
    }
}

/// Various types of tokens that text can be split with.
#[derive(Clone, Copy, Debug)]
enum SplitTokenType {
    /// "\n\n\n"
    Section,
    /// "\n\n"
    Paragraph,
    /// "\n"
    Line,
    Word,
    Char,
}

impl SplitTokenType {
    /// Returns the amount of bytes to jump forward to skip the separator
    /// after a token of this type.
    fn jump_size(self) -> usize {
        match self {
            SplitTokenType::Section => 3,
            SplitTokenType::Paragraph => 2,
            SplitTokenType::Line => 1,
            SplitTokenType::Word => 1,
            SplitTokenType::Char => 0,
        }
    }
    /// Returns the size of the next token of this type.
    fn next_token_len(self, text: &str) -> usize {
        // It'd be cooler to just use str::split* functions, but they can
        // jump over an arbitrary amount of splitting tokens in a single
        // iteration, but we want to keep track of that.
        match self {
            SplitTokenType::Section => text.find("\n\n\n").unwrap_or(text.len()),
            SplitTokenType::Paragraph => text.find("\n\n").unwrap_or(text.len()),
            SplitTokenType::Line => text.find('\n').unwrap_or(text.len()),
            SplitTokenType::Word => text
                .find(|x| char::is_ascii_whitespace(&x))
                .unwrap_or(text.len()),
            SplitTokenType::Char => text
                .chars()
                .next()
                .expect("Tried to split with maximum length less than a character")
                .len_utf8(),
        }
    }

    /// Returns an iterator over the text with this type of token.
    #[allow(dead_code)] // i wrote this and it's nice i dont want to delete :(
    fn split_by<'a>(self, text: &'a str) -> Box<dyn Iterator<Item = &'a str> + 'a> {
        match self {
            SplitTokenType::Section => Box::new(text.split("\n\n\n")),
            SplitTokenType::Paragraph => Box::new(text.split("\n\n")),
            SplitTokenType::Line => Box::new(text.split('\n')),
            SplitTokenType::Word => Box::new(text.split_ascii_whitespace()),
            SplitTokenType::Char => Box::new(text.char_indices().map(|(byte, _)| {
                let text = &text[byte..text.len()];
                let first_char_size = text
                    .chars()
                    .next()
                    .expect("Tried to split with too maximum length less than a character")
                    .len_utf8();
                &text[0..first_char_size]
            })),
        }
    }
}

/// Iterator that splits text into tokens, all of which are under a specified size.
///
/// Splits by sections (separated by 3 newlines), then by paragraphs (2 newlines),
/// then by lines (1 newline), then by word (ASCII whitespace), then character by character,
/// until it fits.
pub struct SplitOverLengthTokens<'a> {
    data: &'a str,
    max_len: usize,
}

impl<'a> SplitOverLengthTokens<'a> {
    /// Create a new splitter with specified max length by bytes.
    ///
    /// # Panics
    /// Panics if a max length of 4 or less is specified.
    /// It may be impossible to output data at such lengths due to
    /// characters being up to 4 bytes in size.
    #[must_use]
    pub fn new(data: &str, max_len: usize) -> SplitOverLengthTokens<'_> {
        assert!(max_len >= 4, "Max length is too small");
        SplitOverLengthTokens { data, max_len }
    }
}

impl<'a> Iterator for SplitOverLengthTokens<'a> {
    type Item = &'a str;
    fn next(&mut self) -> Option<Self::Item> {
        self.data = self.data.trim_start();
        if self.data.is_empty() {
            return None;
        }

        let len = self.data.len();

        // Early return if the whole string fits.
        if len <= self.max_len {
            let output = self.data;
            self.data = &self.data[len..len];
            return Some(output);
        }

        let mut split_type;

        split_type = SplitTokenType::Section;
        if split_type.next_token_len(self.data) > self.max_len {
            split_type = SplitTokenType::Paragraph;
            if split_type.next_token_len(self.data) > self.max_len {
                split_type = SplitTokenType::Line;
                if split_type.next_token_len(self.data) > self.max_len {
                    split_type = SplitTokenType::Word;
                    if split_type.next_token_len(self.data) > self.max_len {
                        split_type = SplitTokenType::Char;
                    }
                }
            }
        }

        // split_type now contains the biggest token type we can fit.
        // Try to fit as many of those as possible into our output.

        let preprocessed_data = self.data;
        let mut output_size = 0;
        loop {
            let next_token_size = split_type.next_token_len(self.data);
            let total_token_size = if output_size > 0 {
                // There is a separator between the previous and this token.
                // Include its size.
                next_token_size + split_type.jump_size()
            } else {
                // This is the first token. Include as is.
                next_token_size
            };

            // Can we fit it in?
            if output_size + total_token_size <= self.max_len {
                output_size += total_token_size;
                self.data = &self.data[total_token_size..];
            } else {
                break;
            }
        }

        dbg!(split_type);
        assert_ne!(output_size, 0, "Maximum length is too small");

        // Send it.

        Some(preprocessed_data[..output_size].trim())

        //let jump = split_type.jump();
        ////println!("Jump size: {}", jump);
        ////println!("Text: {}", self.data);

        //let mut output_length = self.next_token(split_type);
        //let mut split = split_type.split_by(self.data);
        //let mut jump_at_end = false;
        //split.next();

        //loop {
        //    // Can we fit another one?
        //    let Some(next_split) = split.next() else {
        //        break;
        //    };

        //    // We can.
        //    if output_length + jump + next_split.len() > self.max_len {
        //        jump_at_end = true;
        //        break;
        //    }

        //    output_length += jump + next_split.len();
        //    //println!("New output length: {}", output_length);
        //}

        //let output = &self.data[0..output_length];
        //self.data = &self.data[output_length..len];
        //if jump_at_end {
        //    self.data = &self.data[jump..self.data.len()];
        //}

        //Some(output)
    }
}

#[cfg(test)]
mod tests {
    use super::SplitOverLengthTokens;

    #[test]
    fn word_split() {
        let data = "hi hello hi   HELLO!!!";
        assert_eq!(data.len(), 22);
        let mut splitter = SplitOverLengthTokens::new(data, 22);
        assert_eq!(splitter.next(), Some(data));
        assert_eq!(splitter.next(), None);

        let mut splitter = SplitOverLengthTokens::new(data, 21);
        assert_eq!(splitter.next(), Some("hi hello hi"));
        assert_eq!(splitter.next(), Some("HELLO!!!"));
        assert_eq!(splitter.next(), None);
    }

    #[test]
    fn word_and_char_split() {
        let data = "12345 123456 1234567 123 123456";
        let mut splitter = SplitOverLengthTokens::new(data, 6);
        assert_eq!(splitter.next(), Some("12345"));
        assert_eq!(splitter.next(), Some("123456"));
        assert_eq!(splitter.next(), Some("123456"));
        assert_eq!(splitter.next(), Some("7 123"));
        assert_eq!(splitter.next(), Some("123456"));
        assert_eq!(splitter.next(), None);
    }

    #[test]
    fn line_word_char_splits() {
        let data = "12345 12345\n12345\n12\n12\n1234567";
        let mut splitter = SplitOverLengthTokens::new(data, 6);
        assert_eq!(splitter.next(), Some("12345"));
        assert_eq!(splitter.next(), Some("12345"));
        assert_eq!(splitter.next(), Some("12345"));
        assert_eq!(splitter.next(), Some("12\n12"));
        assert_eq!(splitter.next(), Some("123456"));
        assert_eq!(splitter.next(), Some("7"));
        assert_eq!(splitter.next(), None);
    }
}
