use reqwest::Error;
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
        if let Ok(is_spam) = visit_and_check_if_spam(url).await {
            // Add it to the database.
            log::debug!("Visited {} and got: {:?}", url, is_spam);
            database
                .add_domain(domain, Some(url), is_spam, false)
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

    let text = client.get(url.as_str()).send().await?.text().await?;

    if is_spam_html(&text) {
        return Ok(IsSpam::Yes);
    }

    if (text.contains("<title>Just a moment...</title>")
        && text.contains("Enable JavaScript and cookies to continue"))
        || text.contains("Attention Required! | Cloudflare")
    {
        // Cloudflare captcha.
        return Ok(IsSpam::Maybe);
    }

    Ok(IsSpam::No)
}

fn is_spam_html(text: &str) -> bool {
    text.contains("cdnjs.cloudflare.com/ajax/libs/ethers")
        || text.contains("ethereumjs")
        || text.contains("web3.min.js")
}

//#[test]
//fn wat(){
//    let text = include_str!("/media/ext_hdd/nobackup/architector4/Downloads/spam.txt");
//    assert!(is_spam_html(text));
//}
