use std::{collections::HashSet, sync::Arc, time::Duration};
use url::Url;

use crate::{
    database::Database,
    types::{Domain, IsSpam},
};

/////// IMPORTANT!!
/////// IMPORTANT!!
/////// IMPORTANT!!
/////// If spam checking logic is updated to catch more spam, increment this.
pub const SPAM_CHECKER_VERSION: u32 = 5;

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
/// Returns the check's result, and whether or not it's from a database.
///
/// Returns [`None`] if both checking methods failed.
pub fn check<'a>(
    database: &'a Arc<Database>,
    domain: &'a Domain,
    url: &'a Url,
) -> impl std::future::Future<Output = Option<(IsSpam, bool)>> + 'a {
    check_inner(database, domain, url, 0)
}

async fn check_inner(
    database: &Arc<Database>,
    domain: &Domain,
    url: &Url,
    recursion_depth: u8,
) -> Option<(IsSpam, bool)> {
    // Check the database...
    let db_result = database
        .is_spam(url, Some(domain), false)
        .await
        .expect("Database died!");

    log::debug!(
        concat!(
            "Checked {} with database (recursion {}) and got: {:?}\n",
            "(second flag is true if manually reviewed)"
        ),
        url,
        recursion_depth,
        db_result
    );

    if recursion_depth > 1 {
        log::debug!("Recursion level in checker reached...");
        return None;
    }

    if let Some((result, true)) = db_result {
        // Manually reviewed. Go ahead.
        return Some((result, true));
    }

    // We now know it's not manually reviewed. Discard that flag.
    let db_result = db_result.map(|x| x.0);

    if let Some(IsSpam::Yes) = db_result {
        // Confirmed spam. Just return.
        return Some((IsSpam::Yes, true));
    }

    if let Some(db_result) = db_result {
        // It's marked as not spam or maybe spam.
        // Was this manually reviewed?

        // Is this specifically for this URL, or just the general domain result?
        if let Some(db_result_for_url) = database
            .is_url_spam(url, false)
            .await
            .expect("Database died!")
        {
            log::debug!(
                "Checked {url} URL specifically with database and got: {db_result_for_url:?}"
            );
            return Some((db_result_for_url.0, true));
        }

        // No result for the URL specifically, but we are in this branch.
        // This means `db_result` contains the result for the domain.

        // Assumption: if a domain is marked as not spam or maybe spam,
        // and a URL is just the domain without a path, then the domain's
        // result is accurate for that specific URL too.

        // URL crate's "empty path" seems to be just the slash,
        // but also check for emptystring in case this isn't always true.
        if url.path() == "/" || url.path().is_empty() {
            return Some((db_result, true));
        }
    }

    let mut url_maybe_spam = false;

    // All stuff above did not answer anything. Vibe check just the link...

    if let Some(url_looks_like_spam) = check_url_by_its_looks(url) {
        // Add it to the database.
        log::debug!("Checked if URL {url} looks like a spam URL and got: {url_looks_like_spam:?}");

        match url_looks_like_spam {
            IsSpam::Yes => {
                database
                    .add_url(url, url_looks_like_spam, false, false)
                    .await
                    .expect("Database died!");
                return Some((url_looks_like_spam, false));
            }
            // In case it's maybe spam or not spam, still check it properly.
            IsSpam::Maybe => url_maybe_spam = true,
            IsSpam::No => (),
        }
    }

    log::debug!("{url} Is not in the database. Debouncing...");
    let mut visit_guard = None;
    let has_visit_guard = if recursion_depth == 0 {
        visit_guard = database.domain_visit_debounce(domain.clone()).await;
        visit_guard.is_some()
    } else {
        true
    };

    if !has_visit_guard {
        log::debug!("{url} was just visited. Trying the database.");
        // Oh no nevermind, someone else visited it.
        // Just get the database result.
        drop(visit_guard);
        database
            .is_spam(url, domain, false)
            .await
            .expect("Database died!")
            .map(|x| (x.0, true))
    } else if let Ok(mut is_spam_check) =
        visit_and_check_if_spam(database, domain, url, recursion_depth).await
    {
        // Add it to the database.
        log::debug!("Visited {url} and got: {is_spam_check:?}");
        database
            .add_url(url, is_spam_check.into(), false, false)
            .await
            .expect("Database died!");
        // All the other cases effectively apply to the domains too...
        if is_spam_check != IsSpamCheckResult::YesUrl {
            if is_spam_check == IsSpamCheckResult::No && url_maybe_spam {
                is_spam_check = IsSpamCheckResult::Maybe;
            }

            database
                .add_domain(domain, url, is_spam_check.into(), false, false)
                .await
                .expect("Database died!");
        }

        Some((is_spam_check.into(), false))
    } else {
        // The visit probably timed out or something. Meh.
        log::debug!("{url} timed out");
        None
    }
}

fn get_reqwest_client(use_proxy: bool) -> Result<reqwest::Client, reqwest::Error> {
    use reqwest::*;
    use std::fs::*;
    use std::io::{BufRead, BufReader};
    use std::net::*;

    // Default policy is to follow up to 10 redirects.
    // And yeah, I'm using a "real browser" user agent. Sorgy. At least I'm not spamming
    // requests like a scraper or something. Need this to get around some CloudFlare
    // captchas lol
    let mut client = Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36")
        .timeout(Duration::from_secs(7))
        .connect_timeout(Duration::from_secs(7))
        // Force IPv4 because a proxy of mine doesn't support it lol
        // https://github.com/seanmonstar/reqwest/issues/584
        .local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    if use_proxy {
        if let Ok(proxies) = File::open("proxies.txt").map(|x| BufReader::new(x).lines()) {
            for line in proxies {
                match line {
                    Ok(line) => {
                        client = client.proxy(Proxy::all(line.trim())?);
                    }
                    Err(_) => break,
                }
            }
        }
    }

    client.build()
}

