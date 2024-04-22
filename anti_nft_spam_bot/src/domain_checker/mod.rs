use std::{sync::Arc, time::Duration};
use url::Url;

use crate::{
    database::Database,
    types::{Domain, IsSpam},
};

// Checkers
mod american_groundhog_spam;
mod nft_spam;

/// Check the link's domain against the database, or by visiting, as needed.
///
/// Returns [`None`] if both checking methods failed.
pub async fn check(database: &Arc<Database>, domain: &Domain, url: &Url) -> Option<IsSpam> {
    // Check the database...
    if let Some(is_spam) = database
        .is_spam(url, Some(domain))
        .await
        .expect("Database died!")
    {
        log::debug!("Checked {} with database and got: {:?}", url, is_spam);

        if is_spam != IsSpam::Yes {
            // Potential problem scenario:
            // 1. Spammers use a new type of spam that isn't detected by this bot yet.
            // 2. Someone replies /spam to it, and it gets put into the database as
            // "maybe spam", or otherwise "not spam".
            // 3. The bot is updated to support this new type of spam.
            // 4. The spam that reuses the same links doesn't get blocked, since
            // it's marked as "maybe/not spam" in the database, and hence not
            // checked by the new code.
            //
            // This could probably use a database table field, like "bot check version",
            // which makes the entry be ignored if it's of an older version than the
            // bot is currently running, or something. For now, I'm too lazy, So
            // I'll just patch this up by always re-checking non-spam telegram URLs
            // anyway. It's a few string comparisons and may be even cheaper than
            // a database lookup, to be honest. lol

            if let Some(telegram_url_check) = nft_spam::is_spam_telegram_url(url) {
                log::debug!(
                    "Checked {} as a TG URL anyway and got: {:?}",
                    url,
                    telegram_url_check
                );
                return Some(telegram_url_check);
            }
        }
        Some(is_spam)
    } else {
        log::debug!("URL is not in database...");
        // Not in the database. Check for real...

        if let Some(is_telegram_spam) = nft_spam::is_spam_telegram_url(url) {
            // Add it to the database.
            log::debug!("Checked TG URL {} and got: {:?}", url, is_telegram_spam);
            database
                .add_url(url, is_telegram_spam, false, false)
                .await
                .expect("Database died!");
            Some(is_telegram_spam)
        } else {
            // No database result. We're going to visit the URL and log by the domain.
            log::debug!("{} Is not in the database. Debouncing...", url);
            let visit_guard = database.domain_visit_debounce(domain.clone()).await;

            if visit_guard.is_none() {
                log::debug!("{} was just visited. Trying the database.", url);
                // Oh no nevermind, someone else visited it.
                // Just get the database result.
                drop(visit_guard);
                database
                    .is_domain_spam(domain)
                    .await
                    .expect("Database died!")
            } else if let Ok(is_spam) = visit_and_check_if_spam(url).await {
                // Add it to the database.
                log::debug!("Visited {} and got: {:?}", url, is_spam);
                database
                    .add_domain(domain, Some(url), is_spam, false, false)
                    .await
                    .expect("Database died!");

                Some(is_spam)
            } else {
                // The visit probably timed out or something. Meh.
                log::debug!("{} timed out", url);
                None
            }
        }
    }
}

/// Check if a website served by the given URL is spam or not by visiting it.
pub async fn visit_and_check_if_spam(url: &Url) -> Result<IsSpam, reqwest::Error> {
    // Default policy is to follow up to 10 redirects.
    let client = reqwest::Client::builder()
        .user_agent("GoogleOther")
        .timeout(Duration::from_secs(7))
        .connect_timeout(Duration::from_secs(7))
        .build()?;

    let result = client.get(url.as_str()).send().await?;

    // Gather some specifics relevant to cloudflare captchas...
    let header_powered_by = result.headers().get("x-powered-by").is_some();
    let header_cf_ray = result.headers().get("cf-ray").is_some();
    let header_cache = result.headers().get("cf-cache-status").is_some();
    let header_content_length = result.headers().get("content-length").is_some();
    let status_code_forbidden = result.status() == reqwest::StatusCode::FORBIDDEN;

    let text = result.text().await?;

    if (text.contains("<title>Just a moment...</title>")
        && text.contains("Enable JavaScript and cookies to continue"))
        || text.contains("Attention Required! | Cloudflare")
        || (text.contains("cloudflare") && text.contains("erify that you are a human"))
    {
        // Cloudflare captcha.

        // Check validity of it being a *real* cloudflare captcha.
        if status_code_forbidden
            && !header_powered_by
            && !header_cache
            && header_cf_ray
            && header_content_length
        {
            // Good enough lol
            return Ok(IsSpam::Maybe);
        }

        // Fake cloudflare captcha.
        // Can't believe we got lied to. So sad :(

        return Ok(IsSpam::Yes);
    }

    // Check the HTML...
    if nft_spam::is_spam_html(&text) {
        return Ok(IsSpam::Yes);
    }
    if american_groundhog_spam::check_spam_html(&client, &text).await? {
        return Ok(IsSpam::Yes);
    }

    Ok(IsSpam::No)
}
