# Air Traffic Control

A 2D top-down air traffic control game written in Rust with
[macroquad](https://macroquad.rs). Work a shift, an hour by default, as Approach, Tower,
Ground or Clearance Delivery, or combine positions the way a real facility does when it's quiet.
Vector arrivals onto the ILS, clear them to land, taxi them to the gate, and
get departures from their IFR clearance at the gate to their exit fix, all without losing
separation or putting two aircraft on one runway.

It aims to be a realistic simulator rather than an arcade game. Aircraft fly
real performance limits, pilots take a few seconds to respond and sometimes
reply "unable", and the scope shows only what radar sees on each sweep.

## Airports

| Airport | Data |
|---|---|
| **XHGT** Harrowgate International | Fictional. Two parallel runways, airline traffic that builds through the shift. |
| **KSUS** Spirit of St. Louis | Real runways, STARs, SIDs, ILS approaches and minimum vectoring altitudes from FAA data. Business jets and general aviation at an estimated real-world rate. |

Choose the airport, your position and a traffic level, from Very quiet to
Very busy, on the start screen. The middle level is the airport's normal rate.

The shift is an hour unless you change it. Any whole number of minutes from
1 to 12 hours works: - and + step it by 5 minutes (15 above two hours, 1 with
Shift held), typing a number sets it in minutes, and the buttons offer 15 min
to 4 h. Traffic builds from the start of the shift to its peak at the end, or
after an hour on a longer shift, then holds.

## Positions

| Position | You work | Automated |
|---|---|---|
| **Approach** | The radar: arrivals from Center to final, departures out to Center | Tower and Ground |
| **Local (Tower)** | The runways: landing and takeoff clearances, runway crossings | Ground and Approach |
| **Ground** | The taxiways: gate to hold line, runway exit to gate | Tower and Approach |
| **Clearance Delivery** | IFR clearances for departures at the gate | Ground, Tower and Approach |
| **Ground + Delivery** | The gate and the taxiways, combined | Tower and Approach |
| **Local + Ground** | Both tower positions, combined | Approach |
| **All combined** | Everything | Nothing |

On separate positions you hand each aircraft to the next controller, and a
forgotten handoff costs points. On a combined position there's nobody to hand
to, so it happens silently. Without Approach, arrivals turn up established on
the ILS about 10 miles out. Tower positions get a surface map with a radar
inset; with all of them, F1 and F2 swap which is full size.

### VFR traffic

On any tower position the start menu has a VFR toggle (V), on by default.
It's set before the shift starts and stays that way until the shift ends. With
it on, light aircraft share the field with the airliners:

- **Closed traffic** taxis out, departs and flies circuits at 1,000 ft above
  the field, doing touch-and-goes until it makes a full stop.
- **VFR arrivals** call from about 9 miles out on the pattern side and hold
  outside the pattern until you tell them to enter the downwind.
- **VFR departures** climb out ahead and want a frequency change (T) by 3
  miles.

VFR traffic flies from the parallel normally used for departures. At KSUS
(26R or 08L, as in real life) it gets that runway to itself, and IFR
departures move over to share the arrival runway. Harrowgate is an airline
airport that can't give up a runway, so the jets keep theirs and the light
aircraft share it, with Tower slotting them between departures. Each runway
end flies its pattern on the side
published in the FAA data (right traffic at KSUS on 08R and 26R), which keeps
the circuit on the far side from the IFR runway, and VFR departures turn out
that way too. Pilots self-space on downwind behind whoever
they're told to follow, extend if there's no gap, and won't cut in front of
traffic already on base or final. Arrivals join one at a time: the next waits
in a circle a couple of miles short of the entry point until it's free. You sequence them rather than separate
them, so the only air penalty between VFR aircraft is a near collision. A VFR
aircraft not cleared by short final goes around and rejoins the pattern.
Each airport sets its own VFR rate: KSUS sees about 8 light aircraft an hour
at Normal traffic, busy Harrowgate about 4.

## Weather and runway changes

The wind changes through the shift. It drifts a few degrees and knots at a
time, and in most shifts a front comes through between about a quarter and
three fifths of the way in (8 to 18 minutes of a 30-minute shift), veering the
wind round over a few minutes.

- **ATIS:** a new information letter goes out when the wind moves 30 degrees
  or 5 knots, or the runways change. It's logged ("Information Charlie. Wind
  090 at 14...") and shown beside the wind on every status line. Pilots
  checking in with Tower quote it.
- **Runway change:** once the tailwind on the arrival runway passes 5 knots
  and the other direction is better, a change is announced 4 minutes ahead.
  The status lines show it in amber (`RWY CHG 3:40 ARR 09R DEP 09L`).
- **When it takes effect:** departures not yet cleared for takeoff are given
  the new runway and a SID with the same exit fix. They need re-taxiing: the
  surface tag reads `RE-TAXI 09L`, and on Local you send anyone at the old
  hold back to Ground. Arrivals already cleared for the ILS land on the old
  runway; arrivals still on their STAR switch to the new one. Everyone else
  needs vectoring round to the new final. VFR traffic in the pattern breaks
  off and re-enters for the new runway.
- **Automated Tower** won't release a departure into the new direction while
  an arrival is still landing the old way.

## Holding

Approach (and Tower, for a missed approach) can hold an arrival at a fix.

- **Giving it:** press H to hold at the fix the aircraft is flying to, or its
  next route fix; with neither, it holds at the nearest fix within 5 NM.
  Ctrl-drag (Cmd-drag on a Mac) from the target onto any fix to hold there.
  The racetrack is previewed on the scope before you transmit.
- **Wording:** a published hold is "hold northeast of ALDER as published,
  expect further clearance 1435". At other fixes the pilot is given the
  course: "hold west of TOPAZ on the 090 course inbound, right turns,
  1-minute legs, expect further clearance 1435". The EFC is ten minutes on.
- **Flying it:** the pilot picks a direct, teardrop or parallel entry from
  the heading it arrives on (AIM 5-3-8), slows to the holding speed (200 kt to
  6,000 ft, 230 kt to 14,000 ft), and flies 1-minute legs (1.5 minutes above
  14,000 ft) at a standard-rate turn, correcting for wind. Altitudes are
  yours to assign, so a stack is just aircraft 1,000 ft apart.
- **Leaving it:** a heading, a direct or R. R takes it on along its route
  past the hold fix. Clear it for the approach only after vectoring it out
  of the hold.
- **EFC:** the data block reads `HLD` (and `EFC` a couple of minutes before
  it), and the strip says `EFC` a minute before. At the EFC the pilot asks
  for further clearance; two minutes past it with nothing new costs points.
  A new hold instruction gives a new EFC.
- **Published holds** are drawn faintly on the scope, brighter while
  someone is in one. Harrowgate has holds at its eight feeder fixes; KSUS has
  the seven in the FAA CIFP, FTZ's missed-approach hold among them.
- **Automated Approach:** on Tower positions, when the final is full or the
  runway can't take arrivals, new arrivals wait in the published holds,
  stacked from 5,000 ft, and are sent to final lowest first as room opens.

## Clearance Delivery

With Delivery staffed, every IFR departure files a flight plan and calls from
the gate: "Clearance, NVA101, at G3, IFR to XKST, ready to copy, with
information Juliett." Its strip waits in the CLEARANCE bay, marked `CLNC`.

- **Check the plan.** The side panel shows what it filed: the departure
  route and the cruising altitude, with the magnetic course out by the exit
  fix. About one plan in six has a mistake:
  - **an altitude wrong for the direction of flight** (14 CFR 91.179): odd
    thousands for magnetic courses 0 to 179, even for 180 to 359; above FL410,
    FL410, 450 and 490 eastbound and FL430, 470 and 510 westbound;
  - **a SID that isn't in use** with the current runways, filed from the other
    configuration's list. A runway change can make a plan wrong too.
- **Amend it.** Up and Down step the altitude (1,000 ft, or 2,000 above
  FL410). S picks the next SID in use, leaving by the same exit fix where
  there is one. Nothing changes until you transmit.
- **Clear it.** I builds the clearance, in the 7110.65 order: "cleared to
  XKST airport via MAPLE, TOPAZ, then as filed, maintain 5,000, expect FL350
  one zero minutes after departure, squawk 0415". The strip gets `CLR`, with
  amendments written over the printed altitude and route.
- **Listen to the readback.** About one in ten comes back with the code or
  the initial altitude wrong, and the pilot sets what it heard. Press I again
  and transmit: "I say again, maintain 5,000, squawk 0415". The repeat says
  the same thing whether there was an error or not, so the readback is the
  only clue.
- **Send it to Ground.** T: "contact ground". On Ground + Delivery the pilot
  calls for taxi on its own 30 seconds after the readback, or taxis as soon
  as you give it a route. Ground won't move an aircraft that hasn't been
  cleared ("unable, we don't have our IFR clearance yet").
- **Scoring:** 30 points for each clearance, less 2 a minute for a pilot
  kept waiting more than 3 minutes. Catching a filed mistake earns 40.
  Clearing one costs 100, and a wrong readback left uncorrected costs 75; the
  next controller sorts either out, so the flight still goes.
- **A wrong code you missed shows on the radar.** Once airborne, the aircraft's
  data block shows the code it's really squawking, in amber, until Departure
  has it reset at radar contact ("reset transponder, squawk 0415"). When you work Approach
  too, that's your job: select it and press I, then Enter.
- **F4** hides the course hint, for when you'd rather work it out yourself.

With Delivery automated, departures are cleared as they reach the gate, as
filed and without mistakes.

## Emergencies

The start menu sets how often things go wrong (E steps through the levels).
Each IFR movement has a chance of an emergency, so busier shifts see more:

| Level | Chance per flight | At Harrowgate, Normal traffic, 30 minutes |
|---|---|---|
| Off | none | never |
| Realistic | 1 in 3,500 (about Newark's rate) | about 1 in 200 shifts |
| Occasional (default) | 1 in 48 | about 1 in 3 shifts |
| Frequent | 1 in 16 | about 1 a shift |
| Intense | 1 in 5 | several a shift |

The pilot declares on your frequency ("Mayday, mayday, mayday...", or
"Pan-pan" for a medical), the alert sounds, and the data block shows `EM` in
red (`RDOF` for a radio failure), blinking until you select the aircraft. The strip has `EMRG ENG`, `EMRG FUEL` and
so on written on it in red, and moves to the top of its bay.

- **Medical:** wants priority, and paramedics at the gate. Ground earns a
  bonus for parking it within 4 minutes of landing.
- **Low fuel:** won't slow below 210 knots away from the approach. A second
  go-around and it diverts to its alternate, which costs you.
- **Engine failure or bird strike:** a departure levels off at about 3,000 ft
  above the field, climbs at a third of its normal rate, and asks to come
  back. It refuses high altitudes and high speeds. With Approach automated, it
  turns up on final again a few minutes later.
- **Tower-only positions:** the automated Approach puts an emergency on a
  6-mile final, rather than the usual 10, when there's a gap for it.
- **Pressurization failure** (Approach only): Center hands it over at 16,000
  or 17,000 ft. Soon after, it descends at 6,000 fpm to 10,000 ft (or the MVA)
  and ignores altitude instructions on the way down. Separation lost during
  that descent isn't charged to you.
- **Radio failure:** squawks 7600 and answers nothing. It flies its arrival,
  then joins the ILS by itself. Clearing it to land from Local becomes a
  steady green light; without one it goes around and tries again.
- **Smoke, hydraulic failure or a gear problem:** it lands and stops on the
  runway, which closes for 6 to 10 minutes until the tow clears it. Only at
  airports with a parallel runway. Arrivals and departures move to the
  parallel, a new ATIS goes out, the surface map crosses out the closed runway,
  and the status lines show `RWY 27L/09R CLSD 6:40`. VFR pattern work stops
  while it's closed.

Landing an emergency earns 150 points, or 250 within 8 minutes of declaring.
A go-around costs 150 and a low-fuel diversion 300. The automated Tower holds
departures while an emergency is within 10 miles of landing on the same runway.
The end-of-shift screen lists every emergency and how it ended.

## Flight strips

The side panel is a strip bay of FAA terminal flight progress strips, laid
out as in FAA Order JO 7110.65 (paragraph 2-3-3): departure strips (Form
7230-7.1) and arrival strips (Form 7230-7.2).

```
+--------------+------+---------+--------------------------+----+----+----+
| 1 NVA101   2 | 6    | 8       | 9                        | 10 | 13 | 16 |
| 3 H/B744/L   | 7    | 8a      |                          | 11 | 14 | 17 |
| 4 512  5 2731| 2a   | 8b      |  9a       9b       9c    | 12 | 15 | 18 |
+--------------+------+---------+--------------------------+----+----+----+
```

| Block | Departure | Arrival |
|---|---|---|
| 1, 2, 2a | Callsign, revision number, position it was printed for | same |
| 3 | Heavy indicator, type, equipment suffix (`H/B77W/L`) | same |
| 4, 5 | Computer ID, beacon code | same |
| 6 | Proposed time (`P1432`) | Previous fix (here the origin) |
| 7 | Requested altitude, struck and rewritten if Delivery amends it | Coordination fix |
| 8 | Departure airport | ETA at the coordination fix |
| 9 | Route and destination, struck and rewritten if Delivery amends the route | Altitude and remarks (the STAR) |
| 9a, 9b | Vectors and speeds; direct-to and route amendments | same |

Blocks 10 to 18 are set by each facility. This game's facility directive:

| Block | Mark |
|---|---|
| 10, 11, 12 | Runway, ATIS letter, taxi route |
| 13 | Ground: `HP` hold position, `HS27L` holding short, `X27L` crossing; Delivery: `CLR` IFR clearance given |
| 14 | Local: `L` cleared to land and `MA` missed approach (from the order's tables), `T→` takeoff, `LUAW`, `TG`, `OPT`, VFR pattern marks |
| 15 | Approach: `ILS` cleared for the approach; `H ALDER 35` holding at ALDER, expect further clearance at 35 past the hour |
| 16 | Time of the block 14 mark |
| 17 | Handoff: the next position, struck through once it has the aircraft |
| 18 | What it's waiting for from you |

What the flight data computer printed is in black and stays put. What
controllers write afterwards is in blue pen, struck through when superseded:
a new altitude strikes out the old one. Emergencies are written in red, with
the 7700 or 7600 squawk beside the printed code. Holders are blue for
departures, amber for arrivals, green for VFR and red for an emergency.
Strips on other positions are greyed.

Strips sit in bays in the order a controller works them: INBOUND, ARRIVALS,
FINAL and DEPARTURES on the radar, and ARRIVING, RUNWAY, HOLD SHORT, TAXI OUT,
TAXI IN and PENDING in the tower. A strip waiting for you (an arrival to
accept, a departure ready to taxi or at the hold, a check-in nobody has
answered) has a blinking holder and a word in block 18, and moves up its bay.
One about to cost points (no landing clearance inside 4 miles, or a handoff
due) also gets a blinking amber frame. When the bay is full, other positions'
strips fold to one line, and the mouse wheel scrolls it. Hover over a strip
to ring the aircraft on the scope and see what it's doing now. F3 gives the
strip bay the whole panel.

## Sound

Most sounds copy the equipment a real controller hears: what triggers each,
how it repeats and how it's silenced.

| Sound | Plays for | Stops |
|---|---|---|
| **Conflict Alert** (STARS) | A conflict or loss of separation involving your traffic. A steady pulsed tone, continuous, as the real CA is | When you select either aircraft (STARS: slew the track), or the conflict clears |
| **Low altitude** (STARS MSAW) | Your aircraft below the minimum vectoring altitude. The same family of tone, modulated differently, as the real ones are | When you select it, or after 10 seconds, when the real aural alarm stops while the LA keeps flashing |
| **Special code** (STARS) | An aircraft squawking 7700 or 7600 | When you select it |
| **ASDE-X** | Tower positions: a voice, "Warning, runway two seven left, arrival, runway occupied" (or "departure"), on a runway incursion, a takeoff with the runway occupied, or an arrival inside a mile with the runway occupied | Repeats every 5 seconds while it lasts |
| **Radio** | Each pilot transmission: the key-up, the carrier and the squelch tail. No roger beep: aviation radios don't have one | |

Sources for the real behaviour: [AVweb](https://avweb.com/flight-safety/computer-generated-safety-alerts/)
and the [vNAS STARS documentation](https://docs.virtualnas.net/crc/stars/)
(Conflict Alert continuous until acknowledged; the special-code tone),
[NTSB safety recommendation A-06-44](https://www.ntsb.gov/safety/safety-recs/RecLetters/A06_44_47.pdf)
(MSAW modulated differently from the CA; the aural alarm ends while the
visual alert continues) and [NTSB report AIR-24-01](https://www.ntsb.gov/investigations/AccidentReports/Reports/AIR2401.pdf)
(ASDE-X's "runway occupied" aural alert). The STARS tones' actual
frequencies and waveforms aren't published, so those here are stand-ins that
behave like the real ones. Wake turbulence and taxiway conflicts are shown,
not sounded, as on the real tools.

STARS makes no sound for a handoff. A few **game cues** help anyway: a soft
ding when Center offers you an arrival, a quiet double tick when a strip
starts waiting urgently, and a chime when one of your aircraft lands, parks
or leaves. The pause menu turns game cues off, and sets the volume.

The ASDE-X words are WAV clips in `assets/sounds/voice`, rendered once with
the open-source [Piper](https://github.com/OHF-Voice/piper1-gpl) speech
synthesizer and its `en_US-ljspeech-high` voice, trained on the public-domain
[LJ Speech](https://keithito.com/LJ-Speech-Dataset/) recordings. The game
plays them the same on every platform. To render them again:

```sh
python -m pip install piper-tts
python tools/make_voice.py
```

## Tutorials

The start menu has a lesson for each position (F5 Approach, F6 Local, F7
Ground, F8 Clearance Delivery). Each puts a few aircraft at Harrowgate and walks you through working
them, one instruction at a time, with a ring around the aircraft or spot it's
about. Press S on any step to have it done for you. If a step goes wrong (an
arrival goes around, say), the lesson takes you back and lets you try again.

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
- **Surface movement:** aircraft taxi along the real taxiway graph, stop at
  hold lines without a crossing clearance, queue behind traffic, and turn off
  the runway at the first exit ahead they can stop for, never one that
  doubles back. Departures line up where their taxiway meets the runway, an
  intersection departure if there's enough runway left for the type.
- **Runway safety:** a landing needs a clearance by 1 NM final and a clear
  runway. Entering a runway with traffic inside 2 NM or rolling is an
  incursion; two aircraft nose to nose on a taxiway is a deadlock.
- **Scoring:** points for landings, parked arrivals, IFR clearances and clean handoffs,
  penalties for incidents, and a rating at the end. You only earn points for
  your own positions' work: a Ground controller isn't credited for the
  landings an automated Tower made. Five separation losses
  and you're decertified; a near collision ends the shift.

## Running it

You need a recent stable [Rust toolchain](https://rustup.rs) (edition 2024).

```sh
cargo run --release
```

### macOS app

To build a double-clickable `Air Traffic Control.app` on a Mac:

```sh
cargo install cargo-bundle --locked
cargo bundle --release
```

The app is written to `target/release/bundle/osx/`. Drag it to Applications.
It isn't signed, so the first time, right-click it and choose Open. For an
Apple Silicon and Intel build, add `--target aarch64-apple-darwin` or
`--target x86_64-apple-darwin` (after `rustup target add` for it).

On Windows the release build opens without a console window.

### Releases

Pushing a version tag builds Windows and macOS releases on GitHub Actions
(`.github/workflows/release.yml`) and publishes them on the Releases page:

```sh
git tag v0.2.0
git push origin v0.2.0
```

The macOS build is a universal app (Apple Silicon and Intel), signed ad hoc
but not notarized. To build without publishing, run the workflow by hand from
the Actions tab and download the zips from the run.

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
| T | Accept a handoff, or hand off to the next controller |
| G | Go around |
| Click a hold line, gate or junction | Taxi the selected aircraft there |
| Ctrl-click a taxiway | Add it to the taxi route ("via") |
| H | On the ground: hold position, or continue. In the air: hold at the fix it's flying to |
| Ctrl-drag onto a fix (Cmd on a Mac) | Hold at that fix |
| X | Cross the runway ahead |
| I | Delivery: the IFR clearance; again once cleared, repeat the code and altitude. Airborne: reset a wrong code |
| Up / Down at Delivery | Amend the altitude to expect |
| S at Delivery | The next SID in use |
| F4 | Delivery: show or hide the course hint |
| W / K | Line up and wait / Cleared for takeoff |
| L | Cleared to land (a full stop, for VFR) |
| E | VFR: enter the downwind, or extend it once in the pattern |
| B | VFR: turn base now |
| O / U | VFR: cleared for the option / cleared touch-and-go |
| Shift-click an aircraft | VFR: follow that traffic |
| F1 / F2 | Radar or surface full size (all combined) |
| F3 | Strip bay fills the side panel |
| Click a strip | Select that aircraft, and silence its alarms |
| Mouse wheel over the strips | Scroll the strip bay |
| Enter | Transmit the pending clearance |
| Backspace | Cancel the pending clearance |
| Space | Pause |
| Esc | Pause menu |
| F5 / F6 / F7 / F8 | Start menu: Approach, Local, Ground or Delivery tutorial |
| E | Start menu: how often emergencies happen |
| S | Tutorial: show me |
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
from a seed, so scenarios run headless. A longer run, 600 automated shifts
with emergencies on Intense, is ignored by default:

```sh
cargo test --release --test emergency -- --ignored
```

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
src/sim/      headless simulation: aircraft, autopilot, clearances, positions, ground
              graph and movement, tower, traffic, conflicts, scoring
src/ui/       macroquad front end: radar scope, surface map, side panel, input, menus, audio
assets/       airport data files and the ASDE-X voice clips
tests/        scenario tests for each airport
tools/        FAA and OpenStreetMap importers and their tests
```

## Data notice

**Not for navigation.** KSUS data is derived from FAA CIFP, NASR and MVA
products, simplified for a game and not kept current. See
[DATA_NOTICE.md](DATA_NOTICE.md) for sources and the FAA's required
disclaimers.

KSUS taxiway data © OpenStreetMap contributors, available under the
[ODbL](https://opendatacommons.org/licenses/odbl/).
