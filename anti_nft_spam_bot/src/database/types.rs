use std::fmt::Display;

use teloxide::types::{ChatId, MessageId, UserId};
use url::Url;

use crate::{
    sanitized_url::SanitizedUrl,
    types::{MessageDeleteReason, UrlDesignation},
};

/// Shortened info of a database entry on a URL.
#[derive(Clone, Copy, Debug, Hash)]
pub struct UrlInfoShort {
    /// ID of the entry.
    pub(super) id: i64,
    /// Amount of `?query` parameters of the URL this entry is for.
    pub(super) param_count: i64,
    /// Designation of this URL.
    pub(super) designation: UrlDesignation,
    /// Whether or not this designation has been manually decided on by a reviewer.
    pub(super) manually_reviewed: bool,
}

impl UrlInfoShort {
    /// Returns the ID of the entry.
    #[must_use]
    pub fn id(&self) -> i64 {
        self.id
    }
    /// Returns the amount of `?query` parameters of the URL this entry is for.
    #[must_use]
    #[allow(unused)]
    pub fn param_count(&self) -> i64 {
        self.param_count
    }
    /// Returns the designation of this URL.
    #[must_use]
    pub fn designation(&self) -> UrlDesignation {
        self.designation
    }
    /// Returns whether or not this entry's designation has been manually decided on by a reviewer.
    #[must_use]
    pub fn manually_reviewed(&self) -> bool {
        self.manually_reviewed
    }
}

impl Display for UrlInfoShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "<b>ID</b>: {}", self.id)?;
        writeln!(f, "<b>Parameter count</b>: {}", self.param_count)?;
        writeln!(f, "<b>Designation</b>: {}", self.designation)?;
        writeln!(f, "<b>Manually reviewed</b>: {}", self.manually_reviewed)
    }
}

/// Full info of a database entry on a URL.
#[derive(Clone, Debug)]
pub struct UrlInfoFull {
    /// Shortened info this is a superset of.
    pub(super) short: UrlInfoShort,
    /// Sanitized URL this entry represents.
    pub(super) sanitized_url: SanitizedUrl,
    /// Original URL this entry was based on.
    pub(super) original_url: Url,
}

impl UrlInfoFull {
    /// Returns the ID of the entry.
    #[must_use]
    #[allow(unused)]
    pub fn id(&self) -> i64 {
        self.short.id
    }
    /// Returns the amount of `?query` parameters of the URL this entry is for.
    #[must_use]
    #[allow(unused)]
    pub fn param_count(&self) -> i64 {
        self.short.param_count
    }
    /// Returns the designation of this URL.
    #[must_use]
    pub fn designation(&self) -> UrlDesignation {
        self.short.designation
    }
    /// Returns whether or not this entry's designation has been manually decided on by a reviewer.
    #[must_use]
    pub fn manually_reviewed(&self) -> bool {
        self.short.manually_reviewed
    }
    /// Returns the short info of this entry.
    #[allow(unused)]
    #[must_use]
    pub fn short(&self) -> &UrlInfoShort {
        &self.short
    }
    /// Returns the sanitized URL this entry represents.
    #[must_use]
    pub fn sanitized_url(&self) -> &SanitizedUrl {
        &self.sanitized_url
    }
    /// Returns the original URL this entry was based on.
    #[must_use]
    #[allow(unused)]
    pub fn original_url(&self) -> &Url {
        &self.original_url
    }
}

impl Display for UrlInfoFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "<b>Sanitized URL</b>: {}", self.sanitized_url)?;
        writeln!(f, "<b>Original URL</b>: {}", self.original_url)?;
        self.short.fmt(f)
    }
}

/// Result of [`Database::send_to_review`].
///
/// [`Database::send_to_review`]: super::Database::send_to_review
#[derive(Clone, Debug)]
pub enum SendToReviewResult {
    /// The URL was successfully sent for review and is now under the given ID.
    Sent { review_entry_id: i64 },
    /// This URL is already in the review queue.
    AlreadyOnReview,
    /// This URL is already in the database.
    AlreadyInDatabase(UrlInfoFull),
}

/// Result of [`Database::insert_or_update_url`].
///
/// [`Database::insert_or_update_url`]: super::Database::insert_or_update_url
#[derive(Clone, Copy, Hash, Debug)]
pub enum InsertOrUpdateResult {
    /// New URL entry was inserted into the database.
    Inserted { new_id: i64 },
    /// An existing URL entry was updated in the database with new info.
    Updated { old_info: UrlInfoShort },
    /// No change was done.
    NoChange { existing_info: UrlInfoShort },
}

impl InsertOrUpdateResult {
    /// Returns ID of either the existing data or the newly inserted data.
    #[must_use]
    pub fn id(&self) -> i64 {
        match self {
            Self::Inserted { new_id } => *new_id,
            Self::Updated { old_info } => old_info.id(),
            Self::NoChange { existing_info } => existing_info.id(),
        }
    }
}

/// An entry for a notification that a message(s) was removed.
#[derive(Clone, Copy, Debug)]
pub struct RemovalNote {
    /// ID of the entry.
    pub(super) id: i64,
    /// ID of the user whose message was removed.
    pub(super) userid: UserId,
    /// ID of the chat where this is posted.
    pub(super) chatid: ChatId,
    /// ID of this message within the chat.
    pub(super) messageid: MessageId,
    /// Timestamp of when this message was last posted or edited, in seconds since Unix epoch.
    // Could have probably made this std::time::SystemTime, but that also stores nanoseconds, so,
    // whatever
    pub(super) timestamp: i64,
    /// Count of removed messages that this currently represents.
    pub(super) count: u32,
    /// Reason for these messages being deleted, if singular.
    pub(super) delete_reason: Option<MessageDeleteReason>,
}

impl RemovalNote {
    /// Returns ID of the entry.
    #[must_use]
    pub fn id(&self) -> i64 {
        self.id
    }
    /// Returns the ID of the user whose message was removed.
    #[must_use]
    #[allow(unused)]
    pub fn userid(&self) -> UserId {
        self.userid
    }
    /// Returns chat ID of the message that this represents.
    #[must_use]
    #[allow(unused)]
    pub fn chatid(&self) -> ChatId {
        self.chatid
    }
    /// Returns message ID of the message that this represents.
    #[must_use]
    pub fn messageid(&self) -> MessageId {
        self.messageid
    }
    /// Returns the timestamp of when this message was last posted or edited, in seconds since Unix
    /// epoch.
    #[must_use]
    #[allow(unused)]
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }
    /// Returns the count of removed messages that this currently represents.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }
    /// Returns the reason for these messages being deleted, if singular.
    #[must_use]
    pub fn delete_reason(&self) -> Option<MessageDeleteReason> {
        self.delete_reason
    }
}
