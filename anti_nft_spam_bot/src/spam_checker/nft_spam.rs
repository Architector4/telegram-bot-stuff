use url::Url;

use crate::types::IsSpam;

pub fn is_spam_html(text: &str) -> bool {
    text.contains("cdnjs.cloudflare.com/ajax/libs/ethers")
        || text.contains("ethereumjs")
        || text.contains("web3.min.js")
}

/// Returns `None` if it's not a telegram URL.
pub fn is_spam_telegram_url(url: &Url) -> Option<IsSpam> {
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

    use super::is_spam_telegram_url;
    use crate::types::IsSpam;

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
