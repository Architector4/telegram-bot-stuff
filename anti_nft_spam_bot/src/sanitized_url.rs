use std::{borrow::Cow, fmt::Display, str::FromStr};

use url::{Host, Url};

use crate::misc::parse_url_like_telegram;

fn is_host_an_ip_address(url: &Url) -> bool {
    matches!(url.host(), Some(Host::Ipv4(_) | Host::Ipv6(_)))
}

/// Normalize percent-encoding and lowercase the ASCII parts of the text.
pub fn normalize(input: &str, output: &mut String) {
    use percent_encoding::*;

    // All non-printable characters, but also
    // all whitespace and separators for URL paths and query separators, and percent itself lol
    const THIS_ASCII_SET: AsciiSet = CONTROLS
        .add(b'%')
        .add(b'&')
        .add(b'=')
        .add(b' ')
        .add(b'+')
        .add(b'/')
        .add(b'\\');

    if input.is_empty() {
        return;
    }

    // Percent decode.
    let mut data: Cow<'_, [u8]> = percent_decode(input.as_bytes()).into();

    // Replace all pluses with whitespace, if there's any.
    if let Some(first_plus) = data.iter().position(|x| *x == b'+') {
        let mut data_owned = data.into_owned();
        let has_pluses = &mut data_owned[first_plus..];
        has_pluses
            .iter_mut()
            .map(|x| {
                if *x == b'+' {
                    *x = b' ';
                }
            })
            .last();

        data = data_owned.into();
    }
    // Now percent encode.
    let percent_normalized = percent_encode(&data, &THIS_ASCII_SET);

    // This happens *after* percent-encoding, so percent-encoded characters are not
    // lowercased. Only ASCII characters can be lowercased here, so use ASCII lowercasing.
    let lowercased = percent_normalized
        .flat_map(|x| x.chars())
        .map(|c| c.to_ascii_lowercase());

    lowercased.map(|c| output.push(c)).last();
}

/// Convenience wrapper around [`normalize`] that returns a new string with the result.
#[must_use]
#[allow(unused)]
pub fn normalize_new_string(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    normalize(input, &mut output);
    output
}

/// A URL with various guarantees applied. See [`Self::new`] for details.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SanitizedUrl(Url);

