use std::collections::HashSet;

use url::Url;

pub async fn check_spam_html(
    client: &reqwest::Client,
    mut html: &str,
) -> Result<bool, reqwest::Error> {
    // Find Telegram invite links here, visit them, and figure if
    // they lead to that "American Groundhog" chat

    let mut matches: HashSet<Url> = HashSet::new();

    for _ in 0..100 {
        // Limit this to 100 matches
        let Some(link_start) = html.find("https://t.me/+") else {
            break;
        };

        let mut a_match = &html[link_start..];

        let link_length = a_match.find('"').unwrap_or(a_match.len());

        a_match = &a_match[..link_length];

        // We found a wholesale Telegram link. Add it to our collection.
        if let Ok(url) = Url::parse(a_match) {
            matches.insert(url);
        }
        // Advance html forward so we don't match on this same thing.
        html = &html[link_start + link_length..];
    }

    // Now check each of those links.
    for a_match in matches {
        let result = client.get(a_match).send().await?;

        let text = result.text().await?;

        if text.contains("<span dir=\"auto\">American groundhog 🇺🇸</span>") {
            // buh-bye!
            return Ok(true);
        }

        if text.contains("<span dir=\"auto\">WikiLeaks</span>")
            && text.contains("We are here to bring you the truth")
        {
            return Ok(true);
        }

        if text.contains("<span dir=\"auto\">Memento</span>")
            && text.contains("Uncover hidden truths, decode mysteries")
        {
            return Ok(true);
        }

        // Sleep for a bit, so we don't hammer telegram in case there's multiple links.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // Nothing sus found. Oh well lol
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::super::{visit_and_check_if_spam, IsSpamCheckResult};
    use super::*;

    async fn check_url(bad_url: &'static str) {
        let bad_url = Url::parse(bad_url).unwrap();
        assert_eq!(
            visit_and_check_if_spam(&bad_url).await.unwrap(),
            IsSpamCheckResult::YesUrl,
            "failed on {}",
            bad_url
        );
    }

    #[tokio::test]
    async fn detect_american_groundhog() {
        check_url("https://telegra.ph/JOE-BIDEN-OFFICIALLY-SIGNS-THE-TIKTOK-BAN-BUT-YOU-DONT-KNOW-THE-REAL-REASON-FOR-IT-04-24").await;
    }

    #[tokio::test]
    async fn detect_memento() {
        check_url("https://telegra.ph/Simpsons2024LIVE-04-18").await
        // Literally the same thing but with a different date:
        // https://telegra.ph/2-out-of-3-Simpsons-Predictions-in-BANNED-Episode-Come-True-Third-One-Targeting-Donald-Trump-Expected-for-April-30-04-29
    }

    #[tokio::test]
    async fn detect_wikileaks() {
        check_url("https://telegra.ph/Sex-Trafficking-Ring-Organized-By-Famous-People-03-31").await
    }
}
