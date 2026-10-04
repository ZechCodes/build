//! Client-held bearer hints select previously validated query destinations.
//! A hint is not an authenticated client identity. Possessing it can cause a
//! probe or replace its remembered address after a validated resolution; the
//! hint cannot supply an address or bypass fresh answer validation.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

const MAX_CLIENTS: usize = 64;
const ADDRESS_LIFETIME: Duration = Duration::from_secs(3600);

struct ClientAddress {
    token: uuid::Uuid,
    address: Ipv4Addr,
    validated_at: Instant,
}

/// Bounded, process-memory cache keyed by a client-held bearer UUID.
/// Remembered addresses are query destinations and never resolution answers.
#[derive(Default)]
pub(crate) struct LanAddressCache {
    clients: Mutex<VecDeque<ClientAddress>>,
}

impl LanAddressCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(super) fn remembered(&self, token: uuid::Uuid) -> Option<Ipv4Addr> {
        let mut clients = self.clients.lock().ok()?;
        clients.retain(|client| client.validated_at.elapsed() < ADDRESS_LIFETIME);
        let position = clients.iter().position(|client| client.token == token)?;
        let client = clients.remove(position)?;
        let address = client.address;
        // Usage affects eviction order, while only a validated reply refreshes
        // the address lifetime.
        clients.push_back(client);
        Some(address)
    }

    pub(super) fn remember_validated(&self, token: uuid::Uuid, addresses: &[IpAddr]) {
        let Some(address) = addresses.iter().find_map(|address| match address {
            IpAddr::V4(address) if super::usable_ipv4(*address) && super::local_ipv4(*address) => {
                Some(*address)
            }
            _ => None,
        }) else {
            return;
        };
        let Ok(mut clients) = self.clients.lock() else {
            return;
        };
        clients.retain(|client| {
            client.token != token && client.validated_at.elapsed() < ADDRESS_LIFETIME
        });
        if clients.len() == MAX_CLIENTS {
            clients.pop_front();
        }
        clients.push_back(ClientAddress {
            token,
            address,
            validated_at: Instant::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn looking_up_a_hint_refreshes_its_eviction_recency() {
        let cache = LanAddressCache::new();
        let hints: Vec<_> = (0..MAX_CLIENTS).map(|_| uuid::Uuid::new_v4()).collect();
        for hint in &hints {
            cache.remember_validated(*hint, &["192.168.1.50".parse().unwrap()]);
        }
        assert!(cache.remembered(hints[0]).is_some());
        cache.remember_validated(uuid::Uuid::new_v4(), &["192.168.1.60".parse().unwrap()]);
        assert!(
            cache.remembered(hints[0]).is_some(),
            "a recently queried hint remains cached"
        );
        assert!(
            cache.remembered(hints[1]).is_none(),
            "the least recently used hint is evicted"
        );
        assert_eq!(cache.clients.lock().unwrap().len(), MAX_CLIENTS);
    }

    #[tokio::test(start_paused = true)]
    async fn looking_up_a_hint_does_not_refresh_its_validation_lifetime() {
        let cache = LanAddressCache::new();
        let hint = uuid::Uuid::new_v4();
        cache.remember_validated(hint, &["192.168.1.50".parse().unwrap()]);
        tokio::time::advance(ADDRESS_LIFETIME - Duration::from_secs(1)).await;
        assert!(cache.remembered(hint).is_some());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            cache.remembered(hint).is_none(),
            "lookup recency cannot extend validated address age"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clients_have_separate_addresses_that_expire_after_an_hour() {
        let cache = LanAddressCache::new();
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        cache.remember_validated(first, &["192.168.1.50".parse().unwrap()]);
        assert_eq!(
            cache.remembered(first),
            Some("192.168.1.50".parse::<std::net::Ipv4Addr>().unwrap())
        );
        assert!(cache.remembered(second).is_none());
        tokio::time::advance(Duration::from_secs(3600)).await;
        assert!(cache.remembered(first).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_fresh_resolution_replaces_the_previous_address() {
        let cache = LanAddressCache::new();
        let client = uuid::Uuid::new_v4();
        let old = "192.168.1.50".parse().unwrap();
        let recent = "192.168.1.60".parse().unwrap();
        cache.remember_validated(client, &[old]);
        tokio::time::advance(Duration::from_secs(1800)).await;
        cache.remember_validated(client, &[recent]);
        tokio::time::advance(Duration::from_secs(1800)).await;
        assert_eq!(
            cache.remembered(client),
            Some("192.168.1.60".parse::<std::net::Ipv4Addr>().unwrap())
        );
    }

    #[tokio::test]
    async fn retained_client_and_address_counts_are_bounded() {
        let cache = LanAddressCache::new();
        let first = uuid::Uuid::new_v4();
        cache.remember_validated(first, &["192.168.1.50".parse().unwrap()]);
        for _ in 0..64 {
            cache.remember_validated(uuid::Uuid::new_v4(), &["192.168.1.60".parse().unwrap()]);
        }
        assert!(cache.remembered(first).is_none());
        let client = uuid::Uuid::new_v4();
        for address in 50..60 {
            cache.remember_validated(
                client,
                &[std::net::Ipv4Addr::new(192, 168, 1, address).into()],
            );
        }
        assert_eq!(
            cache.remembered(client),
            Some(std::net::Ipv4Addr::new(192, 168, 1, 59))
        );
    }
}
