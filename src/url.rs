//! Just enough URL handling for playlists: resolving a reference (a segment's address, a variant's)
//! against the playlist it came from, and naming a host for error messages. Browsers already parse
//! URLs, so this stays out of the way of a full URL crate and its Unicode tables.
//!
//! ponytail: no normalisation beyond what resolving needs (no percent-encoding, no case folding, no
//! IDNA): the address goes to `fetch` or to the app's proxy, which do their own.

/// Resolves `reference` against `base` (RFC 3986 section 5). `base` has to be an absolute URL with
/// an authority (`https://host/...`), as every playlist address is.
pub fn join(base: &str, reference: &str) -> Result<String, String> {
    let bad = || format!("can't resolve \"{reference}\" against \"{base}\"");
    let (scheme, rest) = split_scheme(base).ok_or_else(bad)?;
    let rest = rest.strip_prefix("//").ok_or_else(bad)?;
    let (authority, rest) = match rest.find(['/', '?', '#']) {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let (base_path, base_query, _) = split_path(rest);

    // A reference with a scheme of its own is already absolute.
    if split_scheme(reference).is_some() {
        return Ok(reference.to_owned());
    }
    if let Some(network) = reference.strip_prefix("//") {
        return Ok(format!("{scheme}://{network}"));
    }

    let (path, query, fragment) = split_path(reference);
    let (path, query) = if path.is_empty() {
        (base_path.to_owned(), query.or(base_query))
    } else if path.starts_with('/') {
        (remove_dot_segments(path), query)
    } else {
        let directory = match base_path.rfind('/') {
            Some(at) => &base_path[..=at],
            // An authority with no path: the reference sits at the root.
            None => "/",
        };
        (remove_dot_segments(&format!("{directory}{path}")), query)
    };
    let mut joined = format!("{scheme}://{authority}{path}");
    if let Some(q) = query {
        joined.push('?');
        joined.push_str(q);
    }
    if let Some(f) = fragment {
        joined.push('#');
        joined.push_str(f);
    }
    Ok(joined)
}

/// The host of an absolute URL, without credentials or port: what an error message may name.
/// `None` if it doesn't look like one.
pub fn host(url: &str) -> Option<&str> {
    let (_, rest) = split_scheme(url)?;
    let rest = rest.strip_prefix("//")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        // [::1]:8080
        Some(v6) => &authority[..v6.find(']').map_or(authority.len(), |at| at + 2)],
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then_some(host)
}

/// `http` and what follows the colon, if the text starts with a scheme.
fn split_scheme(s: &str) -> Option<(&str, &str)> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| (scheme, &s[colon + 1..]))
}

/// Path, query and fragment of what follows an authority (or of a reference).
fn split_path(s: &str) -> (&str, Option<&str>, Option<&str>) {
    let (before_fragment, fragment) = match s.split_once('#') {
        Some((a, f)) => (a, Some(f)),
        None => (s, None),
    };
    match before_fragment.split_once('?') {
        Some((path, query)) => (path, Some(query), fragment),
        None => (before_fragment, None, fragment),
    }
}

/// Removes `.` and `..` segments (RFC 3986 section 5.2.4).
fn remove_dot_segments(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
        match segment {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                // Never above the root, which is the empty first segment of an absolute path.
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples of RFC 3986 section 5.4, which every URL resolver has to agree with.
    #[test]
    fn resolves_like_the_rfc_says() {
        let base = "http://a/b/c/d;p?q";
        for (reference, want) in [
            // normal
            ("g:h", "g:h"),
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("g/", "http://a/b/c/g/"),
            ("/g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("g?y", "http://a/b/c/g?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("g?y#s", "http://a/b/c/g?y#s"),
            (";x", "http://a/b/c/;x"),
            ("g;x", "http://a/b/c/g;x"),
            ("g;x?y#s", "http://a/b/c/g;x?y#s"),
            ("", "http://a/b/c/d;p?q"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../g", "http://a/b/g"),
            ("../..", "http://a/"),
            ("../../", "http://a/"),
            ("../../g", "http://a/g"),
            // abnormal
            ("../../../g", "http://a/g"),
            ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."),
            ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
            ("g?y/./x", "http://a/b/c/g?y/./x"),
            ("g?y/../x", "http://a/b/c/g?y/../x"),
            ("g#s/./x", "http://a/b/c/g#s/./x"),
            ("g#s/../x", "http://a/b/c/g#s/../x"),
        ] {
            assert_eq!(join(base, reference).as_deref(), Ok(want), "{reference}");
        }
    }

    #[test]
    fn playlist_addresses_resolve_as_players_expect() {
        let playlist = "https://cdn.example.com:8443/live/ch1/index.m3u8?token=abc";
        let join = |r| join(playlist, r).unwrap();
        assert_eq!(
            join("seg1.ts"),
            "https://cdn.example.com:8443/live/ch1/seg1.ts"
        );
        assert_eq!(
            join("/other/seg.ts?x=1"),
            "https://cdn.example.com:8443/other/seg.ts?x=1"
        );
        assert_eq!(
            join("../ch2/low.m3u8"),
            "https://cdn.example.com:8443/live/ch2/low.m3u8"
        );
        assert_eq!(
            join("//edge.example.net/seg.ts"),
            "https://edge.example.net/seg.ts"
        );
        assert_eq!(join("http://elsewhere/seg.ts"), "http://elsewhere/seg.ts");
        // A server with no path at all: the reference sits at the root.
        assert_eq!(
            super::join("http://host", "seg.ts").unwrap(),
            "http://host/seg.ts"
        );
        assert_eq!(
            super::join("http://host?x=1", "?y=2").unwrap(),
            "http://host?y=2"
        );
    }

    #[test]
    fn a_base_that_is_not_an_absolute_url_is_refused() {
        assert!(join("index.m3u8", "seg.ts").is_err());
        assert!(join("/live/index.m3u8", "seg.ts").is_err());
        assert!(join("mailto:someone@example.com", "seg.ts").is_err());
    }

    #[test]
    fn the_host_is_named_without_credentials_port_or_path() {
        assert_eq!(host("https://example.com/a/b?c"), Some("example.com"));
        assert_eq!(
            host("http://user:secret@example.com:8080/x"),
            Some("example.com")
        );
        assert_eq!(host("http://[::1]:8080/x"), Some("[::1]"));
        assert_eq!(host("http://127.0.0.1"), Some("127.0.0.1"));
        assert_eq!(host("http://example.com?x"), Some("example.com"));
        assert_eq!(host("not a url"), None);
        assert_eq!(host("http://"), None);
    }
}
