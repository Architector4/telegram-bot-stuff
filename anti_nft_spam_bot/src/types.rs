use std::fmt::Display;

use url::Url;

use crate::{
    database::{self, Database},
    parse_url_like_telegram,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsSpam {
    No = 0,
    Yes = 1,
    Maybe = 2,
}

impl IsSpam {
    /// Picks the option that is most condemning,
    /// along with a boolean that is true if `b` was picked, false otherwise.
    pub fn pick_most_condemning(a: Option<Self>, b: Option<Self>) -> Option<(Self, bool)> {
        match (a, b) {
            (Some(Self::Yes), _) => Some((Self::Yes, false)),
            (_, Some(Self::Yes)) => Some((Self::Yes, true)),
            (Some(Self::Maybe), _) => Some((Self::Maybe, false)),
            (_, Some(Self::Maybe)) => Some((Self::Maybe, true)),
            (_, Some(Self::No)) => Some((Self::No, true)),
            (Some(Self::No), _) => Some((Self::No, false)),
            (None, _) => None,
        }
    }
}

impl From<u8> for IsSpam {
    fn from(value: u8) -> Self {
        use IsSpam::*;
        match value {
            value if value == No as u8 => No,
            value if value == Yes as u8 => Yes,
            value if value == Maybe as u8 => Maybe,
            _ => panic!("Unknown value: {}", value),
        }
    }
}

impl From<IsSpam> for u8 {
    fn from(value: IsSpam) -> Self {
        value as u8
    }
}

/// A single domain name.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Domain(String);

impl Domain {
    pub fn from_url(url: &Url) -> Option<Self> {
        url.domain().map(|x| Self(x.to_lowercase()))
    }
    /// Convenience function to try and parse a string directly to a domain name.
    #[allow(unused)]
    pub fn from_str(string: &str) -> Option<Self> {
        parse_url_like_telegram(string)
            .ok()
            .as_ref()
            .and_then(Self::from_url)
    }

    pub fn as_str(&self) -> &str {
        self.as_ref()
    }

    pub(crate) fn new_invalid_unchecked() -> Domain {
        Domain(String::new())
    }
}

impl AsRef<str> for Domain {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(Debug)]
pub enum ReviewResponse {
    UrlSpam(Option<Domain>, Url),
    DomainSpam(Domain, Url),
    NotSpam(Option<Domain>, Url),
    Skip,
}

impl ReviewResponse {
    /// True if this response marks something as spam.
    #[allow(dead_code)]
    pub fn marks_as_spam(&self) -> bool {
        match self {
            ReviewResponse::Skip => false,
            ReviewResponse::UrlSpam(_, _) => true,
            ReviewResponse::DomainSpam(_, _) => true,
            ReviewResponse::NotSpam(_, _) => false,
        }
    }

    pub fn deconstruct(self) -> Option<(Option<Domain>, Url)> {
        match self {
            ReviewResponse::Skip => None,
            ReviewResponse::UrlSpam(d, u) => Some((d, u)),
            ReviewResponse::DomainSpam(d, u) => Some((Some(d), u)),
            ReviewResponse::NotSpam(d, u) => Some((d, u)),
        }
    }

    /// Returns true if ingesting this into the database
    /// would cause a change that we are interested in.
    pub async fn conflicts_with_db(&self, database: &Database) -> Result<bool, database::Error> {
        Ok(match self {
            ReviewResponse::Skip => false,
            ReviewResponse::UrlSpam(_, url) => database
                .is_url_spam(url, false)
                .await?
                .is_none_or(|x| x.0 != IsSpam::Yes || !x.1),
            ReviewResponse::DomainSpam(domain, _url) => database
                .is_domain_spam(domain, false)
                .await?
                .is_none_or(|x| x.0 != IsSpam::Yes || !x.2),
            ReviewResponse::NotSpam(domain, url) => database
                .is_spam(url, domain.as_ref(), true)
                .await?
                .is_none_or(|x| x.0 != IsSpam::No || !x.1),
        })
    }

    pub async fn from_str(
        value: &str,
        database: &Database,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut iter = value.split_ascii_whitespace();
        let action = iter.next().ok_or("Empty response")?;

        if action == "SKIP" {
            if iter.next().is_some() {
                Err("Extraneous data in response")?;
            }
            return Ok(ReviewResponse::Skip);
        }

        let table = iter.next().ok_or("No table name")?;
        let rowid: i64 = iter
            .next()
            .ok_or("No rowid")?
            .parse()
            .map_err(|_| "Failed to parse rowid")?;

        let crc32hash: u32 = iter
            .next()
            .ok_or("No hash")?
            .parse()
            .map_err(|_| "Failed to parse hash")?;

        if iter.next().is_some() {
            Err("Extraneous data in response")?;
        }

        let Some((url, domain_from_db)) =
            database.get_url_from_table_and_rowid(table, rowid).await?
        else {
            Err("Specified data is not in database")?
        };

        if crc32fast::hash(url.as_str().as_bytes()) != crc32hash {
            Err("Hash does not match! Please mark with a command instead and press Skip.")?;
        }

        let domain = match domain_from_db {
            Some(d) => Ok(d),
            None => Domain::from_url(&url).ok_or("Failed extracting domain from URL"),
        };

        let response = match action {
            "URL_SPAM" => ReviewResponse::UrlSpam(domain.ok(), url),
            "DOMAIN_SPAM" => ReviewResponse::DomainSpam(domain?, url),
            "NOT_SPAM" => ReviewResponse::NotSpam(domain.ok(), url),
            //"SKIP" => ReviewResponse::Skip, // Was handled above
            _ => Err("Unknown action type")?,
        };

        Ok(response)
    }
}

impl Display for ReviewResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewResponse::Skip => write!(f, "Skip"),
            ReviewResponse::UrlSpam(_, url) => write!(f, "URL is spam: {}", url),
            ReviewResponse::DomainSpam(_, url) => write!(f, "Domain and URL is spam: {}", url),
            ReviewResponse::NotSpam(_, url) => write!(f, "Neither domain nor URL is spam: {}", url),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkSusResult {
    Marked,
    AlreadyMarkedSus,
    AlreadyMarkedSpam,
    ManuallyReviewedNotSpam,
}
