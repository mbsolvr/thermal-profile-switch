use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const THERMAL_ROOT: &str = "/sys/class/thermal";

#[derive(Clone)]
enum SensorChoice {
    Auto,
    Type(String),
    Path(PathBuf),
}

struct Options {
    low: f64,
    high: f64,
    poll_seconds: u64,
    high_seconds: u64,
    low_seconds: u64,
    profile_check_seconds: u64,
    sensor: SensorChoice,
    list_sensors: bool,
    dry_run: bool,
    once: bool,
}

struct Reading {
    kind: String,
    path: PathBuf,
    celsius: f64,
}

fn usage() -> &'static str {
    "Usage: thermal-profile-switch [OPTIONS]\n\
     Switch to balanced above 45°C and power-saver below 40°C.\n\n\
     Options:\n\
       --high CELSIUS       Upper threshold (default: 45)\n\
       --low CELSIUS        Lower threshold (default: 40)\n\
       --poll-seconds N     Check interval (default: 5)\n\
       --high-seconds N     Time above --high before balanced (default: 30)\n\
       --low-seconds N      Time below --low before power-saver (default: 120)\n\
       --profile-check-seconds N  Recheck the active profile (default: 60)\n\
       --sensor-type TYPE   Use the thermal zone with this type\n\
       --sensor-path PATH   Read one thermal zone temp file\n\
       --list-sensors       Print readable thermal zones and exit\n\
       --dry-run            Show decisions without changing profiles\n\
       --once               Check once and exit\n\
       -h, --help           Show this help"
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        low: 40.0,
        high: 45.0,
        poll_seconds: 5,
        high_seconds: 30,
        low_seconds: 120,
        profile_check_seconds: 60,
        sensor: SensorChoice::Auto,
        list_sensors: false,
        dry_run: false,
        once: false,
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--high" => {
                options.high = args
                    .next()
                    .ok_or("--high needs a value")?
                    .parse()
                    .map_err(|_| "--high must be a number")?;
            }
            "--low" => {
                options.low = args
                    .next()
                    .ok_or("--low needs a value")?
                    .parse()
                    .map_err(|_| "--low must be a number")?;
            }
            "--poll-seconds" => {
                options.poll_seconds = args
                    .next()
                    .ok_or("--poll-seconds needs a value")?
                    .parse()
                    .map_err(|_| "--poll-seconds must be a positive integer")?;
            }
            "--high-seconds" => {
                options.high_seconds =
                    args.next()
                        .ok_or("--high-seconds needs a value")?
                        .parse()
                        .map_err(|_| "--high-seconds must be a non-negative integer")?;
            }
            "--low-seconds" => {
                options.low_seconds = args
                    .next()
                    .ok_or("--low-seconds needs a value")?
                    .parse()
                    .map_err(|_| "--low-seconds must be a non-negative integer")?;
            }
            "--profile-check-seconds" => {
                options.profile_check_seconds = args
                    .next()
                    .ok_or("--profile-check-seconds needs a value")?
                    .parse()
                    .map_err(|_| "--profile-check-seconds must be a positive integer")?;
            }
            "--sensor-type" => {
                options.sensor =
                    SensorChoice::Type(args.next().ok_or("--sensor-type needs a value")?);
            }
            "--sensor-path" => {
                options.sensor = SensorChoice::Path(PathBuf::from(
                    args.next().ok_or("--sensor-path needs a value")?,
                ));
            }
            "--list-sensors" => options.list_sensors = true,
            "--dry-run" => options.dry_run = true,
            "--once" => options.once = true,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    if !options.low.is_finite() || !options.high.is_finite() || options.low >= options.high {
        return Err("thresholds must be finite and --low must be below --high".into());
    }
    if options.poll_seconds == 0 {
        return Err("--poll-seconds must be greater than zero".into());
    }
    if options.profile_check_seconds == 0 {
        return Err("--profile-check-seconds must be greater than zero".into());
    }
    Ok(options)
}

fn read_temperature(path: &Path) -> Result<f64, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let milli: i64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{}: invalid temperature", path.display()))?;
    if !(1..=150_000).contains(&milli) {
        return Err(format!(
            "{}: implausible temperature {milli}",
            path.display()
        ));
    }
    Ok(milli as f64 / 1000.0)
}

fn thermal_zones() -> Result<Vec<Reading>, String> {
    let entries = fs::read_dir(THERMAL_ROOT).map_err(|e| format!("{THERMAL_ROOT}: {e}"))?;
    let mut readings = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("thermal_zone")
        {
            continue;
        }
        let zone = entry.path();
        let path = zone.join("temp");
        let (Ok(kind), Ok(celsius)) = (
            fs::read_to_string(zone.join("type")),
            read_temperature(&path),
        ) else {
            continue;
        };
        readings.push(Reading {
            kind: kind.trim().to_owned(),
            path,
            celsius,
        });
    }
    readings.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(readings)
}

fn select_reading(choice: &SensorChoice) -> Result<Reading, String> {
    if let SensorChoice::Path(path) = choice {
        return Ok(Reading {
            kind: "explicit path".into(),
            path: path.clone(),
            celsius: read_temperature(path)?,
        });
    }
    let mut readings = thermal_zones()?;
    match choice {
        SensorChoice::Type(kind) => readings.retain(|r| r.kind == *kind),
        SensorChoice::Auto => {
            // Prefer the CPU package sensor; unrelated zones can stay hot at idle.
            for kind in ["x86_pkg_temp", "TCPU"] {
                if readings.iter().any(|r| r.kind == kind) {
                    readings.retain(|r| r.kind == kind);
                    break;
                }
            }
        }
        SensorChoice::Path(_) => unreachable!(),
    }
    readings
        .into_iter()
        .max_by(|a, b| a.celsius.total_cmp(&b.celsius))
        .ok_or_else(|| "no readable matching thermal sensor".into())
}

