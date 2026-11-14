//! uzumaki-airplay: checks for AirPlay, without sending any audio.
//!
//!   uzumaki-airplay list [seconds]   AirPlay speakers on the network (mDNS browse only)
//!   uzumaki-airplay check            which cliairplay the app would run, and its version
//!   uzumaki-airplay plan <name>      the cliairplay command the app would run for that speaker
//!                                    runs nothing

use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            match uzumaki_airplay::discover::discover(Duration::from_secs(secs), None) {
                Ok(devices) if devices.is_empty() => println!("no AirPlay speakers seen in {secs} s"),
                Ok(devices) => {
                    devices.iter().for_each(|d| println!("{}", uzumaki_airplay::discover::describe(d)));
                    let rows = uzumaki_airplay::discover::outputs(&devices, &uzumaki_airplay::discover::own_addresses());
                    println!("\nIn the picker:");
                    for (o, _) in rows {
                        println!("  {} ({}{}{})", o.name, o.kind.label(), if o.pair { ", stereo pair" } else { "" }, if o.password { ", password" } else { "" });
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("plan") => {
            let cfg = uzumaki_airplay::Config::from_env(0);
            let Some(target) = args.get(1).cloned() else {
                eprintln!("usage: uzumaki-airplay plan <name>");
                std::process::exit(2);
            };
            let devices = uzumaki_airplay::discover::discover(Duration::from_secs(8), Some(&target)).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let Some(d) = uzumaki_airplay::discover::pick(&devices, &target) else {
                eprintln!("no speaker called \"{target}\" seen");
                std::process::exit(1);
            };
            println!("{}", uzumaki_airplay::discover::describe(d));
            let helper = cfg.helper.map_or("cliairplay".into(), |p| p.display().to_string());
            let a = uzumaki_airplay::helper::args(d, std::path::Path::new("<cmd.fifo>"), &uzumaki_airplay::helper::Options::seeded(cfg.seed, 30));
            let quoted: Vec<String> = a.iter().map(|x| if x.contains(' ') { format!("'{x}'") } else { x.clone() }).collect();
            println!("{helper} {}", quoted.join(" "));
        }
        Some("check") => {
            let path = uzumaki_airplay::helper::find(std::env::var_os("UZUMAKI_CLIAIRPLAY").map(Into::into));
            let Some(path) = path else {
                eprintln!("cliairplay not found: run scripts/airplay/build-cliairplay.sh or set UZUMAKI_CLIAIRPLAY");
                std::process::exit(1);
            };
            println!("{}", path.display());
            let out = std::process::Command::new(&path).arg("--check").output();
            match out {
                Ok(o) => print!("{}", String::from_utf8_lossy(&o.stdout)),
                Err(e) => {
                    eprintln!("could not run it: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("usage: uzumaki-airplay list [seconds] | check | plan <name>");
            std::process::exit(2);
        }
    }
}
