//! AirPlay speakers on the local network, from mDNS (browse only: no connection to any device).
//! A speaker usually shows up twice: `_airplay._tcp` (`<Name>._airplay._tcp.local.`, AirPlay 2) and
//! `_raop._tcp` (`<MAC>@<Name>._raop._tcp.local.`, AirPlay 1). They are merged by name.
//!
//! Two ways in: [`discover`] browses once for a fixed time (the command line tool, reconnecting by
//! name), and [`Browser`] keeps browsing and reports every change, for the picker while it is open.
//! [`outputs`] turns the raw devices into the picker's rows: audio devices only, this computer
//! hidden, a stereo pair shown once.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
                absorb(&mut found, event, ty);
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

/// Applies one mDNS event to the devices found so far (keyed by lowercased name). Returns whether
/// anything changed.
fn absorb(found: &mut BTreeMap<String, Device>, event: mdns_sd::ServiceEvent, ty: &str) -> bool {
    match event {
        mdns_sd::ServiceEvent::ServiceResolved(info) => {
            let name = device_name(info.get_fullname(), ty);
            let Some(ip) = best_ip(info.get_addresses().iter().copied()) else { return false };
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
            let before = d.clone();
            if ty == AIRPLAY {
                d.airplay = Some(service);
                d.ip = ip; // the AirPlay 2 record's address wins
                d.hostname = info.get_hostname().to_string();
            } else {
                d.raop = Some(service);
            }
            *d != before || before.airplay.is_none() && before.raop.is_none()
        }
        mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => {
            let key = device_name(&fullname, ty).to_lowercase();
            let Some(d) = found.get_mut(&key) else { return false };
            if ty == AIRPLAY {
                d.airplay = None;
            } else {
                d.raop = None;
            }
            if d.airplay.is_none() && d.raop.is_none() {
                found.remove(&key);
            }
            true
        }
        _ => false,
    }
}

/// Browses until dropped (or [`Browser::stop`]) and hands every change of the device list to
/// `on_change`, on its own thread. Nothing runs on the network once it has stopped.
pub struct Browser {
    stop: Arc<AtomicBool>,
}

