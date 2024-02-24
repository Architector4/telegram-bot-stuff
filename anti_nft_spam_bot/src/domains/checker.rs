use reqwest::Error;
use std::time::Duration;
use url::Url;

use super::types::IsSpam;

fn is_spam_html(text: &str) -> bool {
    if !text.contains("cdnjs.cloudflare.com/ajax/libs/ethers") {
        return false;
    }

    if !text.contains("ethereumjs") {
        return false;
    }

    if !text.contains("web3.min.js") {
        return false;
    }

    true
}

pub async fn is_spam(url: &Url) -> Result<IsSpam, Error> {
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

    if text.contains("<title>Just a moment...</title>")
        && text.contains("Enable JavaScript and cookies to continue")
    {
        // Cloudflare captcha.
        return Ok(IsSpam::Maybe);
    }

    Ok(IsSpam::No)
}

//#[test]
//fn wat(){
//    let text = include_str!("/media/ext_hdd/nobackup/architector4/Downloads/spam.txt");
//    assert!(is_spam_html(text));
//}
