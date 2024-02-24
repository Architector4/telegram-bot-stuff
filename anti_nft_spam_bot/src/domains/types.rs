use std::fmt::Display;

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsSpam {
    No = 0,
    Yes = 1,
    Maybe = 2,
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
#[derive(Debug, Clone)]
pub struct Domain(String);

impl Domain {
    pub fn from_url(url: &Url) -> Option<Self> {
        url.domain().map(|x| Self(x.to_lowercase()))
    }
    /// Try to parse a string as a URL or a plain domain name into
    /// the [`Url`] type.
    pub fn preparse(string: &str) -> Result<Url, url::ParseError> {
        match Url::parse(string) {
            Ok(url) => Ok(url),
            Err(e) => {
                // We want to return this original error if the next step fails.
                if let Ok(url) = Url::parse(&format!("http://{}", string)) {
                    Ok(url)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Convenience function to try and parse a string directly to a domain name.
    #[allow(unused)]
    pub fn from_str(string: &str) -> Option<Self> {
        Self::preparse(string)
            .ok()
            .as_ref()
            .and_then(Self::from_url)
    }

    pub fn as_str(&self) -> &str {
        self.as_ref()
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
