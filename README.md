# Approach Control

A 2D top-down air traffic control game written in Rust with
[macroquad](https://macroquad.rs). You work the approach radar position for a
30-minute shift: accept arrivals from Center, vector them onto the ILS, hand
them to Tower, and climb departures out to their exit fixes, all without
losing separation.

It aims to be a realistic simulator rather than an arcade game. Aircraft fly
real performance limits, pilots take a few seconds to respond and sometimes
reply "unable", and the scope shows only what radar sees on each sweep.

## Airports

| Airport | Data |
|---|---|
| **XHGT** Harrowgate International | Fictional. Two parallel runways, airline traffic that builds through the shift. |
| **KSUS** Spirit of St. Louis | Real runways, STARs, SIDs, ILS approaches and minimum vectoring altitudes from FAA data. Business jets and general aviation at an estimated real-world rate. |

Choose the airport and a traffic level, from Very quiet to Very busy, on the
start screen. The middle level is the airport's normal rate.

## Features

- **STARS-style scope:** position symbols, history trails, timeshared data
  blocks, a predicted track line and a system status area.
- **Safety net:** conflict alerts 60 seconds ahead, loss of separation (3 NM or
  1,000 ft), wake turbulence spacing on final, and low-altitude warnings
  against the MVA.
- **Real procedures at KSUS:** headings are shown and spoken in magnetic,
  departures pick a runway by aircraft type, and arrivals and departures share
  the runway.
- **Realistic callsigns:** airline flight numbers, U.S. registrations that
  follow 14 CFR 47.15, and the occasional foreign business jet.
- **Scoring:** points for landings and clean handoffs, penalties for
  incidents, and a rating at the end. Five separation losses and you're
  decertified; a near collision ends the shift.

## Running it

You need a recent stable [Rust toolchain](https://rustup.rs) (edition 2024).

```sh
cargo run --release
```

## Controls

| Input | Action |
|---|---|
| Click or Tab | Select an aircraft |
| Drag from a target | Assign a heading, or drop on a fix to go direct |
| Left / Right | Heading -5 / +5 (Shift for 1) |
| Up / Down | Altitude +1,000 / -1,000 ft |
| `+` / `-` | Speed +10 / -10 kt |
| C | Clear for the ILS |
| R | Resume the published route |
| T | Accept a handoff, or hand off to Tower or Center |
| G | Go around |
| Enter | Transmit the pending clearance |
| Backspace | Cancel the pending clearance |
| Space | Pause |
| Esc | Pause menu |
| 1 / 2 / 4 | Sim speed |
| M | Toggle the MVA map |
| Mouse wheel, right-drag | Zoom and pan |

Instructions build up as a pending clearance, shown on the scope and in the
side panel, and nothing is said until you transmit.

## Tests

```sh
cargo test --release
python -m unittest discover -s tools
```

The simulation in `src/sim` has no graphics dependency and is deterministic
from a seed, so scenarios run headless.

## Updating the KSUS data

`assets/airports/ksus.toml` is generated; don't edit it by hand. To rebuild it
from a newer FAA cycle:

```sh
python tools/import_faa.py --cycle 261029 --nasr 2026-10-29 --airport KSUS --tracon T75
```

The script uses only the Python standard library and caches downloads in
`tools/.cache`. Values that real data can't supply, such as the sector
boundary, handoff altitudes and traffic rate, live in
`tools/ksus_overrides.toml`.

The taxiway layout in `assets/airports/ksus_ground.toml` comes from
OpenStreetMap. Run it after the FAA import, which supplies the runways and map
origin:

```sh
python tools/import_osm.py --airport KSUS --cycle 261029
```

## Project layout

```
src/sim/      headless simulation: aircraft, autopilot, clearances, tower, traffic, conflicts, scoring
src/ui/       macroquad front end: scope, side panel, input, menus, audio
assets/       airport data files
tests/        scenario tests for each airport
tools/        FAA data importer and its tests
```

## Data notice

**Not for navigation.** KSUS data is derived from FAA CIFP, NASR and MVA
products, simplified for a game and not kept current. See
[DATA_NOTICE.md](DATA_NOTICE.md) for sources and the FAA's required
disclaimers.

KSUS taxiway data © OpenStreetMap contributors, available under the
[ODbL](https://opendatacommons.org/licenses/odbl/).