fn powerprofilesctl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("powerprofilesctl")
        .args(args)
        .output()
        .map_err(|e| format!("cannot run powerprofilesctl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "powerprofilesctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn temperature_target(temperature: f64, options: &Options) -> Option<&'static str> {
    if temperature > options.high {
        Some("balanced")
    } else if temperature < options.low {
        Some("power-saver")
    } else {
        None
    }
}

fn stable_target(
    target: Option<&'static str>,
    pending: &mut Option<(&'static str, Instant)>,
    now: Instant,
    options: &Options,
) -> Option<&'static str> {
    let Some(target) = target else {
        *pending = None;
        return None;
    };
    let required_seconds = if target == "balanced" {
        options.high_seconds
    } else {
        options.low_seconds
    };
    if required_seconds == 0 {
        return Some(target);
    }
    match pending {
        Some((previous, since)) if *previous == target => {
            (now.duration_since(*since) >= Duration::from_secs(required_seconds)).then_some(target)
        }
        _ => {
            *pending = Some((target, now));
            None
        }
    }
}

fn check_once(
    options: &Options,
    pending: &mut Option<(&'static str, Instant)>,
    last_checked: &mut Option<(&'static str, Instant)>,
) -> Result<(), String> {
    let reading = select_reading(&options.sensor)?;
    let raw_target = temperature_target(reading.celsius, options);
    let target = if options.once {
        raw_target
    } else {
        stable_target(raw_target, pending, Instant::now(), options)
    };
    let Some(target) = target else {
        if options.once {
            println!(
                "{:.2}°C ({}): within hysteresis band; keeping current profile",
                reading.celsius, reading.kind
            );
        }
        return Ok(());
    };
    if !options.once
        && last_checked.as_ref().is_some_and(|(previous, checked_at)| {
            *previous == target
                && checked_at.elapsed() < Duration::from_secs(options.profile_check_seconds)
        })
    {
        return Ok(());
    }
    let current = powerprofilesctl(&["get"])?;
    if current == target {
        *last_checked = Some((target, Instant::now()));
        if options.once {
            println!(
                "{:.2}°C ({}): already {target}",
                reading.celsius, reading.kind
            );
        }
        return Ok(());
    }
    if options.dry_run {
        println!(
            "{:.2}°C ({}): would switch {current} -> {target}",
            reading.celsius, reading.kind
        );
    } else {
        powerprofilesctl(&["set", target])?;
        println!(
            "{:.2}°C ({}): switched {current} -> {target}",
            reading.celsius, reading.kind
        );
    }
    *last_checked = Some((target, Instant::now()));
    Ok(())
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    if options.list_sensors {
        for reading in thermal_zones()? {
            println!(
                "{:<20} {:>6.2}°C  {}",
                reading.kind,
                reading.celsius,
                reading.path.display()
            );
        }
        return Ok(());
    }
    let initial = select_reading(&options.sensor)?;
    let mut pending = None;
    let mut last_checked = None;
    if !options.once {
        println!(
            "monitoring {} ({}), balanced above {}°C for {}s, power-saver below {}°C for {}s, every {}s",
            initial.kind,
            initial.path.display(),
            options.high,
            options.high_seconds,
            options.low,
            options.low_seconds,
            options.poll_seconds
        );
    }
    loop {
        if let Err(error) = check_once(&options, &mut pending, &mut last_checked) {
            pending = None;
            if options.once {
                return Err(error);
            }
            eprintln!("thermal-profile-switch: {error}");
        }
        if options.once {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(options.poll_seconds));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("thermal-profile-switch: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options {
        Options {
            low: 50.0,
            high: 60.0,
            poll_seconds: 5,
            high_seconds: 30,
            low_seconds: 120,
            profile_check_seconds: 60,
            sensor: SensorChoice::Auto,
            list_sensors: false,
            dry_run: true,
            once: false,
        }
    }

    #[test]
    fn requires_continuous_time_above_high() {
        let options = options();
        let start = Instant::now();
        let mut pending = None;
        assert_eq!(temperature_target(60.0, &options), None);
        assert_eq!(temperature_target(59.0, &options), None);
        assert_eq!(
            stable_target(Some("balanced"), &mut pending, start, &options),
            None
        );
        assert_eq!(
            stable_target(
                Some("balanced"),
                &mut pending,
                start + Duration::from_secs(29),
                &options
            ),
            None
        );
        assert_eq!(
            stable_target(
                None,
                &mut pending,
                start + Duration::from_secs(30),
                &options
            ),
            None
        );
        assert_eq!(
            stable_target(
                Some("balanced"),
                &mut pending,
                start + Duration::from_secs(31),
                &options
            ),
            None
        );
        assert_eq!(
            stable_target(
                Some("balanced"),
                &mut pending,
                start + Duration::from_secs(61),
                &options
            ),
            Some("balanced")
        );
    }

    #[test]
    fn requires_continuous_time_below_low() {
        let options = options();
        let start = Instant::now();
        let mut pending = None;
        assert_eq!(temperature_target(50.0, &options), None);
        assert_eq!(
            stable_target(Some("power-saver"), &mut pending, start, &options),
            None
        );
        assert_eq!(
            stable_target(
                Some("power-saver"),
                &mut pending,
                start + Duration::from_secs(119),
                &options
            ),
            None
        );
        assert_eq!(
            stable_target(
                Some("power-saver"),
                &mut pending,
                start + Duration::from_secs(120),
                &options
            ),
            Some("power-saver")
        );
    }
}
