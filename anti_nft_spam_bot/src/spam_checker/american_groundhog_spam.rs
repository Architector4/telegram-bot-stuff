/// Returns true if the provided HTML is from a spam Telegram invite link URL
/// spread by American Groundhog spammers, or false if it's not known.
///
/// This function does not check if the passed HTML is actually from Telegram,
/// so don't use it for pages that aren't.
pub fn check_spam_telegram_html(html: &str) -> bool {
    if html.contains("<span dir=\"auto\">American groundhog 🇺🇸") {
        // buh-bye!
        return true;
    }

    if html.contains("<span dir=\"auto\">WikiLeaks")
        && html.contains("We are here to bring you the truth")
    {
        return true;
    }

    if html.contains("<span dir=\"auto\">Memento</span>")
        && html.contains("Uncover hidden truths, decode mysteries")
    {
        return true;
    }

    if html.contains("<span dir=\"auto\">X Leaks</span>") {
        return true;
    }

    // Can't see anything of note.
    false
}

#[cfg(test)]
mod tests {
    // too lazy to fix these tests right now lmao

    //use super::super::{visit_and_check_if_spam, IsSpamCheckResult};
    //use super::*;

    //async fn check_url(bad_url: &'static str) {
    //    let bad_url = Url::parse(bad_url).unwrap();
    //    assert_eq!(
    //        visit_and_check_if_spam(&bad_url).await.unwrap(),
    //        IsSpamCheckResult::YesUrl,
    //        "failed on {}",
    //        bad_url
    //    );
    //}

    // Telegram started blocking showing info on theHTTPS accessible
    // description of the invite link, making this test fail.
    // Oh well, it works based on the previous 100 times this test was run lmao
    //#[tokio::test]
    //async fn detect_american_groundhog() {
    //    check_url("https://telegra.ph/JOE-BIDEN-OFFICIALLY-SIGNS-THE-TIKTOK-BAN-BUT-YOU-DONT-KNOW-THE-REAL-REASON-FOR-IT-04-24").await;
    //}

    //#[tokio::test]
    //async fn detect_memento() {
    //    check_url("https://telegra.ph/Simpsons2024LIVE-04-18").await
    //    // Literally the same thing but with a different date:
    //    // https://telegra.ph/2-out-of-3-Simpsons-Predictions-in-BANNED-Episode-Come-True-Third-One-Targeting-Donald-Trump-Expected-for-April-30-04-29
    //}

    // Same issue as above.
    //#[tokio::test]
    //async fn detect_wikileaks() {
    //    check_url("https://telegra.ph/Sex-Trafficking-Ring-Organized-By-Famous-People-03-31").await;
    //    check_url("https://telegra.ph/No-Way-He-Did-That-05-28").await;
    //}
}
