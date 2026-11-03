//! The hosts and browser origins the MCP endpoint answers to.
//!
//! One list drives both checks that guard against DNS rebinding and cross-site calls: the `Host`
//! header must name a listed host, and a browser `Origin` header (the MCP transport spec says
//! servers MUST validate it) must be a page served from a listed host. Requests without an
//! `Origin`, which is every non-browser client, pass.

use axum::http::uri::Authority;

/// Hosts that are always accepted, so a locally run store works without configuration.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// The schemes a page calling the store may be served over.
const ORIGIN_SCHEMES: [&str; 2] = ["http", "https"];

/// Loopback hosts followed by the configured ones.
pub fn allowed_hosts(extra: &[String]) -> Vec<String> {
    LOOPBACK_HOSTS
        .into_iter()
        .map(str::to_owned)
        .chain(extra.iter().cloned())
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

    #[test]
    fn loopback_hosts_come_first() {
        let hosts = allowed_hosts(&["shop.example.org".to_owned()]);
        assert_eq!(hosts, ["localhost", "127.0.0.1", "::1", "shop.example.org"]);
    }
}
