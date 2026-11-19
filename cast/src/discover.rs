//! Cast devices on the local network, from mDNS `_googlecast._tcp` (browse only). TXT keys:
//! `fn` the name the user gave it, `md` the model, `id` a UUID, `ca` capability bits, `rs` what it
//! plays now. A Cast Group made in the Google Home app is a service of its own (`md=Google Cast
//! Group`, another port) on the speaker that leads it: we play to it as to one device.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const GOOGLECAST: &str = "_googlecast._tcp.local.";

#[derive(Clone, Debug, PartialEq)]
pub struct CastDevice {
    /// Its `id`, else the mDNS instance name.
    pub id: String,
    /// The name in the Google Home app (`fn`).
    pub name: String,
    pub model: String,
    /// The address to use (see [`choose`]; the first one mDNS gave until a session picks).
    pub ip: IpAddr,
    /// Every address it advertised (merged over its mDNS answers, sorted): a TV with IPv6
    /// privacy addresses may fetch the stream from another one of its own.
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    /// A Cast Group (`md=Google Cast Group`).
    pub group: bool,
    /// What it plays now (`rs`), e.g. "YouTube"; empty when idle.
    pub playing: String,
}

impl CastDevice {
    /// A device at a known address (tests, `--host`): no mDNS.
    pub fn at(ip: IpAddr, port: u16, name: &str) -> Self {
        CastDevice { id: format!("{ip}:{port}"), name: name.into(), model: "?".into(), ip, addrs: vec![ip], port, group: false, playing: String::new() }
    }
}

/// The device of that name (any case), else of that id.
pub fn pick<'a>(devices: &'a [CastDevice], target: &str) -> Option<&'a CastDevice> {
    let t = target.trim();
    devices.iter().find(|d| d.name.eq_ignore_ascii_case(t)).or_else(|| devices.iter().find(|d| d.id == t))
}

fn from_info(info: &mdns_sd::ServiceInfo) -> Option<CastDevice> {
    let ip = uzumaki_airplay::discover::best_ip(info.get_addresses().iter().copied())?;
    let mut addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
    addrs.sort();
    let get = |k: &str| info.get_property_val_str(k).map(str::to_string).filter(|v| !v.is_empty());
    let instance = info.get_fullname().strip_suffix(GOOGLECAST).unwrap_or(info.get_fullname()).trim_end_matches('.').to_string();
    let model = get("md").unwrap_or_else(|| "?".into());
    Some(CastDevice {
        id: get("id").unwrap_or_else(|| instance.clone()),
        name: get("fn").unwrap_or(instance),
        group: model == "Google Cast Group",
        model,
        ip,
        addrs,
        port: info.get_port(),
        playing: get("rs").unwrap_or_default(),
    })
}

/// Applies one event (devices keyed by their mDNS full name). Returns whether anything changed.
fn absorb(found: &mut BTreeMap<String, CastDevice>, event: mdns_sd::ServiceEvent) -> bool {
    match event {
        mdns_sd::ServiceEvent::ServiceResolved(info) => match from_info(&info) {
            Some(mut d) => {
                // An answer may carry only some of its addresses: keep the ones seen before.
                if let Some(old) = found.get(info.get_fullname()) {
                    for a in &old.addrs {
                        if !d.addrs.contains(a) {
                            d.addrs.push(*a);
                        }
                    }
                    d.addrs.sort();
                    d.ip = uzumaki_airplay::discover::best_ip(d.addrs.iter().copied()).unwrap_or(d.ip);
                }
                found.insert(info.get_fullname().to_string(), d.clone()).as_ref() != Some(&d)
            }
            None => false,
        },
        mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => found.remove(&fullname).is_some(),
        _ => false,
    }
}

/// Which address family to talk to the device with (`UZUMAKI_CAST_IP=4|6`, the tool's
/// `--ipv4`/`--ipv6`; default `Auto`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Family {
    #[default]
    Auto,
    V4,
    V6,
}

impl Family {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "4" | "ipv4" | "v4" => Some(Family::V4),
            "6" | "ipv6" | "v6" => Some(Family::V6),
            "auto" | "" => Some(Family::Auto),
            _ => None,
        }
    }
}