impl SanitizedUrl {
    /// Sanitizes an input URL in a destructive manner. In particular, these rules are applied:
    ///
    /// * URLs that have a weird scheme, have no host, or are incomplete, are rejected; [`None`] is
    ///   returned.
    /// * Scheme is set to `https`.
    /// * Fragment (like "#hello" at the end) is discarded.
    /// * Username and password (like `name:pass@example.com`) are discarded.
    /// * Port specification is discarded.
    /// * Host (IP address or domain name) is [normalize]d.
    /// * Each individual segment of the path is [normalize]d; empty ones are removed. This breaks
    ///   some URLs with case-sensitive websites.
    /// * Trailing "/" at the end of the path is trimmed.
    /// * Modifications to the URL may be applied based on the domain name; for example, `youtu.be`
    ///   links are rewritten to `youtube.com` links and query (like "?a&b&c=d" at the end) is
    ///   discarded.
    /// * Each individual query parameter's key and value (if any) is [normalize]d. This breaks some
    ///   URLs with case-sensitive parameters.
    /// * Query parameters are alphabetically sorted and deduplicated.
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // Cannot panic
    pub fn new(mut url: Url) -> Option<Self> {
        // For .expect(…) calls
        static CAN_BE_A_BASE: &str =
            "URL shouldn't be cannot-be-a-base due to check at start of function";

        if url.scheme() == "file" || !url.has_host() || url.cannot_be_a_base() {
            return None;
        }
        if url.scheme() != "https" {
            // This discards a bunch of weird, likely invalid URLs while we're at it.
            url.set_scheme("https").ok()?;
        }

        // Normalize the host.
        //
        // A hostname is guaranteed to only contain ASCII letters a through z (uppercase or
        // lowercase), digits 0 through 9, and hyphen.
        // Other possibilities are IPv4 and IPv6 addresses as the host: those are just digits,
        // periods, and in IPv6's case, semicolons and square brackets.
        //
        // I assume that just lowercasing and removing "www" from start is enough here and the Url
        // crate can normalize whatever else.
        {
            let host = url.host_str().expect("Check above ensures host is present");

            if host.starts_with("www.") || !host.chars().all(|x| x.is_ascii_lowercase()) {
                let lowercased = host.to_ascii_lowercase();
                let www_trimmed = lowercased.trim_start_matches("www.");
                url.set_host(Some(www_trimmed))
                    .expect("Lowercasing host should not fail");
            }
        }

        // Some URLs like Signal's use fragments for security.
        // We'd like to wipe fragments here, which destroys those URLs too much, so preserve the
        // fragment before doing so.

        if let Some(fragment) = url.fragment().filter(|f| !f.is_empty()) {
            let host_str = url.host_str().expect("Host str should exist at this point");

            match host_str {
                "signal.me" | "signal.group" | "signal.link" | "signal.tube" | "signal.art" => {
                    let new_path = format!("{}/fragment__{}", url.path(), fragment);
                    url.set_path(&new_path);
                }
                _ => (),
            }
        }

        url.set_fragment(None);
        url.set_username("").ok()?;
        url.set_password(None).ok()?;
        url.set_port(None).ok()?;

        // Normalize path via individual segments.
        // This is because "example.com/a/b" and "example.com/a%2Fb" are two different things even
        // if they percent-decode to the same thing.
        //
        // This invalidates some URLs since some of them are case sensitive.
        // This is fine, it is exceedingly unlikely that a URL is spam but another URL that has the
        // exact same letters in it but with different casing isn't.
        {
            let mut normalized_path = String::new();
            for segment in url.path_segments().expect(CAN_BE_A_BASE) {
                // Skip empty segments.
                if segment.is_empty() {
                    continue;
                }
                normalized_path.push('/');
                normalize(segment, &mut normalized_path);
            }

            url.set_path(&normalized_path);
        }

        if !is_host_an_ip_address(&url) {
            // Domain specific modifications to the URL
            let host_str = url.host_str().expect("Host str should exist at this point");
            match host_str {
                "t.me" | "telegram.me" | "telegram.dog" => {
                    if host_str != "t.me" {
                        url.set_host(Some("t.me")).expect("t.me is a valid host");
                    }
                }
                x if x.ends_with(".t.me") => {
                    // It's a link like https://architector4.t.me/
                    // Translate to a normal username link.

                    // url.set_*() function calls are arranged to reduce top allocated memory at any
                    // point. Probably doesn't matter, but eh.

                    url.set_query(None);

                    // I'm unsure if links like https://foo.bar.t.me/ might exist,
                    // so I'm assuming that everything before ".t.me" in the host is a username.
                    let host_str = url.host_str().expect("Host str should exist at this point");
                    let username = host_str.trim_end_matches(".t.me").to_string();

                    url.set_host(Some("t.me")).expect("t.me is a valid host");

                    url.set_path(&username);
                }
                "youtu.be" => {
                    // Example URL: https://youtu.be/dQw4w9WgXcQ?blahblah
                    // We want to convert this to a normal YouTube URL, disregarding the query part.
                    let query = format!("v={}", &url.path()[1..]);
                    url.set_host(Some("youtube.com"))
                        .expect("youtube.com is a valid host");
                    url.set_path("watch");
                    url.set_query(Some(&query));
                }
                "youtube.com" | "m.youtube.com" => {
                    if host_str != "youtube.com" {
                        url.set_host(Some("youtube.com"))
                            .expect("youtube.com is a valid host");
                    }

                    if url.path() == "/watch" {
                        // A link to a video. Find the video query param, isolate it, remove all
                        // parameters, then add it.
                        //
                        // Video param may be not present. In that case, this code just clears the
                        // params entirely. That's fine.
                        let video_param = url
                            .query()
                            .and_then(|q| {
                                q.split('&').find(|param| {
                                    // The param may be percent-encoded and/or uppercase.
                                    // We need to check if it starts with "v=" or "%76="
                                    let mut chars = param.chars().map(|x| x.to_ascii_lowercase());

                                    match chars.next() {
                                        Some('v') => chars.next() == Some('='),
                                        Some('%') => {
                                            chars.next() == Some('%')
                                                && chars.next() == Some('7')
                                                && chars.next() == Some('6')
                                                && chars.next() == Some('=')
                                        }
                                        _ => false,
                                    }
                                })
                            })
                            .map(ToString::to_string);

                        url.set_query(video_param.as_deref());
                    } else if let Some((_, video_id)) = url.path().split_once("/shorts/") {
                        // A YouTube™ Shorts™ video. Unshortsing.
                        let video_param = format!("v={video_id}");

                        url.set_path("watch");
                        url.set_query(Some(&video_param));
                    }
                }
                "fixupx.com" | "fxtwitter.com" | "girlcockx.com" | "mobile.twitter.com"
                | "mobile.x.com" | "stupidpenisx.com" | "twitter.com" | "vxtwitter.com"
                | "x.com" | "hitlerx.com" | "cunnyx.com" | "fixvx.com" => {
                    if host_str != "twitter.com" {
                        url.set_host(Some("twitter.com"))
                            .expect("twitter.com is a valid host");
                    }

                    // If this is a tweet, extract its ID and exclude the username handle.
                    let mut segments = url.path_segments().expect(CAN_BE_A_BASE);

                    if segments.nth(1) == Some("status") {
                        if let Some(tweet_id) = segments.next() {
                            let new_path = format!("i/status/{tweet_id}");
                            url.set_path(&new_path);
                        }
                    }

                    // Query params never meaningfully matter on Twitter, as far as I can tell.
                    url.set_query(None);
                }
                _ => {}
            }
        }

        // Normalize query via individual parameters, if there's any.
        // This kills some URLs too. Same caveat as above.
        if let Some(query) = url.query() {
            if query.is_empty() {
                // If empty, just remove it.
                url.set_query(None);
            } else {
                let mut params: Vec<String> = Vec::new();
                let mut last_param: Option<&str> = None;

                for param in query.split('&') {
                    if last_param == Some(param) {
                        // Immediate duplicate. Skip.
                        continue;
                    }
                    last_param = Some(param);

                    let (key, val) = param.split_once('=').unwrap_or((param, ""));

                    let mut param_normalized = String::with_capacity(key.len() + 1 + val.len());

                    normalize(key, &mut param_normalized);
                    if !val.is_empty() {
                        param_normalized.push('=');
                        normalize(val, &mut param_normalized);
                    }

                    params.push(param_normalized);
                }

                // Sort by alphabet ascending,
                params.sort_unstable();
                // Proper deduping after the sort.
                params.dedup();

                let normalized_params = params.join("&");

                if normalized_params.is_empty() {
                    url.set_query(None);
                } else {
                    url.set_query(Some(&normalized_params));
                }
            }
        }

        Some(Self(url))
    }

