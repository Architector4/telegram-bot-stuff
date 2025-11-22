use crate::{database::Database, sanitized_url::SanitizedUrl, types::UrlDesignation};

/// # Panics
///
/// Panics if the database dies lol
pub async fn is_url_spam(database: &Database, url: &SanitizedUrl) -> bool {
    if let Some(info) = database.get_url(url, false).await.expect("Database died!") {
        return info.designation() == UrlDesignation::Spam;
    }

    // No entry in the database found.
    // TODO: automatic checking.

    false
}
