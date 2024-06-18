use std::{sync::Arc, time::Duration};
use url::Url;

use crate::{
    database::Database,
    types::{Domain, IsSpam},
};

/////// IMPORTANT!!
/////// IMPORTANT!!
/////// IMPORTANT!!
/////// If spam checking logic is updated to catch more spam, increment this.
pub const SPAM_CHECKER_VERSION: u32 = 2;

// Checkers
mod american_groundhog_spam;
mod nft_spam;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IsSpamCheckResult {
    No,
    YesUrl,
    YesDomain,
    Maybe,
}

impl From<IsSpamCheckResult> for IsSpam {
    fn from(val: IsSpamCheckResult) -> Self {
        match val {
            IsSpamCheckResult::No => IsSpam::No,
            IsSpamCheckResult::YesUrl | IsSpamCheckResult::YesDomain => IsSpam::Yes,
            IsSpamCheckResult::Maybe => IsSpam::Maybe,
        }
    }
}

/// Check the link's domain against the database, or by visiting, as needed.
///
/// Returns [`None`] if both checking methods failed.
pub async fn check(database: &Arc<Database>, domain: &Domain, url: &Url) -> Option<IsSpam> {
    // Check the database...
    if let Some(is_spam) = database
        .is_spam(url, Some(domain), false)
        .await
        .expect("Database died!")
    {
        log::debug!("Checked {} with database and got: {:?}", url, is_spam);

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
            log::debug!("{} Is not in the database. Debouncing...", url);
            let visit_guard = database.domain_visit_debounce(domain.clone()).await;

            if visit_guard.is_none() {
                log::debug!("{} was just visited. Trying the database.", url);
                // Oh no nevermind, someone else visited it.
                // Just get the database result.
                drop(visit_guard);
                database
                    .is_spam(url, domain, false)
                    .await
                    .expect("Database died!")
            } else if let Ok(is_spam_check) = visit_and_check_if_spam(url).await {
                // Add it to the database.
                log::debug!("Visited {} and got: {:?}", url, is_spam_check);
                match is_spam_check {
                    IsSpamCheckResult::YesUrl => {
                        database
                            .add_url(url, IsSpam::Yes, false, false)
                            .await
                            .expect("Database died!");
                    }
                    _ => {
                        // All the other cases effectively apply to the domains.
                        database
                            .add_domain(domain, url, is_spam_check.into(), false, false)
                            .await
                            .expect("Database died!");
                    }
                };

                Some(is_spam_check.into())
            } else {
                // The visit probably timed out or something. Meh.
                log::debug!("{} timed out", url);
                None
            }
        }
    }
}

/// Check if a website served by the given URL is spam or not by visiting it.
async fn visit_and_check_if_spam(url: &Url) -> Result<IsSpamCheckResult, reqwest::Error> {
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
        {
            // Good enough lol
            return Ok(IsSpamCheckResult::Maybe);
        }

        // Fake cloudflare captcha.
        // Can't believe we got lied to. So sad :(

        return Ok(IsSpamCheckResult::YesUrl);
    }

    // Check the HTML...
    if nft_spam::is_spam_html(&text) {
        return Ok(IsSpamCheckResult::YesDomain);
    }
    if american_groundhog_spam::check_spam_html(&client, &text).await? {
        return Ok(IsSpamCheckResult::YesUrl);
    }

    // guess not.
    Ok(IsSpamCheckResult::No)
}