    /// Returns the serialization of this URL.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Returns the host (a domain name or an IP address) in this URL.
    #[allow(clippy::missing_panics_doc)] // Cannot panic
    #[allow(unused)]
    #[must_use]
    pub fn host(&self) -> Host<&str> {
        self.as_ref()
            .host()
            .expect("SanitizedUrl guarantees URL has a host")
    }

    /// Returns whether or not the host in this URL is an IP address.
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // Cannot panic
    pub fn is_host_an_ip_address(&self) -> bool {
        is_host_an_ip_address(self.as_ref())
    }

    /// Returns the host (a domain name or an IP address) in this URL as a string.
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // Cannot panic
    pub fn host_str(&self) -> &str {
        self.as_ref()
            .host_str()
            .expect("SanitizedUrl guarantees URL has a host")
    }

    /// Returns the `?query` part in this URL, if any.
    ///
    /// Either [`None`] or a non-empty string.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.as_ref().query()
    }

    /// Returns a path in this URL. Guaranteed to start with `/`.
    #[must_use]
    pub fn path(&self) -> &str {
        self.as_ref().path()
    }

    /// Returns both sanitized result and original.
    #[must_use]
    pub fn from_url_with_original(url: Url) -> Option<(Self, Url)> {
        Some((Self::new(url.clone())?, url))
    }

    /// Parses the string to an [`Url`] and returns both that and the sanitized result.
    #[must_use]
    pub fn from_str_with_original(s: &str) -> Option<(Self, Url)> {
        let url = parse_url_like_telegram(s).ok()?;
        Some((Self::new(url.clone())?, url))
    }

    /// Removes all parts of the URL except the host and the protocol.
    pub fn remove_all_but_host(&mut self) {
        self.0.set_fragment(None);
        self.0.set_query(None);
        self.0.set_path("");
    }

    /// Return an iterator that destructures the URL.
    /// See [`SanitizedUrlDestructureIter`] for more details.
    #[must_use]
    pub fn destructure(&self) -> SanitizedUrlDestructureIter<'_> {
        SanitizedUrlDestructureIter::new(self)
    }

    /// How many times the URL should be destructured. 0 for none, 1 for same host and path but no query
    /// (if there was no query in the first place, means the same thing as 0), 2 and onward as
    /// iterations over [`SanitizedUrlDestructureIter`].
    ///
    /// Returns the result of destructuring this URL this many times, or [`None`] if it was
    /// destructured too much.
    #[must_use]
    pub fn destructure_to_number(&self, count: u64) -> Option<SanitizedUrl> {
        if count == 0 {
            Some(self.clone())
        } else {
            let mut destructurer = self.destructure();
            let mut host = "";
            let mut path = "";
            for _ in 1..=count {
                (host, path) = destructurer.next()?;
            }

            let output = SanitizedUrl::from_str(&format!("https://{host}{path}"))
                .expect("Host and path from SanitizedUrl are guaranteed to be valid");

            Some(output)
        }
    }
}

