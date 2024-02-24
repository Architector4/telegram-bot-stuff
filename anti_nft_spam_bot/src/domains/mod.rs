use url::Url;

use self::{
    database::Database,
    types::{Domain, IsSpam},
};

// Various types shared for this endeavor.
pub mod types;

// Spam checker itself
pub mod checker;

// Database
pub mod database;

/// Check the link against the database, or by visiting, as needed.
///
/// Returns [`None`] if both checking methods failed.
pub async fn check(database: &Database, domain: &Domain, url: &Url) -> Option<IsSpam> {
    // Check the database...
    if let Some(is_spam) = database
        .is_domain_spam(domain)
        .await
        .expect("Database died!")
    {
        log::debug!("Checked {} with database and got: {:?}", url, is_spam);
        Some(is_spam)
    } else {
        log::debug!("URL is not in database...");
        // Not in the database. Check for real...
        if let Ok(is_spam) = checker::is_spam(url).await {
            // Add it to the database.
            log::debug!("Visited {} and got: {:?}", url, is_spam);
            database
                .add_domain(domain, Some(url), is_spam)
                .await
                .expect("Database died!");

            Some(is_spam)
        } else {
            // Probably timed out or something. Meh.
            log::debug!("{} timed out", domain);
            None
        }
    }
}
