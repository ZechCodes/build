//! Public proof signing is bounded independently from protected execution.
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;

const WINDOW: Duration = Duration::from_secs(60);
const SOURCE_REQUESTS: usize = 10;
const DEVICE_REQUESTS: usize = 60;
const MAX_SOURCES: usize = 1024;

#[derive(Default)]
pub struct DiscoveryLimiter(Mutex<State>);

#[derive(Default)]
struct State {
    sources: HashMap<IpAddr, VecDeque<Instant>>,
    device: VecDeque<Instant>,
}

impl DiscoveryLimiter {
    pub fn admit(&self, source: IpAddr) -> bool {
        let now = Instant::now();
        let mut state = self.0.lock().unwrap();
        trim(&mut state.device, now);
        state.sources.retain(|_, requests| {
            trim(requests, now);
            !requests.is_empty()
        });
        if state.device.len() >= DEVICE_REQUESTS {
            return false;
        }
        if !state.sources.contains_key(&source) && state.sources.len() >= MAX_SOURCES {
            return false;
        }
        let requests = state.sources.entry(source).or_default();
        if requests.len() >= SOURCE_REQUESTS {
            return false;
        }
        requests.push_back(now);
        state.device.push_back(now);
        true
    }
}

fn trim(requests: &mut VecDeque<Instant>, now: Instant) {
    while requests
        .front()
        .is_some_and(|time| now.duration_since(*time) >= WINDOW)
    {
        requests.pop_front();
    }
}
