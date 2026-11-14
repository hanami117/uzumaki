//! AirPlay speakers on the local network, from mDNS (browse only: no connection to any device).
//! A speaker usually shows up twice: `_airplay._tcp` (`<Name>._airplay._tcp.local.`, AirPlay 2) and
//! `_raop._tcp` (`<MAC>@<Name>._raop._tcp.local.`, AirPlay 1). They are merged by name.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

const AIRPLAY: &str = "_airplay._tcp.local.";
const RAOP: &str = "_raop._tcp.local.";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    /// The mDNS instance name (`Bedroom`, or `AABBCCDDEEFF@Bedroom` for RAOP).
    pub instance: String,
    pub port: u16,
    pub txt: Vec<(String, String)>,
}

impl Service {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// The name the user sees in Apple's picker.
    pub name: String,
    pub ip: IpAddr,
    /// The mDNS host name (`Bedroom.local.`).
    pub hostname: String,
    pub airplay: Option<Service>,
    pub raop: Option<Service>,
}

impl Device {
    pub fn model(&self) -> &str {
        self.airplay.as_ref().and_then(|s| s.get("model")).or_else(|| self.raop.as_ref().and_then(|s| s.get("am"))).unwrap_or("?")
    }
}

/// The speaker's name from an mDNS full name: the instance before the service type, without the
/// RAOP `<MAC>@` prefix.
pub fn device_name(fullname: &str, ty: &str) -> String {
    let instance = fullname.strip_suffix(ty).unwrap_or(fullname).trim_end_matches('.');
    match instance.split_once('@') {
        Some((mac, name)) if !mac.is_empty() && mac.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
        _ => instance.to_string(),
    }
}

/// The device called `target` (case-insensitive, exact name).
pub fn pick<'a>(devices: &'a [Device], target: &str) -> Option<&'a Device> {
    devices.iter().find(|d| d.name.eq_ignore_ascii_case(target.trim()))
}

