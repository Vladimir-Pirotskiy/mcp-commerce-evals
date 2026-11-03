//! The hosts and browser origins the MCP endpoint answers to.
//!
//! One list drives both checks that guard against DNS rebinding and cross-site calls: the `Host`
//! header must name a listed host, and a browser `Origin` header (the MCP transport spec says
//! servers MUST validate it) must be a page served from a listed host. Requests without an
//! `Origin`, which is every non-browser client, pass.

use std::fmt;
use std::net::Ipv6Addr;
use std::str::FromStr;

use axum::http::uri::Authority;

/// Hosts that are always accepted, so a locally run store works without configuration.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// The schemes a page calling the store may be served over.
const ORIGIN_SCHEMES: [&str; 2] = ["http", "https"];

/// A `Host` the store is willing to answer to: an authority (a name or IP address, with an
/// optional port) and nothing else. Parsing refuses a scheme, a path, userinfo, whitespace and
/// an empty or malformed entry, so a typo like `https://shop.example.org/` fails at startup
/// instead of silently never matching a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedHost(String);

/// Why an `--allowed-host` entry was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHost(String);

impl fmt::Display for InvalidHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidHost {}

impl AllowedHost {
    /// The entry as written (trimmed), for example `shop.example.org:8443`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AllowedHost {
    type Err = InvalidHost;

    fn from_str(text: &str) -> Result<Self, InvalidHost> {
        let entry = text.trim();
        let refuse = |why: &str| {
            InvalidHost(format!(
                "{text:?} is not a host: {why}; write a name or address with an optional port, \
                 such as shop.example.org or shop.example.org:8443"
            ))
        };
        if entry.is_empty() {
            return Err(refuse("it is empty"));
        }
        if entry.contains("://") {
            return Err(refuse("it has a scheme"));
        }
        if entry.contains('/') || entry.contains('?') || entry.contains('#') {
            return Err(refuse("it has a path"));
        }
        if entry.contains('@') {
            return Err(refuse("it has user information"));
        }
        if entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(refuse("it has whitespace"));
        }
        // A bare IPv6 address has colons that are not a port; everything else must parse as an
        // authority, which also checks a bracketed IPv6 address and the port range.
        if entry.parse::<Ipv6Addr>().is_ok() {
            return Ok(Self(entry.to_owned()));
        }
        let authority = Authority::try_from(entry).map_err(|_| refuse("it is malformed"))?;
        if authority.host().is_empty() {
            return Err(refuse("it has no host name"));
        }
        // `Authority` does not range-check the port, so look at the text after the last colon.
        let port = entry
            .rsplit_once(':')
            .map(|(_, port)| port)
            .filter(|port| !port.contains(']'));
        if port.is_some_and(|port| port.parse::<u16>().is_err()) {
            return Err(refuse("the port is malformed"));
        }
        Ok(Self(entry.to_owned()))
    }
}

impl fmt::Display for AllowedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Loopback hosts followed by the configured ones.
pub fn allowed_hosts(extra: &[AllowedHost]) -> Vec<String> {
    LOOPBACK_HOSTS
        .into_iter()
        .map(str::to_owned)
        .chain(extra.iter().map(|host| host.0.clone()))
        .collect()
}

/// The origin patterns for `hosts`, in the form rmcp matches: a scheme, the host, and either the
/// host's own port or `:*` when the entry names none.
pub fn allowed_origins(hosts: &[String]) -> Vec<String> {
    hosts
        .iter()
        .flat_map(|host| {
            let (name, port) = split_port(host);
            let port = port.map_or_else(|| "*".to_owned(), |p| p.to_string());
            ORIGIN_SCHEMES
                .into_iter()
                .map(move |scheme| format!("{scheme}://{name}:{port}"))
        })
        .collect()
}

/// A host entry split into its name (IPv6 in brackets, as an origin writes it) and port.
fn split_port(entry: &str) -> (String, Option<u16>) {
    let entry = entry.trim();
    match Authority::try_from(entry) {
        Ok(authority) => (bracketed(authority.host()), authority.port_u16()),
        Err(_) => (bracketed(entry), None),
    }
}

fn bracketed(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins(hosts: &[&str]) -> Vec<String> {
        let hosts: Vec<String> = hosts.iter().map(|h| (*h).to_owned()).collect();
        allowed_origins(&hosts)
    }

    #[test]
    fn a_host_without_a_port_allows_any_port_over_both_schemes() {
        assert_eq!(
            origins(&["shop.example.org"]),
            ["http://shop.example.org:*", "https://shop.example.org:*"]
        );
    }

    #[test]
    fn a_host_with_a_port_allows_only_that_port() {
        assert_eq!(
            origins(&["shop.example.org:8443"]),
            [
                "http://shop.example.org:8443",
                "https://shop.example.org:8443"
            ]
        );
    }

    #[test]
    fn ipv6_hosts_are_bracketed_as_an_origin_writes_them() {
        assert_eq!(origins(&["::1"])[0], "http://[::1]:*");
        assert_eq!(origins(&["[::1]:8787"])[0], "http://[::1]:8787");
    }

    fn parse(text: &str) -> Result<AllowedHost, InvalidHost> {
        text.parse()
    }

    #[test]
    fn names_addresses_and_ports_are_valid_hosts() {
        for ok in [
            "shop.example.org",
            "shop.example.org:8443",
            "localhost",
            "127.0.0.1:8787",
            "::1",
            "[::1]:8787",
            " shop.example.org ",
        ] {
            assert!(parse(ok).is_ok(), "{ok:?}");
        }
        assert_eq!(
            parse(" shop.example.org ").unwrap().as_str(),
            "shop.example.org"
        );
    }

    #[test]
    fn a_scheme_path_userinfo_or_junk_is_refused_with_a_clear_message() {
        for (bad, why) in [
            ("https://shop.example.org", "scheme"),
            ("shop.example.org/mcp", "path"),
            ("shop.example.org?x=1", "path"),
            ("user@shop.example.org", "user information"),
            ("", "empty"),
            ("   ", "empty"),
            ("shop example.org", "whitespace"),
            ("shop.example.org:99999", "port"),
            ("shop.example.org:", "port"),
            (":8443", "no host name"),
            ("[::1", "malformed"),
        ] {
            let error = parse(bad).unwrap_err().to_string();
            assert!(error.contains(why), "{bad:?}: {error}");
            assert!(error.contains("shop.example.org"), "{bad:?}: {error}");
        }
    }

    #[test]
    fn loopback_hosts_come_first() {
        let hosts = allowed_hosts(&["shop.example.org".parse().unwrap()]);
        assert_eq!(hosts, ["localhost", "127.0.0.1", "::1", "shop.example.org"]);
    }
}