/// Check if a website served by the given URL is spam or not by visiting it.
async fn visit_and_check_if_spam(
    database: &Arc<Database>,
    domain: &Domain,
    url: &Url,
    recursion_depth: u8,
) -> Result<IsSpamCheckResult, reqwest::Error> {
    let mut client = get_reqwest_client(true)?;

    let result = match client.get(url.as_str()).send().await {
        Ok(x) => x,
        Err(e) => {
            // Try without proxy?
            client = get_reqwest_client(false)?;
            let result = client.get(url.as_str()).send().await?;
            // The ? in the line above is intentional.
            // If we got here, that means connecting without proxy succeeded.
            // Warn only in that case to not spam up logs due to generally invalid URLs.
            log::warn!("Proxy failed, but normal request didn't:\n{e:?}");
            result
        }
    };

    if result.url() != url {
        // We have been redirected. Check where we ended up with the database.
        if let Some(db_result) = database
            .is_spam(result.url(), None, false)
            .await
            .expect("Database died!")
        {
            let response = match db_result.0 {
                IsSpam::No => IsSpamCheckResult::No,
                IsSpam::Yes => {
                    // The "Yes" answer may be for the domain of the new URL as a whole.
                    // However, this does not necessarily villify *this* whole domain.
                    IsSpamCheckResult::YesUrl
                }
                IsSpam::Maybe => IsSpamCheckResult::Maybe,
            };

            return Ok(response);
        }
    }

    // Gather some specifics relevant to cloudflare captchas...
    let header_powered_by = result.headers().get("x-powered-by").is_some();
    let header_cf_ray = result.headers().get("cf-ray").is_some();
    let header_cache = result.headers().get("cf-cache-status").is_some();
    let status_code_forbidden = result.status() == reqwest::StatusCode::FORBIDDEN;

    let text = result.text().await?;

    if (text.contains("<title>Just a moment...</title>")
        && text.contains("Enable JavaScript and cookies to continue"))
        || text.contains("<title>Attention Required! | Cloudflare</title>")
        || (text.contains("cloudflare") && text.contains("erify that you are a human"))
    {
        // Cloudflare captcha.

        // Check validity of it being a *real* cloudflare captcha.
        if status_code_forbidden && !header_powered_by && !header_cache && header_cf_ray {
            // It's a captcha. Bleh. If it's spam, users will let us know with /spam.

            log::debug!("Got CloudFlare captcha on URL {url}");
            return Ok(IsSpamCheckResult::No);
        }

        // Fake cloudflare captcha.
        // Can't believe we got lied to. So sad :(

        return Ok(IsSpamCheckResult::YesUrl);
    }

    if domain.as_str().eq_ignore_ascii_case("telegra.ph")
        || domain.as_str().eq_ignore_ascii_case("teletype.in")
    {
        // If it's telegra.ph, do some extra funny checks.
        // Find links here and figure if they're spam themselves.

        let mut matches: HashSet<Url> = HashSet::with_capacity(20);
        let mut html: &str = &text;
        let mut current_consensus = IsSpamCheckResult::No;

        // Limit this to 20 matches
        while matches.len() < 20 {
            let Some(link_start) = html.find("http") else {
                break;
            };

            let mut a_match = &html[link_start..];

            let link_length = a_match.find('"').unwrap_or(a_match.len());

            a_match = &a_match[..link_length];

            // We found a potential link. Add it to our collection.
            if let Ok(new_url) = Url::parse(a_match) {
                if &new_url != url {
                    matches.insert(new_url);
                }
            }
            // Advance html forward so we don't match on this same thing.
            html = &html[link_start + link_length..];
        }

        log::debug!(
            "RECURSING #{} on {} with {} links...",
            recursion_depth,
            url,
            matches.len()
        );

        let mut iter = matches.iter().peekable();

        // Now check each of those links.
        while let Some(a_match) = iter.next() {
            let Some(match_domain) = Domain::from_url(a_match) else {
                continue;
            };
            // We don't care if this is from DB or not here lol
            if let Some((x, _)) = Box::pin(check_inner(
                database,
                &match_domain,
                a_match,
                recursion_depth + 1,
            ))
            .await
            {
                match x {
                    IsSpam::No => (),
                    IsSpam::Yes => return Ok(IsSpamCheckResult::YesUrl),
                    IsSpam::Maybe => current_consensus = IsSpamCheckResult::Maybe,
                }
            }

            // Sleep for a bit, so we don't hammer telegram in case there's multiple links.
            if iter.peek().is_some() {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }

        // Checked a telegra.ph link. Return results on that.
        return Ok(current_consensus);
    }

    // Check the HTML...
    if nft_spam::is_spam_html(&text) {
        return Ok(IsSpamCheckResult::YesDomain);
    }

    if is_telegram_url(url) && american_groundhog_spam::check_spam_telegram_html(&text) {
        return Ok(IsSpamCheckResult::YesUrl);
    }

    // guess not.
    Ok(IsSpamCheckResult::No)
}

/// Check if this URL, just on its own, looks like spam.
pub fn check_url_by_its_looks(url: &Url) -> Option<IsSpam> {
    nft_spam::is_spam_telegram_url(url)
}

/// Returns true if this URL's domain is Telegram.
pub fn is_telegram_url(url: &Url) -> bool {
    let Some(domain) = url.domain() else {
        return false;
    };

    domain.eq_ignore_ascii_case("t.me")
        || domain.eq_ignore_ascii_case("telegram.me")
        || domain.eq_ignore_ascii_case("telegram.dog")
}