/// Browses for up to `timeout`. With `target`, stops early once that device has both services
/// (or 1.5 s after its first one, for a speaker that only has one).
pub fn discover(timeout: Duration, target: Option<&str>) -> Result<Vec<Device>, String> {
    let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS: {e}"))?;
    let rx_airplay = mdns.browse(AIRPLAY).map_err(|e| format!("mDNS browse: {e}"))?;
    let rx_raop = mdns.browse(RAOP).map_err(|e| format!("mDNS browse: {e}"))?;
    let mut found: BTreeMap<String, Device> = BTreeMap::new();
    let start = Instant::now();
    let mut target_seen: Option<Instant> = None;
    while start.elapsed() < timeout {
        let mut got = false;
        for (rx, ty) in [(&rx_airplay, AIRPLAY), (&rx_raop, RAOP)] {
            while let Ok(event) = rx.try_recv() {
                got = true;
                if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                    let name = device_name(info.get_fullname(), ty);
                    let Some(ip) = best_ip(info.get_addresses().iter().copied()) else { continue };
                    let service = Service {
                        instance: info.get_fullname().strip_suffix(ty).unwrap_or(info.get_fullname()).trim_end_matches('.').to_string(),
                        port: info.get_port(),
                        txt: info.get_properties().iter().map(|p| (p.key().to_string(), p.val_str().to_string())).collect(),
                    };
                    let d = found.entry(name.to_lowercase()).or_insert_with(|| Device {
                        name: name.clone(),
                        ip,
                        hostname: info.get_hostname().to_string(),
                        airplay: None,
                        raop: None,
                    });
                    if ty == AIRPLAY {
                        d.airplay = Some(service);
                        d.ip = ip; // the AirPlay 2 record's address wins
                        d.hostname = info.get_hostname().to_string();
                    } else {
                        d.raop = Some(service);
                    }
                }
            }
        }
        if let Some(t) = target
            && let Some(d) = found.get(&t.trim().to_lowercase())
        {
            let seen = *target_seen.get_or_insert_with(Instant::now);
            if (d.airplay.is_some() && d.raop.is_some()) || seen.elapsed() > Duration::from_millis(1500) {
                break;
            }
        }
        if !got {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = mdns.shutdown();
    Ok(found.into_values().collect())
}

/// IPv4 first (AirPlay devices answer on both, IPv4 is what Music Assistant uses), then IPv6
/// that isn't link-local (a link-local address needs a scope id the helper can't take).
fn best_ip(addrs: impl Iterator<Item = IpAddr>) -> Option<IpAddr> {
    let mut v6 = None;
    for a in addrs {
        match a {
            IpAddr::V4(v4) if !v4.is_link_local() => return Some(a),
            IpAddr::V6(v6a) if (v6a.segments()[0] & 0xffc0) != 0xfe80 => v6 = Some(a),
            _ => {}
        }
    }
    v6
}

/// cliairplay's `--txt`: the `_airplay._tcp` TXT as one space-separated `key=value` string, the
/// way Music Assistant passes it (pairs with whitespace are skipped: the binary splits on spaces).
/// Some receivers advertise their feature bits only on `_raop` (`ft=`): add them then.
pub fn txt_arg(device: &Device) -> String {
    let mut pairs: Vec<String> = Vec::new();
    if let Some(ap) = &device.airplay {
        for (k, v) in &ap.txt {
            if k.is_empty() || k.chars().any(char::is_whitespace) || v.chars().any(char::is_whitespace) {
                continue;
            }
            pairs.push(format!("{k}={v}"));
        }
        if ap.get("features").or(ap.get("ft")).is_none()
            && let Some(ft) = device.raop.as_ref().and_then(|r| r.get("ft"))
        {
            pairs.push(format!("ft={ft}"));
        }
    }
    pairs.join(" ")
}

/// One line per device for logs and `uzumaki-airplay list`.
pub fn describe(d: &Device) -> String {
    let services = match (&d.airplay, &d.raop) {
        (Some(a), Some(r)) => format!("_airplay:{} _raop:{}", a.port, r.port),
        (Some(a), None) => format!("_airplay:{} (AirPlay 2 only)", a.port),
        (None, Some(r)) => format!("_raop:{} (AirPlay 1 only)", r.port),
        (None, None) => "no services".into(),
    };
    let pw = d.airplay.as_ref().and_then(|s| s.get("pw")).or(d.raop.as_ref().and_then(|s| s.get("pw"))).unwrap_or("false");
    format!("{:<24} {:<20} {:<16} {services} pw={pw}", format!("\"{}\"", d.name), d.model(), d.ip.to_string())
}

#[cfg(test)]
fn service(instance: &str, port: u16, txt: &[(&str, &str)]) -> Service {
    Service { instance: instance.into(), port, txt: txt.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
}

/// A test HomePod mini (made-up addresses).
#[cfg(test)]
pub(crate) fn homepod() -> Device {
    Device {
        name: "Bedroom".into(),
        ip: "192.168.1.50".parse().unwrap(),
        hostname: "Bedroom.local.".into(),
        airplay: Some(service("Bedroom", 7000, &[
            ("acl", "0"),
            ("features", "0x4A7FDFD5,0x3C155FDE"),
            ("model", "AudioAccessory5,1"),
            ("gpn", "Home Theater"),
            ("osvers", "27.2"),
        ])),
        raop: Some(service("AABBCCDDEEFF@Bedroom", 7000, &[("am", "AudioAccessory5,1"), ("cn", "0,1,2,3"), ("et", "0,3,5"), ("ft", "0x4A7FDFD5,0x3C155FDE")])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_instance_without_the_raop_mac() {
        assert_eq!(device_name("Bedroom._airplay._tcp.local.", AIRPLAY), "Bedroom");
        assert_eq!(device_name("A1B2C3D4E5F6@Bedroom._raop._tcp.local.", RAOP), "Bedroom");
        assert_eq!(device_name("Living Room (2)._airplay._tcp.local.", AIRPLAY), "Living Room (2)");
        // An @ that isn't after a MAC stays part of the name.
        assert_eq!(device_name("Me@Home._airplay._tcp.local.", AIRPLAY), "Me@Home");
    }

    #[test]
    fn picks_by_name_ignoring_case() {
        let devices = vec![homepod()];
        assert_eq!(pick(&devices, "bedroom").map(|d| d.ip.to_string()), Some("192.168.1.50".into()));
        assert!(pick(&devices, "Bed").is_none());
    }

    #[test]
    fn txt_skips_pairs_with_spaces_and_borrows_raop_features() {
        let d = homepod();
        assert_eq!(txt_arg(&d), "acl=0 features=0x4A7FDFD5,0x3C155FDE model=AudioAccessory5,1 osvers=27.2");
        let mut bare = homepod();
        bare.airplay.as_mut().unwrap().txt.retain(|(k, _)| k != "features");
        assert!(txt_arg(&bare).ends_with(" ft=0x4A7FDFD5,0x3C155FDE"));
    }

    #[test]
    fn prefers_a_routable_ipv4() {
        let a: Vec<IpAddr> = vec!["fe80::1".parse().unwrap(), "2001:db8::5".parse().unwrap(), "192.168.1.9".parse().unwrap()];
        assert_eq!(best_ip(a.into_iter()), Some("192.168.1.9".parse().unwrap()));
        let b: Vec<IpAddr> = vec!["fe80::1".parse().unwrap(), "2001:db8::5".parse().unwrap()];
        assert_eq!(best_ip(b.into_iter()), Some("2001:db8::5".parse().unwrap()));
    }
}
