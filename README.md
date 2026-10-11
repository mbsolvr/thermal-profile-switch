# thermal-profile-switch

A small, standalone Linux tool that switches the system power profile based on
temperature. It reads Linux thermal zones under `/sys/class/thermal` and uses
`powerprofilesctl` to select `balanced` or `power-saver`. It does not depend on
panafanpwr. If panafanpwrd follows system power profiles, it will respond to
these changes as usual.

By default, the tool switches to `balanced` **above** 45°C and to
`power-saver` **below** 40°C. Between the thresholds, it leaves the current
profile unchanged. It checks the temperature every 5 seconds. Outside the
threshold band, it checks the active profile immediately and then at most once
every 60 seconds. It runs `powerprofilesctl set` only when the active profile
differs from the target.

## Requirements

- Linux with readable thermal zones under `/sys/class/thermal`
- A working `powerprofilesctl` command and compatible power profile service
- Rust toolchain to build from source

The Rust program has no third-party crate dependencies.

## Build and try it

```sh
cargo build --release
./target/release/thermal-profile-switch --list-sensors
./target/release/thermal-profile-switch --once --dry-run
./target/release/thermal-profile-switch
```

Automatic sensor selection prefers `x86_pkg_temp`, then `TCPU`, and otherwise
uses the hottest readable thermal zone. Use `--sensor-type TCPU` to select a
zone by type, or `--sensor-path /sys/class/thermal/thermal_zone8/temp` to read
one file. A type is usually more stable than a zone number, which can change
between boots.

Set the thresholds and polling interval with, for example,
`--high 58 --low 52 --poll-seconds 5`. The `--profile-check-seconds 60` option
controls how often the tool rechecks the active profile while the temperature
remains outside the threshold band. A manual profile change in that state may
be overridden at the next recheck. Within the band, manual changes are left
alone until the temperature crosses a threshold.

Use `--once` for a single check and `--dry-run` to preview decisions without
changing the profile. If a sensor or `powerprofilesctl` becomes unavailable,
the tool logs the error and retries on the next polling cycle.

## Install as a user service

Build the binary and install it in `~/.local/bin`. The service then runs
independently of the source and `target/` directories.

**Adjust the thresholds before enabling the service.** The included unit uses
58°C / 52°C as an example for one laptop; these values are not universal. Edit
`ExecStart` in `systemd/thermal-profile-switch.service` to suit your machine.
Running the binary directly without arguments still uses 45°C / 40°C.

```sh
cargo build --release
install -Dm755 target/release/thermal-profile-switch ~/.local/bin/thermal-profile-switch
mkdir -p ~/.config/systemd/user
cp systemd/thermal-profile-switch.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now thermal-profile-switch.service
journalctl --user -fu thermal-profile-switch.service
```

To update the binary, stop the service before replacing it:

```sh
systemctl --user stop thermal-profile-switch.service
cargo build --release
install -Dm755 target/release/thermal-profile-switch ~/.local/bin/thermal-profile-switch
systemctl --user start thermal-profile-switch.service
```

To stop automatic switching and disable the service:

```sh
systemctl --user disable --now thermal-profile-switch.service
```

The tool changes the system-wide active power profile. Other automatic profile
switchers may overwrite its choices, or it may overwrite theirs.

## License

This project is licensed under the [MIT License](LICENSE).