fn link_local(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(v) => v.is_link_local(),
        IpAddr::V6(v) => (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn in_subnet(a: Ipv4Addr, ip: Ipv4Addr, mask: Ipv4Addr) -> bool {
    u32::from(a) & u32::from(mask) == u32::from(ip) & u32::from(mask)
}

/// This computer's IPv4 addresses with their netmasks.
pub fn local_v4() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .map(|v| {
            v.into_iter()
                .filter_map(|i| match i.addr {
                    if_addrs::IfAddr::V4(a) => Some((a.ip, a.netmask)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The device's address to connect to and to serve the stream on, and why. `Auto`: IPv4 when the
/// device has one on a subnet of this computer (then the stream URL is IPv4 too and the TV fetches
/// from the address we talk to), else IPv4 when this computer has any (routed), else a global IPv6.
/// None when the forced family has no address.
pub fn choose(addrs: &[IpAddr], family: Family, local: &[(Ipv4Addr, Ipv4Addr)]) -> Option<(IpAddr, &'static str)> {
    let v4: Vec<Ipv4Addr> = addrs.iter().filter_map(|a| if let IpAddr::V4(v) = a { Some(*v) } else { None }).filter(|v| !v.is_link_local()).collect();
    let v6: Vec<IpAddr> = addrs.iter().copied().filter(|a| a.is_ipv6() && !link_local(*a)).collect();
    match family {
        Family::V4 => v4.first().map(|a| (IpAddr::V4(*a), "IPv4 (forced)")),
        Family::V6 => v6.first().map(|a| (*a, "IPv6 (forced)")),
        Family::Auto => {
            if let Some(a) = v4.iter().find(|a| a.is_loopback() || local.iter().any(|(ip, mask)| in_subnet(**a, *ip, *mask))) {
                return Some((IpAddr::V4(*a), "IPv4 on this computer's subnet"));
            }
            if let Some(a) = v4.first().filter(|_| local.iter().any(|(ip, _)| !ip.is_loopback())) {
                return Some((IpAddr::V4(*a), "IPv4 (routed)"));
            }
            v6.first().map(|a| (*a, "IPv6 (the device has no usable IPv4)")).or_else(|| addrs.first().map(|a| (*a, "the only address")))
        }
    }
}

/// Sorted by name, this computer left out (a Linux box running a Cast receiver, say).
fn list(found: &BTreeMap<String, CastDevice>, own: &[IpAddr]) -> Vec<CastDevice> {
    let mut v: Vec<CastDevice> = found.values().filter(|d| !d.addrs.iter().any(|a| own.contains(a))).cloned().collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    v
}

/// Browses for up to `timeout`; with `target`, stops as soon as that device is found.
pub fn discover(timeout: Duration, target: Option<&str>) -> Result<Vec<CastDevice>, String> {
    let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS: {e}"))?;
    let rx = mdns.browse(GOOGLECAST).map_err(|e| format!("mDNS browse: {e}"))?;
    let own = uzumaki_airplay::discover::own_addresses();
    let mut found = BTreeMap::new();
    let start = Instant::now();
    while start.elapsed() < timeout {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(e) => {
                absorb(&mut found, e);
            }
            Err(_) => continue,
        }
        if let Some(t) = target
            && pick(&list(&found, &own), t).is_some()
        {
            break;
        }
    }
    let _ = mdns.shutdown();
    Ok(list(&found, &own))
}

/// Browses until dropped and hands every change of the list to `on_change` (its own thread).
pub struct Browser {
    stop: Arc<AtomicBool>,
}

impl Browser {
    pub fn start(on_change: impl Fn(Vec<CastDevice>) + Send + 'static) -> Result<Self, String> {
        let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS: {e}"))?;
        let rx = mdns.browse(GOOGLECAST).map_err(|e| format!("mDNS browse: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        std::thread::Builder::new()
            .name("cast-browse".into())
            .spawn(move || {
                let own = uzumaki_airplay::discover::own_addresses();
                let mut found = BTreeMap::new();
                let mut shown: Option<Vec<CastDevice>> = None;
                while !quit.load(Ordering::SeqCst) {
                    let Ok(e) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
                    if absorb(&mut found, e) {
                        let now = list(&found, &own);
                        if shown.as_ref() != Some(&now) {
                            on_change(now.clone());
                            shown = Some(now);
                        }
                    }
                }
                let _ = mdns.shutdown();
            })
            .map_err(|e| e.to_string())?;
        Ok(Browser { stop })
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(instance: &str, ip: &str, port: u16, txt: &[(&str, &str)]) -> mdns_sd::ServiceInfo {
        mdns_sd::ServiceInfo::new(GOOGLECAST, instance, "host.local.", ip, port, txt).unwrap()
    }

    #[test]
    fn a_tv_and_a_group_become_rows() {
        let mut found = BTreeMap::new();
        let tv = info("Google-TV-0123", "192.168.1.40", 8009, &[("fn", "Smart TV Pro"), ("md", "Google TV Streamer"), ("id", "abc"), ("rs", "")]);
        let group = info("Google-Cast-Group-9", "192.168.1.41", 32187, &[("fn", "Downstairs"), ("md", "Google Cast Group"), ("id", "grp")]);
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceResolved(tv.clone())));
        assert!(!absorb(&mut found, mdns_sd::ServiceEvent::ServiceResolved(tv)), "the same record again changes nothing");
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceResolved(group)));
        let rows = list(&found, &[]);
        assert_eq!(rows.iter().map(|d| (d.name.as_str(), d.group, d.port)).collect::<Vec<_>>(), vec![("Downstairs", true, 32187), ("Smart TV Pro", false, 8009)]);
        assert_eq!(pick(&rows, "smart tv pro").map(|d| d.id.as_str()), Some("abc"));
        assert_eq!(pick(&rows, "grp").map(|d| d.name.as_str()), Some("Downstairs"));
        let own: Vec<IpAddr> = vec!["192.168.1.40".parse().unwrap()];
        assert_eq!(list(&found, &own).len(), 1, "this computer is left out");
        let full = format!("Google-TV-0123.{GOOGLECAST}");
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceRemoved(GOOGLECAST.into(), full)));
        assert_eq!(list(&found, &[]).len(), 1);
    }

    #[test]
    fn ipv4_wins_when_this_computer_is_on_its_subnet() {
        let v4: IpAddr = "192.168.1.173".parse().unwrap();
        let v6a: IpAddr = "2a01:e11:4039:c270:2b07:8567:abb:8fd8".parse().unwrap();
        let ll: IpAddr = "fe80::1".parse().unwrap();
        let addrs = vec![v6a, ll, v4];
        let home = [("192.168.1.20".parse().unwrap(), "255.255.255.0".parse().unwrap())];
        assert_eq!(choose(&addrs, Family::Auto, &home), Some((v4, "IPv4 on this computer's subnet")));
        let elsewhere = [("10.0.0.2".parse().unwrap(), "255.255.255.0".parse().unwrap())];
        assert_eq!(choose(&addrs, Family::Auto, &elsewhere).map(|c| c.0), Some(v4), "routed IPv4 still before IPv6");
        assert_eq!(choose(&addrs, Family::Auto, &[]).map(|c| c.0), Some(v6a), "no IPv4 here: IPv6, never link-local");
        assert_eq!(choose(&[v6a, ll], Family::Auto, &home).map(|c| c.0), Some(v6a));
        assert_eq!(choose(&addrs, Family::V6, &home).map(|c| c.0), Some(v6a));
        assert_eq!(choose(&addrs, Family::V4, &[]).map(|c| c.0), Some(v4));
        assert_eq!(choose(&[v6a], Family::V4, &home), None, "forced IPv4 without one");
        assert_eq!(choose(&["127.0.0.1".parse().unwrap()], Family::Auto, &[]).map(|c| c.0), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(Family::parse("6"), Some(Family::V6));
        assert_eq!(Family::parse("ipv4"), Some(Family::V4));
    }

    #[test]
    fn addresses_from_several_answers_are_kept() {
        let mut found = BTreeMap::new();
        let one = info("TV", "2a01:e11::8fd8", 8009, &[("fn", "TV")]);
        let two = info("TV", "192.168.1.173", 8009, &[("fn", "TV")]);
        absorb(&mut found, mdns_sd::ServiceEvent::ServiceResolved(one));
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceResolved(two)));
        let d = &list(&found, &[])[0];
        assert_eq!(d.addrs.len(), 2, "{:?}", d.addrs);
        assert_eq!(d.ip, "192.168.1.173".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn without_fn_the_instance_name_is_shown() {
        let d = from_info(&info("Living-Room-speaker", "10.0.0.5", 8009, &[])).unwrap();
        assert_eq!((d.name.as_str(), d.id.as_str(), d.model.as_str()), ("Living-Room-speaker", "Living-Room-speaker", "?"));
    }
}