impl Browser {
    pub fn start(on_change: impl Fn(Vec<Device>) + Send + 'static) -> Result<Self, String> {
        let mdns = mdns_sd::ServiceDaemon::new().map_err(|e| format!("mDNS: {e}"))?;
        let rx_airplay = mdns.browse(AIRPLAY).map_err(|e| format!("mDNS browse: {e}"))?;
        let rx_raop = mdns.browse(RAOP).map_err(|e| format!("mDNS browse: {e}"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        std::thread::Builder::new()
            .name("airplay-browse".into())
            .spawn(move || {
                let mut found: BTreeMap<String, Device> = BTreeMap::new();
                // A speaker's two records land a few milliseconds apart: report once they settle.
                let mut dirty: Option<Instant> = None;
                while !quit.load(Ordering::SeqCst) {
                    let mut got = false;
                    for (rx, ty) in [(&rx_airplay, AIRPLAY), (&rx_raop, RAOP)] {
                        while let Ok(event) = rx.try_recv() {
                            got = true;
                            if absorb(&mut found, event, ty) {
                                dirty.get_or_insert_with(Instant::now);
                            }
                        }
                    }
                    if dirty.is_some_and(|t| t.elapsed() >= Duration::from_millis(120)) {
                        dirty = None;
                        on_change(found.values().cloned().collect());
                    }
                    if !got {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                let _ = mdns.shutdown();
            })
            .map_err(|e| e.to_string())?;
        Ok(Browser { stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a device is, for its icon in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    HomePod,
    HomePodMini,
    AppleTv,
    Tv,
    Speaker,
    Computer,
}

impl Kind {
    pub fn of(model: &str, features: Option<u64>) -> Self {
        let m = model.trim();
        if m.starts_with("AudioAccessory5") {
            Kind::HomePodMini
        } else if m.starts_with("AudioAccessory") {
            Kind::HomePod
        } else if m.starts_with("AppleTV") {
            Kind::AppleTv
        } else if m.starts_with("Mac") || m.starts_with("iMac") {
            Kind::Computer
        } else if features.is_some_and(|f| f & 1 != 0) {
            Kind::Tv // bit 0: video
        } else {
            Kind::Speaker
        }
    }

    /// A short word for lists (`uzu airplay list`).
    pub fn label(self) -> &'static str {
        match self {
            Kind::HomePod | Kind::HomePodMini => "HomePod",
            Kind::AppleTv => "Apple TV",
            Kind::Tv => "TV",
            Kind::Speaker => "Speaker",
            Kind::Computer => "Computer",
        }
    }
}

/// One row of the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// Stable for a device (its `deviceid`, else its lowercased name).
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// Two HomePods set up as a stereo pair, shown once.
    pub pair: bool,
    /// The device says it wants a password (`pw=true`).
    pub password: bool,
}

impl Device {
    /// The AirPlay feature bits (`features` on `_airplay`, `ft` on `_raop`): "0xLOW,0xHIGH".
    pub fn features(&self) -> Option<u64> {
        let raw = self.airplay.as_ref().and_then(|s| s.get("features").or(s.get("ft"))).or_else(|| self.raop.as_ref().and_then(|s| s.get("ft")))?;
        let mut parts = raw.split(',').map(|p| u64::from_str_radix(p.trim().trim_start_matches("0x").trim_start_matches("0X"), 16));
        let low = parts.next()?.ok()?;
        let high = parts.next().and_then(Result::ok).unwrap_or(0);
        Some(low | (high << 32))
    }

    fn txt(&self, key: &str) -> Option<&str> {
        self.airplay.as_ref().and_then(|s| s.get(key)).or_else(|| self.raop.as_ref().and_then(|s| s.get(key)))
    }

    pub fn id(&self) -> String {
        match self.txt("deviceid").filter(|v| !v.is_empty()) {
            Some(id) => id.to_lowercase(),
            None => match self.raop.as_ref().and_then(|r| r.instance.split_once('@')) {
                Some((mac, _)) if !mac.is_empty() => mac.to_lowercase(),
                _ => self.name.to_lowercase(),
            },
        }
    }

    pub fn wants_password(&self) -> bool {
        self.txt("pw").is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
    }

    pub fn kind(&self) -> Kind {
        Kind::of(self.model(), self.features())
    }

    /// The id two HomePods of a stereo pair share (`tsid`), if this is one of them.
    fn pair_id(&self) -> Option<&str> {
        matches!(self.kind(), Kind::HomePod | Kind::HomePodMini).then(|| self.txt("tsid")).flatten().filter(|v| !v.is_empty())
    }
}

/// "Living Room (3)" → "Living Room": the suffix the system adds to a second device of one name.
fn plain_name(name: &str) -> &str {
    match name.rsplit_once(" (") {
        Some((base, n)) if n.ends_with(')') && n[..n.len() - 1].chars().all(|c| c.is_ascii_digit()) && !base.is_empty() => base,
        _ => name,
    }
}

/// The picker's rows from the devices seen: those that take audio, without this computer
/// (`own` are its addresses), a stereo pair once (the member that leads it, or the plainest name),
/// sorted by name.
pub fn outputs(devices: &[Device], own: &[IpAddr]) -> Vec<(Output, Device)> {
    let mut rows: Vec<(Output, Device)> = Vec::new();
    for d in devices {
        if own.contains(&d.ip) {
            continue;
        }
        // Bit 9: audio. A device with no feature bits at all is kept (old AirPort Express).
        if d.features().is_some_and(|f| f & (1 << 9) == 0) {
            continue;
        }
        let out = Output { id: d.id(), name: d.name.clone(), kind: d.kind(), pair: false, password: d.wants_password() };
        if let Some(pair) = d.pair_id()
            && let Some((first, first_dev)) = rows.iter_mut().find(|(_, o)| o.pair_id() == Some(pair))
        {
            first.pair = true;
            // The row is the pair's leader (`igl=1`), else the member with the plain name.
            let leads = |x: &Device| x.txt("igl") == Some("1");
            if (leads(d) && !leads(first_dev)) || (leads(d) == leads(first_dev) && d.name.len() < first_dev.name.len()) {
                *first = Output { pair: true, ..out };
                *first_dev = d.clone();
            }
            first.name = plain_name(&first.name).to_string();
            continue;
        }
        rows.push((out, d.clone()));
    }
    rows.sort_by(|a, b| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase()).then(a.0.id.cmp(&b.0.id)));
    rows
}

/// This computer's addresses, to leave it out of the picker (a Mac is an AirPlay receiver too).
pub fn own_addresses() -> Vec<IpAddr> {
    if_addrs::get_if_addrs().map(|v| v.into_iter().map(|i| i.ip()).collect()).unwrap_or_default()
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

    fn dev(name: &str, ip: &str, txt: &[(&str, &str)]) -> Device {
        Device { name: name.into(), ip: ip.parse().unwrap(), hostname: format!("{name}.local."), airplay: Some(service(name, 7000, txt)), raop: None }
    }

    #[test]
    fn kinds_come_from_the_model_and_the_video_bit() {
        assert_eq!(Kind::of("AudioAccessory5,1", None), Kind::HomePodMini);
        assert_eq!(Kind::of("AudioAccessory6,1", None), Kind::HomePod);
        assert_eq!(Kind::of("AppleTV14,1", Some(1)), Kind::AppleTv);
        assert_eq!(Kind::of("TCL 55C745", Some(0x201)), Kind::Tv);
        assert_eq!(Kind::of("Sonos One", Some(0x200)), Kind::Speaker);
        assert_eq!(Kind::of("Mac17,8", Some(0x201)), Kind::Computer);
        assert_eq!(homepod().features(), Some(0x3C155FDE_4A7FDFD5));
    }

    #[test]
    fn rows_hide_this_computer_and_non_audio_and_show_a_pair_once() {
        let audio = ("features", "0x200,0x0");
        let devices = vec![
            dev("Living Room (3)", "192.168.1.11", &[audio, ("model", "AudioAccessory6,1"), ("tsid", "PAIR"), ("deviceid", "AA:AA")]),
            dev("Living Room", "192.168.1.10", &[audio, ("model", "AudioAccessory6,1"), ("tsid", "PAIR"), ("deviceid", "BB:BB")]),
            dev("Bedroom", "192.168.1.50", &[audio, ("model", "AudioAccessory5,1"), ("deviceid", "CC:CC")]),
            dev("Studio", "192.168.1.2", &[audio, ("model", "Mac17,8")]),
            dev("Screen", "192.168.1.60", &[("features", "0x1,0x0"), ("model", "Projector")]),
            dev("Kitchen", "192.168.1.70", &[audio, ("pw", "true")]),
        ];
        let rows = outputs(&devices, &["192.168.1.2".parse().unwrap()]);
        let names: Vec<_> = rows.iter().map(|(o, _)| (o.name.as_str(), o.pair, o.kind)).collect();
        assert_eq!(names, vec![("Bedroom", false, Kind::HomePodMini), ("Kitchen", false, Kind::Speaker), ("Living Room", true, Kind::HomePod)]);
        // The pair's row is the member with the plain name.
        assert_eq!(rows[2].0.id, "bb:bb");
        assert_eq!(rows[2].1.ip.to_string(), "192.168.1.10");
        assert!(rows[1].0.password);
    }

    #[test]
    fn a_pair_row_is_its_leader() {
        let audio = ("features", "0x200,0x0");
        let devices = vec![
            dev("Den", "192.168.1.10", &[audio, ("model", "AudioAccessory6,1"), ("tsid", "P"), ("deviceid", "AA")]),
            dev("Den (2)", "192.168.1.11", &[audio, ("model", "AudioAccessory6,1"), ("tsid", "P"), ("deviceid", "BB"), ("igl", "1")]),
        ];
        let rows = outputs(&devices, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].0.name.as_str(), rows[0].0.id.as_str(), rows[0].0.pair), ("Den", "bb", true));
    }

    #[test]
    fn removed_services_drop_the_device() {
        let mut found = BTreeMap::new();
        found.insert("bedroom".to_string(), homepod());
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceRemoved(RAOP.into(), "AABBCCDDEEFF@Bedroom._raop._tcp.local.".into()), RAOP));
        assert!(found["bedroom"].raop.is_none());
        assert!(absorb(&mut found, mdns_sd::ServiceEvent::ServiceRemoved(AIRPLAY.into(), "Bedroom._airplay._tcp.local.".into()), AIRPLAY));
        assert!(found.is_empty());
    }

    #[test]
    fn prefers_a_routable_ipv4() {
        let a: Vec<IpAddr> = vec!["fe80::1".parse().unwrap(), "2001:db8::5".parse().unwrap(), "192.168.1.9".parse().unwrap()];
        assert_eq!(best_ip(a.into_iter()), Some("192.168.1.9".parse().unwrap()));
        let b: Vec<IpAddr> = vec!["fe80::1".parse().unwrap(), "2001:db8::5".parse().unwrap()];
        assert_eq!(best_ip(b.into_iter()), Some("2001:db8::5".parse().unwrap()));
    }
}
