use url::Url;

use crate::types::IsSpam;

pub fn is_spam_html(text: &str) -> bool {
    text.contains("cdnjs.cloudflare.com/ajax/libs/ethers")
        || text.contains("ethereumjs")
        || text.contains("web3.min.js")
}

/// Returns `None` if it's not a telegram URL.
/// Returns `Some(IsSpam::No)` if it's not recognized as spam
/// by this function.
pub fn is_spam_telegram_url(url: &Url) -> Option<IsSpam> {
    if !super::is_telegram_url(url) {
        return None;
    }

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

    if username == "blum" || username == "blumcryptobot" {
        // Annoying crypto spam that's a telegram bot but
        // also has a username without "bot" at the end.
        return Some(IsSpam::Yes);
    };

    if username == "notpixel" {
        // Same as above.
        return Some(IsSpam::Yes);
    }

    if !username.ends_with("bot") {
        // Not a telegram bot (usually).
        return Some(IsSpam::No);
    };

    if username.ends_with("hamster_kombat_bot") {
        // Specific one that's being spammed a bunch.
        return Some(IsSpam::Yes);
    }

    if username.ends_with("gemgombot") {
        return Some(IsSpam::Yes);
    }

    if username.ends_with("drft_party_bot") {
        return Some(IsSpam::Yes);
    }

    if username.ends_with("drop_bot") {
        // No way in hell a "...drop_bot" is anything other than spam, right?
        return Some(IsSpam::Yes);
    };

    let Some(params) = segments.next() else {
        // It's a bot, but no params. Probably fine.
        return Some(IsSpam::No);
    };

    // It has parameters... That's somewhat sus.

    if params.contains("claim") || params.contains("drop") {
        // Who else would post a bot with params of "claim" than spammers anyway?
        return Some(IsSpam::Yes);
    }

    let Some(query) = url.query() else {
        // Checks below check for the query parameters specifically
        return Some(IsSpam::Maybe);
    };

    if query.contains("startapp=kentId") {
        // Weird specificity of a bunch of "nft game telegram bot" spam links
        return Some(IsSpam::Yes);
    }

    if params.contains("game") && query.contains("ref=") {
        // Some spam "nft game telegram bot" links use this type of params instead
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
            Some(IsSpam::No)
        ));

        let spam_url = Url::parse("https://t.me/FawunBot/claim").unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));

        let spam_url =
            Url::parse("https://t.me/stonksdrop_bot?start=bd658555-7bc6-4652-8afb-e69fdd3d4c0d")
                .unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));

        let spam_url =
            Url::parse("https://t.me/hAmster_kombat_bot/start?startapp=kentId677635570").unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));

        let spam_url = Url::parse("http://t.me/trumpton_bot/game?ref=129383dHJJS").unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));

        let spam_url =
            Url::parse("https://t.me/notpixel/app?startapp=f6983374587_s573790").unwrap();
        assert!(matches!(is_spam_telegram_url(&spam_url), Some(IsSpam::Yes)));
    }
}