impl Display for SanitizedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<Url> for SanitizedUrl {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl FromStr for SanitizedUrl {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_url_like_telegram(s)
            .ok()
            .and_then(SanitizedUrl::new)
            .ok_or(())
    }
}

/// An iterator that destructures a [`SanitizedUrl`] and returns an iterator of tuples of host and
/// path. Query part is ignored.
///
/// For example, a link like <https://a.b.c.example.com/some/funky/path?ignored&params> would
/// destructure to:
///
/// * Some(("a.b.c.example.com", "/some/funky/path"))
/// * Some(("a.b.c.example.com", "/some/funky"))
/// * Some(("a.b.c.example.com", "/some"))
/// * Some(("a.b.c.example.com", "/"))
/// * Some(("b.c.example.com", "/"))
/// * Some(("c.example.com", "/"))
/// * Some(("example.com", "/"))
/// * None
///
/// (Can't put example code in rustdoc for private items unfortunately D:)
pub struct SanitizedUrlDestructureIter<'a> {
    host: &'a str,
    path: &'a str,
    host_is_ip: bool,
}

impl<'a> SanitizedUrlDestructureIter<'a> {
    /// Create an instance of this iterator destructuring this URL.
    #[must_use]
    pub fn new(url: &'a SanitizedUrl) -> Self {
        let host_is_ip = url.is_host_an_ip_address();
        Self::from_host_and_path(url.host_str(), url.path(), host_is_ip)
    }

    /// Create the iterator manually given a host and a path, as well as whether or not the host is
    /// an IP address or not.
    ///
    /// # Panics
    ///
    /// Panics if the input path does not start with a "/".
    #[must_use]
    pub fn from_host_and_path(host: &'a str, path: &'a str, host_is_ip: bool) -> Self {
        assert!(path.starts_with('/'), "Path must always start with a /");
        Self {
            host,
            path,
            host_is_ip,
        }
    }
}

impl<'a> Iterator for SanitizedUrlDestructureIter<'a> {
    type Item = (&'a str, &'a str);
    fn next(&mut self) -> Option<Self::Item> {
        if self.path.len() > 1 {
            let output = (self.host, self.path);

            (self.path, _) = self
                .path
                .rsplit_once('/')
                .expect("Path is guaranteed to always start with /");

            return Some(output);
        }

        // Path is empty; reduce host.

        if self.host_is_ip {
            // Host is an IP. Reduce and return that if we have it, otherwise bail.
            if self.host.is_empty() {
                None
            } else {
                let output = (self.host, "/");
                self.host = "";
                Some(output)
            }
        } else if let Some((_subdomain_to_shed, rest_of_host)) = self.host.split_once('.') {
            let output = (self.host, "/");
            self.host = rest_of_host;
            Some(output)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// Mostly just to note this for my own sanity lol
    #[test]
    fn url_crate_does_not_sanitize_percent_encoding() {
        let url = Url::parse("http://%68ello/%68ello?%68ello=%68ello").unwrap();
        assert_ne!(url.as_str(), "http://hello/hello?hello=hello");
    }
    #[test]
    fn normalize_is_idempotent() {
        // Not sure of the best way to test this, but here goes.
        let initial = "%252525%25%2525%25%25%25%25252525";
        let mut result = normalize_new_string(initial);
        assert_eq!(result, "%252525%25%2525%25%25%25%25252525");
        result = normalize_new_string(&result);
        assert_eq!(result, "%252525%25%2525%25%25%25%25252525");
    }

    #[test]
    fn general_test_idk() {
        // Note: during query parameter parsing, + itself is considered to mean whitespace.
        let url: SanitizedUrl = "ftp://AMOGUS:AMOGUS@EXAMPLE.com:6969/lol/wat?1+%31=%32&AMONG#us"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), "https://example.com/lol/wat?1%201=2&among");

        let url = Url::parse("https://example.com/woot/").unwrap();
        assert_eq!(
            SanitizedUrl::new(url).unwrap().as_str(),
            "https://example.com/woot"
        );
    }

