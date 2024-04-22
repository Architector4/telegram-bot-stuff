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
    #[tokio::test]
    async fn detect_american_groundhog() {
        let bad_url = Url::parse("https://telegra.ph/JEFFREY-EPSTEIN-SPOTTED-IN-MEXICO-AND-NEW-FAMOUS-PEOPLE-DISCOVERED-ON-THE-JEFFREY-EPSTEIN-LIST-04-18").unwrap();
        assert_eq!(
            visit_and_check_if_spam(&bad_url).await.unwrap(),
            IsSpamCheckResult::YesUrl
        );
    }
}
