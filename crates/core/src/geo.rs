//! Server geolocation.
//!
//! The Roblox log records the game server's public address, which we turn into a
//! rough location purely for display. Independent of db-api — this is the one
//! part of v1 that carried over unchanged in spirit.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde::Deserialize;

const ENDPOINT: &str = "https://ipinfo.io";
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Location {
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}

impl Location {
    /// Human-readable one-liner, skipping the parts the service did not know.
    pub fn describe(&self) -> String {
        let parts: Vec<&str> = [&self.city, &self.region, &self.country]
            .into_iter()
            .filter_map(|p| p.as_deref())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            "Unknown location".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Whether an address is worth sending to a geolocation service.
///
/// The log's `UDMUX` line also carries an `RCC Server Address`, which is a
/// private LAN address. Sending one out would leak nothing useful but would
/// waste a request and return nonsense, so filter here as well as in the parser.
pub fn is_geolocatable(ip: &str) -> bool {
    match ip.parse::<Ipv4Addr>() {
        Ok(addr) => {
            !addr.is_private()
                && !addr.is_loopback()
                && !addr.is_link_local()
                && !addr.is_unspecified()
                && !addr.is_broadcast()
                && !addr.is_documentation()
        }
        // Not an IPv4 literal; refuse rather than guess.
        Err(_) => false,
    }
}

/// Look up an address. Returns `None` for anything unroutable or on any failure
/// — geolocation is decoration, so it must never surface as an error.
pub async fn locate(ip: &str) -> Option<Location> {
    if !is_geolocatable(ip) {
        return None;
    }

    crate::tls::ensure_provider();
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
    let response = client
        .get(format!("{ENDPOINT}/{ip}/json"))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }
    response.json::<Location>().await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_addresses() {
        // The RCC address from a real UDMUX line.
        assert!(!is_geolocatable("10.8.4.154"));
        assert!(!is_geolocatable("192.168.1.1"));
        assert!(!is_geolocatable("172.16.0.1"));
    }

    #[test]
    fn rejects_loopback_and_unspecified() {
        assert!(!is_geolocatable("127.0.0.1"));
        assert!(!is_geolocatable("0.0.0.0"));
    }

    #[test]
    fn rejects_documentation_ranges() {
        // Our own scrubbed fixture uses 203.0.113.x, which must not be queried.
        assert!(!is_geolocatable("203.0.113.10"));
    }

    #[test]
    fn rejects_non_addresses() {
        assert!(!is_geolocatable(""));
        assert!(!is_geolocatable("not-an-ip"));
        assert!(!is_geolocatable("1.2.3"));
    }

    #[test]
    fn accepts_a_real_public_address() {
        // The genuine UDMUX address from the captured session.
        assert!(is_geolocatable("128.116.95.33"));
    }

    #[test]
    fn describes_a_full_location() {
        let l = Location {
            city: Some("Toronto".into()),
            region: Some("Ontario".into()),
            country: Some("CA".into()),
        };
        assert_eq!(l.describe(), "Toronto, Ontario, CA");
    }

    #[test]
    fn describes_a_partial_location() {
        let l = Location {
            city: None,
            region: Some("".into()),
            country: Some("CA".into()),
        };
        assert_eq!(l.describe(), "CA", "empty and missing parts are skipped");
    }

    #[test]
    fn describes_an_empty_location() {
        let l = Location {
            city: None,
            region: None,
            country: None,
        };
        assert_eq!(l.describe(), "Unknown location");
    }

    #[tokio::test]
    async fn never_queries_an_unroutable_address() {
        // No network involved: the guard must short-circuit.
        assert_eq!(locate("10.0.0.1").await, None);
        assert_eq!(locate("garbage").await, None);
    }
}