    #[test]
    fn telegram_test() {
        let url: SanitizedUrl = "telegram.dog".parse().unwrap();
        assert_eq!(url.as_str(), "https://t.me/");

        let url: SanitizedUrl = "https://telegram.dog/Architector_4/amogus/amogus"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), "https://t.me/architector_4/amogus/amogus");

        let url: SanitizedUrl = "https://foo.bar.amogus.t.me/".parse().unwrap();
        assert_eq!(url.as_str(), "https://t.me/foo.bar.amogus");

        // We do NOT want to strip the tag thing after the /m/
        let url: SanitizedUrl = "https://t.me/m/awawawawa".parse().unwrap();
        assert_eq!(url.as_str(), "https://t.me/m/awawawawa");
    }

    #[test]
    fn youtube_test() {
        let expected_sanitized = "https://youtube.com/watch?v=dqw4w9wgxcq";

        let url: SanitizedUrl = "https://www.youtube.com/watch?t=22&v=dQw4w9WgXcQ"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);

        let url: SanitizedUrl = "https://www.m.youtube.com/watch?v=dQw4w9WgXcQ&t=22"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);

        let url: SanitizedUrl = "https://youtu.be/dQw4w9WgXcQ?t=69420".parse().unwrap();
        assert_eq!(url.as_str(), expected_sanitized);

        // This video isn't a Shorts, but the idea is the same.
        let url: SanitizedUrl = "https://www.youtube.com/shorts/dQw4w9WgXcQ"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);
    }

    #[test]
    fn twitter_test() {
        let expected_sanitized = "https://twitter.com/i/status/1668313119301718016";
        let url: SanitizedUrl =
            "https://www.x.com/rejectHisDesign/status/1668313119301718016?blahblah"
                .parse()
                .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);

        let url: SanitizedUrl =
            "https://www.twitter.com/rejectHisDesign/status/1668313119301718016?blahblah"
                .parse()
                .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);

        let url: SanitizedUrl = (
            "https://www.stupidpenisx.com/rejectHisDesign/status/1668313119301718016?tracking=shit"
        ).parse()
        .unwrap();
        assert_eq!(url.as_str(), expected_sanitized);
    }

    #[test]
    fn signal_test() {
        let url: SanitizedUrl = "https://signal.group/#wasdWASD".parse().unwrap();

        assert_eq!(url.as_str(), "https://signal.group/fragment__wasdwasd");
    }

    #[test]
    fn destructure_test() {
        let url: SanitizedUrl = "https://a.b.c.example.com/some/funky/path?ignored&params"
            .parse()
            .unwrap();
        let mut destructure = url.destructure();
        assert_eq!(
            destructure.next(),
            Some(("a.b.c.example.com", "/some/funky/path"))
        );

        assert_eq!(
            destructure.next(),
            Some(("a.b.c.example.com", "/some/funky"))
        );

        assert_eq!(destructure.next(), Some(("a.b.c.example.com", "/some")));

        assert_eq!(destructure.next(), Some(("a.b.c.example.com", "/")));

        assert_eq!(destructure.next(), Some(("b.c.example.com", "/")));

        assert_eq!(destructure.next(), Some(("c.example.com", "/")));

        assert_eq!(destructure.next(), Some(("example.com", "/")));

        assert_eq!(destructure.next(), None);

        assert_eq!(destructure.next(), None);
    }

    #[test]
    fn destructure_test_with_nothing_to_do() {
        let url: SanitizedUrl = "https://amogus.com/".parse().unwrap();
        let mut destructure = url.destructure();
        assert_eq!(destructure.next(), Some(("amogus.com", "/")));

        assert_eq!(destructure.next(), None);

        assert_eq!(destructure.next(), None);
    }

    #[test]
    fn destructure_to_number() {
        let url: SanitizedUrl = "https://a.b.c.example.com/some/funky/path?ignored&params"
            .parse()
            .unwrap();

        assert_eq!(url.destructure_to_number(0).unwrap(), url);
        assert_eq!(
            url.destructure_to_number(1).unwrap().as_str(),
            "https://a.b.c.example.com/some/funky/path"
        );
        assert_eq!(
            url.destructure_to_number(2).unwrap().as_str(),
            "https://a.b.c.example.com/some/funky"
        );
        assert_eq!(
            url.destructure_to_number(3).unwrap().as_str(),
            "https://a.b.c.example.com/some"
        );
        assert_eq!(
            url.destructure_to_number(4).unwrap().as_str(),
            "https://a.b.c.example.com/"
        );
        assert_eq!(
            url.destructure_to_number(5).unwrap().as_str(),
            "https://b.c.example.com/"
        );
        assert_eq!(
            url.destructure_to_number(6).unwrap().as_str(),
            "https://c.example.com/"
        );
        assert_eq!(
            url.destructure_to_number(7).unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(url.destructure_to_number(8), None);
    }
}
