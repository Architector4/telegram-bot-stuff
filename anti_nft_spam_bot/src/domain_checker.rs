use reqwest::{Error, StatusCode};
use std::time::Duration;
use url::Url;

use crate::{
    database::Database,
    types::{Domain, IsSpam},
};

/// Check the link's domain against the database, or by visiting, as needed.
///
/// Returns [`None`] if both checking methods failed.
pub async fn check(database: &Database, domain: &Domain, url: &Url) -> Option<IsSpam> {
    // Check the database...
    if let Some(is_spam) = database
        .is_spam(url, Some(domain))
        .await
        .expect("Database died!")
    {
        log::debug!("Checked {} with database and got: {:?}", url, is_spam);
        Some(is_spam)
    } else {
        log::debug!("URL is not in database...");
        // Not in the database. Check for real...

        if let Some(is_telegram_spam) = is_spam_telegram_url(url) {
            // Add it to the database.
            log::debug!("Checked TG URL {} and got: {:?}", url, is_telegram_spam);
            database
                .add_url(url, is_telegram_spam, false, false)
                .await
                .expect("Database died!");
            Some(is_telegram_spam)
        } else if let Ok(is_spam) = visit_and_check_if_spam(url).await {
            // Add it to the database.
            log::debug!("Visited {} and got: {:?}", url, is_spam);
            database
                .add_domain(domain, Some(url), is_spam, false, false)
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

/// Check if a website served by the given URL is spam or not by visiting it.
pub async fn visit_and_check_if_spam(url: &Url) -> Result<IsSpam, Error> {
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
    let cf_mitigated_challenge = result
        .headers()
        .get("cf-mitigated")
        .and_then(|x| x.to_str().ok())
        .is_some_and(|x| x == "challenge");
    let status_code_forbidden = result.status() == StatusCode::FORBIDDEN;

    let text = result.text().await?;

    if is_spam_html(&text) {
        return Ok(IsSpam::Yes);
    }

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
            && cf_mitigated_challenge
            && header_cf_ray
            && header_content_length
            && !text.contains('\n')
        {
            // Good enough lol
            return Ok(IsSpam::Maybe);
        }

        // Fake cloudflare captcha.
        // Can't believe we got lied to. So sad :(

        return Ok(IsSpam::Yes);
    }

    Ok(IsSpam::No)
}

fn is_spam_html(text: &str) -> bool {
    text.contains("cdnjs.cloudflare.com/ajax/libs/ethers")
        || text.contains("ethereumjs")
        || text.contains("web3.min.js")
}

/// Returns `None` if it's not a telegram URL.
fn is_spam_telegram_url(url: &Url) -> Option<IsSpam> {
    let domain = url.domain()?;

    // Check if it's a telegram domain...
    if !matches!(
        domain.to_lowercase().as_str(),
        "t.me" | "telegram.me" | "telegram.dog"
    ) {
        return None;
    };

    // Ripping out Url::path_segments() body here lol
    let Some(path) = url.path().strip_prefix('/') else {
        // Shouldn't happen but eh
        return Some(IsSpam::No);
    };

    let path_lower = path.to_lowercase();
    let mut segments = path_lower.split('/');

    let Some(username) = segments.next() else {
        // Someone just linked t.me? lol
        return Some(IsSpam::No);
    };

    if !username.ends_with("bot") {
        // Not a telegram bot.
        return Some(IsSpam::No);
    };

    if username.ends_with("drop_bot") {
        // No way in hell a "...drop_bot" is anything other than spam, right?
        return Some(IsSpam::Yes);
    };

    let Some(params) = segments.next() else {
        // It's a bot, but no params. They use params.
        // If you're reading this:
        // don't worry, we'll review and patch as needed lol
        return Some(IsSpam::Maybe);
    };

    if ["claim", "drop"].iter().any(|x| params.contains(x)) {
        // Who else would post a bot with params of "claim" than spammers anyway?
        return Some(IsSpam::Yes);
    }

    Some(IsSpam::Maybe)
}

#[cfg(test)]
mod tests {
    //#[test]
    //fn wat(){
    //    let text = include_str!("/media/ext_hdd/nobackup/architector4/Downloads/spam.txt");
    //    assert!(is_spam_html(text));
    //}

    use url::Url;

    use crate::{domain_checker::is_spam_telegram_url, types::IsSpam};

    #[test]
    fn test_spam_bot_url() {
        let random_url = Url::parse("https://www.amogus.com/").unwrap();
        assert!(is_spam_telegram_url(&random_url).is_none());

        let random_telegram_url = Url::parse("https://t.me/Architector_4_Channel").unwrap();
        assert!(matches!(
            is_spam_telegram_url(&random_telegram_url),
            Some(IsSpam::No)
        ));

        let random_telegram_bot_url = Url::parse("https://t.me/Anti_NFT_Spam_Bot").unwrap();
        assert!(matches!(
            is_spam_telegram_url(&random_telegram_bot_url),
            Some(IsSpam::Maybe)
        ));

        let spam_url = Url::parse("https://t.me/FawunBot/claim").unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));

        let spam_url =
            Url::parse("https://t.me/stonksdrop_bot?start=bd658555-7bc6-4652-8afb-e69fdd3d4c0d")
                .unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));
    }
}
