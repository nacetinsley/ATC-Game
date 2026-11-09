# Air Traffic Control

A 2D top-down air traffic control game written in Rust with
[macroquad](https://macroquad.rs). Work a shift, an hour by default, as Approach, Tower,
Ground or Clearance Delivery, or combine positions the way a real facility does when it's quiet.
Vector arrivals onto the ILS, RNAV or a visual approach, clear them to land, taxi them to the gate, and
get departures from their IFR clearance at the gate to their exit fix, all without losing
separation or putting two aircraft on one runway.

It aims to be a realistic simulator rather than an arcade game. Aircraft fly
real performance limits, pilots take a few seconds to respond and sometimes
reply "unable", and the scope shows only what radar sees on each sweep.

## Airports

| Airport | Data |
|---|---|
| **XHGT** Harrowgate International | Fictional. Two parallel runways, airline traffic that builds through the shift. |
| **KSUS** Spirit of St. Louis | Real runways, STARs, SIDs, ILS and RNAV (GPS) approaches and minimum vectoring altitudes from FAA data. Business jets and general aviation at an estimated real-world rate. |
| **KMEM** Memphis International | The FedEx superhub, from the same FAA data: four runways with an ILS at every end, nine STARs, RNAV approaches, the Memphis TRACON MVA chart, and the 242 gates and stands mapped in OpenStreetMap: airlines park at the terminal gates, FedEx on its ramps, and general aviation at the FBO. FedEx runs its own ramp tower: Ground taxis freighters to a ramp spot and hands them over (see [Ramp control](#ramp-control)).
 South flow lands 18L/18R and departs 18C; north flow the reverse. FedEx freighters (777F, MD-11F, 767F, A300F, 757F and Caravan feeders) alongside the passenger airlines, at an estimated rate building into a bank. |

Choose the airport and your position on the start screen. Press D (or click
the Difficulty box) to set how hard the shift is:

| Setting | Choices |
|---|---|
| Traffic | Very quiet to Very busy. The middle level is the airport's normal rate. |
| Emergencies | Off, Realistic, Occasional, Frequent or Intense (see Emergencies) |
| Pilot mistakes | Off, Realistic, Occasional or Frequent (see Pilot mistakes) |
| Weather | Good, Variable, Low IFR or Storms (see Weather) |

Relaxed, Realistic, Standard (the default) and Demanding set all four at
once. Realistic is the real job: the airport's normal traffic, emergencies
and pilot mistakes at real-world rates, and variable weather.

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

### Coordination

Controllers also talk to each other on the landline. Their calls are in
purple in the log. You only need the landline between a position you work
and an automated one. On a combined position you're both ends.
- **On Ground, with Tower automated:**
  - Tower owns the runways, so ask before crossing one. Press X once ("Req
    cross X") to call Tower: "Ground to Tower: request crossing runway 27L
    for TRL300".
  - Tower either approves, or says unable with the reason and calls you back
    when the runway is clear.
  - Once it's approved, the crossing shows dashed cyan on the surface map.
    X then gives the crossing clearance as usual.
  - An approval you don't use within 90 seconds lapses, or Tower cancels it
    for traffic on short final.
  - Crossing without approval goes ahead but costs points.
  - An arrival that Tower can't cross straight away after landing comes to
    you holding short between runways, and Tower calls you with the
    crossing when it can.
- **On Local, with Ground automated:**
  - Ground holds aircraft short of your runways and asks you instead of
    sending them over.
  - Requests show above the log, and the strip shows `LL`. Approve with Y,
    or say unable with N.
  - Ground crosses the aircraft itself once you approve, and asks again a
    minute after an unable.
  - Leaving a request unanswered for 45 seconds costs points.
- **Departure releases, on Local with Approach automated:**
  - IFR departures normally go whenever you're ready. Approach puts them on
    call for release while it has a reason: a missed approach to sort out,
    a runway change, an emergency about to land, or storms on the way out.
    "Approach to Tower: departures off 27R call for release, weather."
  - While that's on, the row above the log says so, and K first asks
    Approach for the release ("Req rel K").
  - Approach releases it, void if not off within two minutes, or says hold
    for release and calls you back.
  - A takeoff clearance without the release still goes out, but costs
    points.
  - Approach lets departures go again a few minutes after the reason has
    gone.
- **On Approach, with Tower automated:**
  - F stops departures: Tower then calls you for each one at the hold line.
    Release it with Y, or hold it with N, and Tower asks again a minute
    later.
  - F again, or the "Release all" button, lets them all go.
- **Go-arounds, on Local with Approach automated:**
  - Tell Approach about every IFR go-around. When you order one (G), the
    call goes with it: "Tower to Approach: go-around NVA101, runway 27L,
    runway heading, 3,000."
  - When the pilot goes around on their own (no landing clearance,
    unstable, runway occupied, below minimums), the call is yours to make.
    The strip shows `LL` and a "Call Approach" row appears above the log.
    Press G, with the aircraft selected or nothing selected, or the row's
    Call button.
  - A go-around Approach hasn't heard about within a minute costs points.
- **Go-arounds, on Approach with Tower automated:**
  - Tower calls you about each go-around. Acknowledge with Y; leaving it 45
    seconds costs points.
- **Emergencies, between any position you work and an automated one:**
  - Before an aircraft in an emergency moves on to an automated position,
    tell that position about it. The strip shows `LL`, a "Call" row appears
    above the log, and T makes the call first ("Call T"): "Approach to
    Tower: emergency, NVA101, medical emergency, 120 souls, 1 hour 30
    minutes of fuel, runway 27L." T again hands it off.
  - Handing it off without the call costs points. That includes a radio
    failure that joins the ILS and goes to Tower by itself.
  - An automated position calls you the same way before handing one to
    you. Acknowledge with Y; leaving it 45 seconds costs points.
  - A go-around call to Approach already covers the emergency.

### Line up and wait

W tells a departure to line up and wait (7110.65 3-9-4): "runway 27L, line up
and wait, traffic, A320, 5 mile final." The closest arrival to that runway
within 6 miles is called with it.
- **No landing clearance with traffic in position:** clearing an aircraft to
  land, touch and go or for the option on a runway where a departure is
  lined up, or on its way to line up, costs points. So does lining one up
  behind an arrival already cleared to land.
  - Memphis has runway safety logic (ASDE-X), which allows both, with
    "traffic holding in position" added to the landing clearance. It
    doesn't apply when the ceiling is below 800 ft or the visibility below 2
    miles.
- **Not behind a heavy:** a small aircraft mustn't wait out a departing
  heavy's wake interval on the runway (3-9-6). Hold it short instead.
- **90 seconds at most:** a departure left in position longer calls the
  tower ("still holding in position") and costs a few points. W again
  tells it "continue holding" and starts the time again.

### Tower headings

On Local you can give an IFR departure or a go-around a heading, to keep it
clear of other traffic. It's optional: without one, departures fly runway
heading and go-arounds climb straight out.

- **Departures:** select one at the hold line or lined up and use Left/Right
  (or the panel's Hdg buttons), then send it, alone or with the takeoff
  clearance: "turn left heading 240, runway 27R, cleared for takeoff."
  - It flies runway heading until 400 feet above the field, then turns.
  - Tower can only give a heading within 90 degrees of the runway.
  - Once it's switched to Departure, an automated Approach turns it on
    course. If you work Approach too, it keeps the heading until you
    vector it or send it on its route.
- **Go-arounds:** press G, then Left/Right, to send "go around, turn left
  heading 220." An aircraft already going around can be given one as well.
  The call to Approach passes the heading on: "go-around NVA101, runway 27L,
  heading 220, 3,000."
- A heading on its own doesn't take an arrival off final: the pilot answers
  "unable, on final".

### VFR traffic

On any tower position the start menu has a VFR toggle (V), on by default.
It's set before the shift starts and stays that way until the shift ends. With
it on, light aircraft share the field with the airliners:

- **Closed traffic** taxis out, departs and flies circuits at 1,000 ft above
  the field, doing touch-and-goes until it makes a full stop. It turns
  crosswind past the runway end at 700 ft or higher (AIM 4-3-3).
- **VFR arrivals** call from about 9 miles out on the pattern side and hold
  outside the pattern until you tell them to enter the downwind.
- **VFR departures** climb out ahead, turning on course at pattern altitude
  (AIM 4-3-3), and want a frequency change (T) by 3 miles.

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
at Normal traffic, busy Harrowgate about 4, and Memphis, which shares its
departure runway like Harrowgate, about 2.

## Reading the scope

The radar follows the FAA's STARS displays, as documented for VATSIM's CRC
client, with a few aids kept for playability.

**Tracks and colours**
- The letter on each target is the position that owns it: `A` Approach, `T`
  Tower, `C` Center.
- **Colours:**
  - Your tracks are white and everyone else's green.
  - The block you've selected is cyan, with a box round it.
  - The history trail fades through five blues.
- The predicted track line (one minute) is drawn for your own tracks and the
  selected one.

**Full data block**, for your tracks, inbound handoffs and anything alerting:

```
CA            line 0: alerts (LA, CA, EM, RF), red, blinking until you select the aircraft
NVA101        line 1: callsign; * after it for VFR
050 25H       line 2: Mode C altitude, handoff, ground speed (tens of kt) and category
              timeshared with:  scratchpad  and  aircraft type   e.g.  H270 B738
4.25 A080     line 3: ATPA in-trail distance; the assigned altitude while climbing
              or descending to it
```

- **The middle of line 2** shows who a handoff is to: `C` while Center is being
  offered it, or your own letter while it's being offered to you (the block
  blinks until you accept).
- **After the speed:** `H` heavy, `F` Boeing 757, `V` VFR, `HL` holding.
- **The scratchpad** is filled in as a controller would:
  - `I27L`, `R27R` or `V27L` once cleared for the ILS, RNAV or a visual approach;
  - `H270` for an assigned heading;
  - the fix it's flying to, in three letters;
  - `EFC` near the hold's EFC time;
  - `WX` (amber) when it's asking to deviate.

**Partial data block** (altitude and speed on one line): tracks another
position owns, such as Tower's departures before they call you. Select one to
see it in full. A tower position quick-looks the radar positions, so there
everything shows in full.

**ATPA** (Automated Terminal Proximity Alert) watches each aircraft behind
another on the same final:
- **Cone:** from the follower toward the leader, as long as the in-trail
  minimum (radar or wake).
- **Blue** while it's spaced.
- **Yellow** when the minimum will be lost within 45 seconds.
- **Orange** within 24 seconds, or once it's lost.
- The in-trail distance shows on line 3, in the same colour.

**The status area** at the top left has:
- the time (HHMM/SS);
- the airport, ATIS letter, wind and runways;
- the range and PTL length;
- the conflict alert list (`NVA101*NVA110 CA`);
- MSAW (`LA`);
- active emergency codes in red.

## Reading the surface map

The tower's surface map follows ASDE-X, as VATSIM's CRC client draws it.
- **Map:** teal ground, grey taxiways, black runways and cyan gates. The game
  adds two aids ASDE-X doesn't have: taxiway letters, and a dark red runway
  while it's occupied.
- **Targets:** white aircraft icons pointing along the track. Heavies, Boeing
  757s and supers get the larger heavy icon, in orange.
- **Hold bars** light up green on a runway someone is landing on or departing
  from, and stay faint otherwise.
- **A closed runway** is hatched red, with a white X on each end.

**Data blocks** are green, brighter for the aircraft on your frequency:

```
NVA101          line 1: callsign; altitude in hundreds once airborne
B738 TAXI 27L   line 2: type, then the status, timeshared with the ground
                speed in tens of knots while it's moving
```

The status reads, for example, `TAXI 27L`, `HOLD 27L` (holding short), `RDY 27L` (at the
hold line), `LUAW 27L`, `ROLL 27L`, `ROLLOUT`, `WAIT` (held up by traffic),
or an arrival's runway and distance out.

### Ramp control

At Memphis, FedEx runs its own ramp tower, so Ground doesn't taxi freighters
to a stand. The ramp spots, white boxes labelled `S1` to `S20`, are where its
ramps meet the taxiways.
- **On the map:** the FedEx and FBO ramps are labelled by name (`FEDEX RAMP`,
  `WILSON AIR`) instead of showing each stand. Their lanes are drawn fainter
  and aren't lettered, since Ground doesn't taxi anyone on them. Only the
  terminal gates are still drawn.
- **FedEx arrivals:**
  - They ask for a spot: "Ground, FDX429, clear of the runway, for spot 6".
  - Taxi them there (click the spot). At the spot they go over to FedEx Ramp, which counts as parked.
  - The ramp then picks the stand and brings them in. Ramp traffic is drawn without a data block.
- **FedEx departures:**
  - They call Delivery from the stand as usual, then the ramp pushes them.
  - They call you from a spot, "Ground, FDX218, at spot 6, ready to taxi", and you taxi them as usual.
- **Private aircraft:** go to the FBO ("for Wilson Air"). Taxi them to its spot and the pilots park themselves. Departing, they call "at Wilson Air, ready to taxi".
- **Unchanged:** passenger airlines still taxi to a terminal gate. KSUS and Harrowgate have no ramp control.

The spots are found from OpenStreetMap: the point where a stand's way out
first reaches a lettered taxiway off the aprons. They aren't Memphis's
published spot numbers.

**Safety logic alerts.** ASDE-X alerts on:
- an arrival within 1.5 NM of an occupied or closed runway;
- a departure rolling with the runway ahead occupied, or on a closed runway;
- an aircraft onto a runway without a clearance.

Each aircraft involved gets a red octagon and flashes red. Those involved keep
their full data blocks, and everyone else drops to the callsign. The alert box
at the bottom left lists up to five alerts, each as runway, callsigns and
description (`27L` / `NVA102 NVA101` / `ARRIVAL RWY OCCUPIED`), and the voice
alert sounds.

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

### Cloud, visibility and fog

The Weather setting on the Difficulty screen decides the sky. **Good** is
clear all shift. **Variable** might stay fair, lower through the shift, bring
a fog bank in for 10 to 20 minutes, or bring a line of storms through. **Low
IFR** is overcast at 200 to 600 ft with 1 to 2 miles all shift, and in half
of shifts the fog thickens below minimums for a spell. **Storms** is fair at
the field with thunderstorms about (see below).

The ATIS reports the visibility and ceiling, and a new letter goes out when
the flight category changes. When it isn't VFR, the bottom status line shows
it (`VIS 1/2 FG OVC002 LIFR  LVP`).

| Conditions | What changes |
|---|---|
| IMC: ceiling below 1,000 ft or visibility below 3 miles | No new VFR traffic. VFR departures that haven't reached the runway cancel. Closed traffic makes its next landing a full stop. VFR arrivals already inbound still land. |
| Low-visibility procedures: below 800 ft or 2 miles | The ILS critical area is protected. Anything you put on the arrival runway while an arrival is inside 5 miles bends the localizer, and the arrival goes around: a penalty. The automated Tower waits for 6.5 miles, and the automated Approach spaces arrivals further apart. |
| Below Category I: below 200 ft or 1/2 mile | Only Category III aircraft can land: the airline jets and the Gulfstream. The others refuse an ILS clearance ("below our minimums, request holding"), go around at minimums if they're already on final, and divert after two go-arounds or 20 minutes of waiting. Go-arounds and diversions for the weather cost nothing. The automated Approach holds them and only sends Category III traffic down the final. Aircraft in an emergency land anyway. |

### Thunderstorms

Cells build, drift with the steering wind for half an hour to an hour, and
die away. They show on the scope as STARS weather does: dim for light rain,
brighter for the heavy cores at level 3 and above, stippled at level 5 and
above. They stay at least 12 miles from the field and 6 from the published
holds, so finals and holds stay usable. The ATIS mentions any heavy cell
within 30 miles ("Thunderstorm 18 miles west, moving east").

- **Deviations:** a pilot whose track runs into level 3 or worse within 10
  miles asks to go round ("request 20 left for weather"). The strip shows
  `WX` and the data block flags it. **D** approves it. A heading or direct
  that misses the weather answers it too.
- **Unanswered:** after 30 seconds, or with the storm 3 miles away, the pilot
  goes anyway ("deviating, unable to wait"), and that costs points.
- **Flying it:** the pilot holds a steady heading off its course, turns
  further if that isn't enough, and calls "clear of the weather, proceeding
  direct BRICK" when the way back is clear. A route fix under a storm is
  skipped.
- **Refusals:** a heading or direct into level 3 or worse gets "unable,
  weather ahead".
- **Automated controllers, and Center,** approve deviations on their own
  frequencies.

## Approaches

Press **C** to step through the approaches on offer, then Enter to send it.
The order is ILS, RNAV and visual to the landing runway, then RNAV and
visual to its parallel, and then none. The parallel is the departure runway,
so arrivals there mix with departures, which the automated Tower fits in.
Each landing runway's extended centreline is drawn on the scope. One with a
working ILS has a tick every mile; RNAV and visual only, just the dashes.

| Approach | How it's flown | Needs |
|---|---|---|
| **ILS** | Vectored to intercept the localizer within 30°, or 20° if it would join less than 2 miles outside the approach gate (a mile outside the final approach fix, and at least 5 miles out), 6 to 35 miles out, below the glideslope (7110.65 5-9-2). The clearance gives an altitude to hold until established: "maintain 3,000 until established on the localizer, cleared ILS runway 27L approach." | A runway with an ILS that's working, and Category I weather (or a Category III aircraft below it). |
| **RNAV (GPS)** | Two ways in. **Via a fix:** drag the aircraft to an initial, intermediate or final approach fix, then press C, and the RNAV through that fix comes first ("direct BRICK, cleared RNAV runway 27L approach"). It flies the published fixes, steps down to each fix's at-or-above altitude (never climbing), and joins the final course at the final approach fix. **On vectors:** the same intercept as the ILS, onto the RNAV final. The scope draws the fixes still to fly. | A published RNAV approach (KSUS from the FAA CIFP; XHGT has fictional ones). A 300 ft ceiling and 1 mile visibility. |
| **Visual** | The pilot makes their own way onto final, joining closer in than an ILS allows. It descends steeply if it has to, and may go to Tower as soon as it's cleared (by 2 miles). Too high or not lined up at 1 mile is an unstable go-around. | Visibility of 3 miles or more, a ceiling 500 ft above the MVA, the field in sight (below the cloud and within the visibility, up to 12 miles), and at least 3 miles from the runway. |

**Following traffic:** with an IFR arrival selected, Shift-click another
arrival on an approach: "follow the B738 on final, cleared visual approach
runway 27L". The pilot needs the traffic in sight: ahead of it, and within the
visibility, up to 8 miles. Once it has it, the follower keeps its own
spacing. That's **visual separation**: the pair is exempt from loss of
separation and conflict alerts, though a near collision still counts. The
follower slows when it closes within a mile of its minimum. That minimum is
the wake turbulence minimum where one applies, and otherwise 2 miles.

**Parallel finals** depend on how far apart the centrelines are:

| Spacing | Treated as |
|---|---|
| Under 2,500 ft (KMEM 18L/18C) | One final, for wake spacing and ATPA. |
| 2,500 to 3,600 ft (KSUS 26L/26R, 2,800 ft) | Dependent. Aircraft established on each final need 1 mile diagonally (7110.65 5-9-6). |
| 3,600 ft or more (XHGT, 7,300 ft; KMEM 18L/18R, 4,320 ft) | Independent. No separation is needed between aircraft established on each (7110.65 5-9-7). |

**Wake turbulence** follows the FAA's Consolidated Wake Turbulence (CWT)
categories, A (super) to I (light singles), as listed in FAA Order JO 7360.1K.
The selected aircraft's category shows in the clearance panel (`B738 L CWT F`).
- **On final** (7110.65 5-5-4): 5 miles for an F behind a B such as a 777,
  3.5 behind a C such as a 767 or MD-11, 4 for an I behind a 757 (E), and the
  3-mile radar minimum where no wake minimum applies.
- **Between takeoffs** from the same runway (3-9-6): 2 minutes behind a B or
  D, 2 behind a C for E to I, and 2 for an I behind a 757. A takeoff inside
  the interval costs points; the automated Tower waits it out.
- **Advisories** (3-10-3): takeoff and landing clearances behind such traffic
  end "caution wake turbulence".

**Speeds:** pilots keep to 250 kt below 10,000 ft, and to 200 kt at or below
2,500 ft within 4 miles of a Class C or D airport such as Harrowgate or KSUS
(14 CFR 91.117). Memphis is Class B, where the 250 kt limit applies. A speed
assignment inside 5 miles on final is refused, since none may be given inside
the final approach fix (7110.65 5-7-2). An approach clearance cancels the
assigned speed, and the strip's speed is struck out. The pilot flies 210 kt
until established, then slows on the approach's own schedule, so an arrival
held at 170 kt speeds up again. To keep a speed, give it in the same
transmission as the approach clearance.

**ILS outages:** on Variable or Low IFR weather the landing runway's ILS
sometimes fails for 20 to 40 minutes. A new ATIS reports it, and the status
area shows `ILS 26L OTS`. Arrivals on the ILS that are still with you report
"localizer's gone, request vectors"; those already with Tower go around, at
no cost to you. RNAV takes over: pilots refuse the ILS, and the automated
Approach sends arrivals down the RNAV.

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
- **EFC:** the data block shows `HL` after the speed and the hold fix in the
  scratchpad (`EFC` a couple of minutes before it), and the strip says `EFC` a minute before. At the EFC the pilot asks
  for further clearance; two minutes past it with nothing new costs points.
  A new hold instruction gives a new EFC.
- **Published holds** are drawn faintly on the scope, brighter while
  someone is in one. Harrowgate has holds at its eight feeder fixes; KSUS has
  the seven in the FAA CIFP, FTZ's missed-approach hold among them, and KMEM
its thirteen.
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
  data block's third line shows the code it's really squawking and the one
  assigned, in amber, until Departure
  has it reset at radar contact ("reset transponder, squawk 0415"). When you work Approach
  too, that's your job: select it and press I, then Enter.
- **F4** hides the course hint, for when you'd rather work it out yourself.

With Delivery automated, departures are cleared as they reach the gate, as
filed and without mistakes.

## Emergencies

The Difficulty screen sets how often things go wrong.
Each IFR movement has a chance of an emergency, so busier shifts see more:

| Level | Chance per flight | At Harrowgate, Normal traffic, 30 minutes |
|---|---|---|
| Off | none | never |
| Realistic | 1 in 3,500 (about Newark's rate) | about 1 in 200 shifts |
| Occasional (default) | 1 in 48 | about 1 in 3 shifts |
| Frequent | 1 in 16 | about 1 a shift |
| Intense | 1 in 5 | several a shift |

The pilot declares on your frequency ("Mayday, mayday, mayday...", or
"Pan-pan" for a medical), the alert sounds, and the data block's top line shows
`EM` in red (`RF` for a radio failure, the STARS codes for 7700 and 7600),
blinking until you select the aircraft. The status area lists it in red. The strip has `EMRG ENG`, `EMRG FUEL` and
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
  parallel, a new ATIS goes out, the surface map hatches the closed runway red,
  and the status lines show `RWY 27L/09R CLSD 6:40`. VFR pattern work stops
  while it's closed.

Landing an emergency earns 150 points, or 250 within 8 minutes of declaring.
A go-around costs 150 and a low-fuel diversion 300. The automated Tower holds
departures while an emergency is within 10 miles of landing on the same runway.
The end-of-shift screen lists every emergency and how it ended.

## Pilot mistakes

Pilots get things wrong, on instructions you give them, and only there:
automated controllers aren't affected. The Difficulty screen sets how often:

| Level | Instructions | Taxi clearances | Serious errors |
|---|---|---|---|
| Off | never | never | never |
| Realistic | 1 in 150 | 1 in 80 | 1 in 4,000 |
| Occasional (default) | 1 in 30 | 1 in 15 | 1 in 600 |
| Frequent | 1 in 12 | 1 in 6 | 1 in 150 |

The scope and strips always show what *you* assigned. Mode C shows what the
pilot is really doing, as on a real STARS, so a mistake is something you
spot by watching.

- **Hearback error:** the pilot reads back a wrong altitude (1,000 ft off) or
  heading (10 or 20 degrees off, or the digits swapped), and flies it. Listen
  to the readback. Say it again within a minute, before the aircraft is 300
  ft past your altitude, for 40 points. Missed, it costs 75, and the
  assigned value in the data block turns amber until you put it right: the
  altitude on line 3 (`A050`), or the heading in the scratchpad.
- **Altitude bust:** the readback is right, but the pilot flies 400 to 1,000
  ft through the altitude. Catch it on Mode C once it's 200 ft through and
  restate the altitude for 40 points. If the pilot notices first ("sorry,
  climbing back to 5,000"), it costs 75.
- **Wrong turn:** a taxiing aircraft takes the wrong taxiway at a junction.
  The surface map keeps drawing the route you gave. Re-route it, or tell it
  to hold position, for 40 points. Missed, the pilot stops, asks for
  progressive taxi, and it costs 75. Wrong turns never lead onto a runway.
- **Runway incursion** (rare): a taxiing aircraft crosses a hold line it
  wasn't cleared past. It's logged as a pilot deviation, not your incursion.
  Within 15 seconds, tell it to hold position or send the arrival around for
  150 points. Leave an arrival to find it there and it costs 200.
- **Takeoff without clearance** (rare): an aircraft told to line up and wait
  starts rolling by itself ("rolling"). **G** tells a rolling aircraft to stop
  immediately, which it can below 80 knots. Stopping it, or sending the
  conflicting arrival around, within 10 seconds earns 150. Missed with
  traffic in the way, it costs 200. With nobody about, it's only logged.

**I** on an airborne aircraft says its assigned altitude and heading again,
an easy way to put a hearback error right.

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
| 9 | Route and destination, struck and rewritten if Delivery amends the route. Closed traffic: `CLOSED TFC`, then `2 TG` (touch-and-goes still wanted) or `FS` (full stop) | Altitude and remarks (the STAR). VFR: `VFR` and the side of the field it called from (`VFR NE`) |
| 9a, 9b | Vectors and speeds; direct-to and route amendments | same |
| 9c | The gate it's parked at, while it waits to push | The gate it's assigned, once it has landed |

Blocks 10 to 18 are set by each facility. This game's facility directive:

| Block | Mark |
|---|---|
| 10, 11, 12 | Runway, ATIS letter, taxi route |
| 13 | Ground: `HP` hold position, `HS27L` holding short, `X27L` crossing; Delivery: `CLR` IFR clearance given |
| 14 | Local: `L` cleared to land and `MA` missed approach (from the order's tables), `T→` takeoff, `LUAW`, `TG`, `OPT`, VFR pattern marks |
| 15 | Approach: `ILS`, `RNAV`, `VIS`, or `VIS FOL` following traffic, the approach cleared; `H ALDER 35` holding at ALDER, expect further clearance at 35 past the hour |
| 16 | Time of the block 14 mark |
| 17 | Handoff: the next position, struck through once it has the aircraft |
| 18 | What it's waiting for from you; `LL` is another controller asking about it on the landline |

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
strips fold to one line, and the mouse wheel scrolls it. F3 gives the strip
bay the whole panel.

Hover over a strip to ring the aircraft on the scope. A tooltip shows what
it's doing now (altitude, speed and heading, or its taxi status). Below that
is the block under the mouse: its number and name, what it says, and how to
read it.

## Sound

Most sounds copy the equipment a real controller hears: what triggers each,
how it repeats and how it's silenced.

| Sound | Plays for | Stops |
|---|---|---|
| **Conflict Alert** (STARS) | A conflict or loss of separation involving your traffic. A steady pulsed tone, continuous, as the real CA is | When you select either aircraft (STARS: slew the track), or the conflict clears |
| **Low altitude** (STARS MSAW) | Your aircraft below the minimum vectoring altitude. The same family of tone, modulated differently, as the real ones are | When you select it, or after 10 seconds, when the real aural alarm stops while the LA keeps flashing |
| **Special code** (STARS) | An aircraft squawking 7700 or 7600 | When you select it |
| **ASDE-X** | Tower positions: a voice, "Warning, runway two seven left, arrival, runway occupied" (or "departure"), on a runway incursion, a takeoff with the runway occupied, or an arrival within 1.5 NM with the runway occupied | Repeats every 5 seconds while it lasts |
| **Radio** | Each pilot transmission: the key-up, the carrier and the squelch tail. No roger beep: aviation radios don't have one. With radio voices on, the voice carries these itself | |

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

### Radio voices

Radio calls are spoken, as they'd sound on the frequency:

- **Pilots:** readbacks, check-ins, requests and maydays, when they're on a
  frequency you work. A pilot reading back to an automated Ground isn't
  heard. Each aircraft keeps its own voice: one of four (two of them
  women's, one British), each pitched three ways and spoken at one of three
  paces. A pilot further from the field sounds fainter, with more hiss, and
  now and then keys the mike a moment late, losing the start of a word.
- **You:** every instruction you send, in the controller's voice, as a
  clean sidetone in your headset. With
  "Speak my transmissions" off in the settings, yours aren't said, but the
  frequency stays busy for as long as they'd take, so pilots don't answer
  over you.
- **Landlines:** coordination calls you make or take, on a telephone line
  rather than the radio. The other facility has its own voice.

Automated controllers stay silent, so only your frequencies are heard. One
transmission plays at a time, as on a real frequency. Any that wait more
than 6 seconds are dropped, so the radio keeps up at x2 and x4. The settings
turn radio voices off.

Words in a group (digits, letters, "two seven left", "three thousand")
run together, and other words follow on with only a brief gap, faded
across. The phrases heard most ("descend and maintain", "cleared to land",
"runway two seven left") are said whole, and a word at the end of a
sentence or before a comma has its own clip, with the fall or rise in pitch
it has there.

Each voice is built from word and phrase clips. Callsigns are said with their
telephony ("Nova one zero one", "November three niner six niner uniform"),
runways digit by digit ("two seven left"), and altitudes as controllers say
them ("one two thousand", "flight level two three zero"). A fix with no clip
is spelled phonetically. The telephony comes from FAA JO 7340.2; the game's
own fictional airlines (Nova, Bronze, Trail, Orion, Meridian, Crown) are
added to it.

### Voice clips

All the speech is rendered once with the open-source
[Piper](https://github.com/OHF-Voice/piper1-gpl) speech synthesizer and
committed to `assets/sounds/voice`, so the game plays it the same on every
platform. Only Piper voices trained from scratch on public-domain recordings
are used:

- the ASDE-X words, and one pilot voice: `en_US-ljspeech-high`, trained on the
  [LJ Speech](https://keithito.com/LJ-Speech-Dataset/) recordings;
- the controller: `en_US-norman-medium`, trained on LibriVox recordings;
- three more pilot voices: `en_US-john-medium`, `en_US-kristin-medium` and
  `en_GB-cori-medium`, also trained on LibriVox recordings.

The radio clips are packed one file per voice (`<voice>.pack`): 8 kHz µ-law,
which holds all of a VHF voice's band. The words come from
`assets/sounds/voice/words.txt`, written by the game's tests from every
radio line it can produce: every word, the commonest phrases, and the
words that end sentences and clauses. Words the voices would misread, fix
names mostly, are respelled in `assets/sounds/voice/say.txt`. To render
them again:

```sh
python -m pip install piper-tts onnx
cargo test --release --test voice -- --ignored write_vocabulary
python tools/make_voice.py                # the radio voices
python tools/make_voice.py --voice cori   # just one of them
python tools/make_voice.py --asdex        # the ASDE-X words
```

To hear a change, render the listening set before and after it and compare:

```sh
cargo run --release --example radio_samples -- target/radio_samples/before
cargo run --release --example radio_samples    # into target/radio_samples/current
```

Each transmission is written as a WAV file, with `busy.wav` holding them
all in a row, and `report.txt` with each one's length, words a minute and
longest gap.

To listen through every clip for a word said wrong:

```sh
cargo run --release --example radio_samples -- --vocabulary
```

This writes one WAV per voice to `target/radio_samples/vocabulary`, with
`vocabulary.txt` giving the time each word starts.

## Settings

**O** on the start menu, or the pause menu during a shift, sets:

- **Sound**, **radio voices** and **game cues** (see Sound).
- **UI scale:** 75% to 200%. Everything grows together and text stays sharp.
  Above 100%, a window too small for the layout gets the largest scale that
  fits, and the setting says so ("UI scale: 150% (125% fits)").
- **Colours:** Standard, **Red-green** (deuteranopia and protanopia: the
  greens become cyan, so they stand apart from red) or **Blue-yellow**
  (tritanopia: ambers become pink, greens cyan). Only the colours that carry
  meaning change.

These, and the last choices on the start menu (airport, position, VFR, shift
length and difficulty), are saved between runs in `settings.toml`:
`%APPDATA%tcgame` on Windows, `~/Library/Application Support/atcgame` on a
Mac, `~/.config/atcgame` on Linux. The settings screen shows the path. Delete
the file to go back to the defaults; a value it doesn't recognise is ignored.

## Debrief and replay

When a shift ends, the debrief shows the score and a **timeline** of the
shift: penalties in red, credits in green, emergencies as amber diamonds.
Hover over a mark to read it, and click it (or pick it in the list below with
Up/Down and press Space) to **replay** that moment, starting 45 seconds before.
**R** replays from the start.

The replay is the shift on the scope again, exactly as it happened: the same
aircraft, the same radio calls, with the aircraft the incident names already
selected. You can select, pan and zoom, read the strips, and switch views, but
not give instructions. Along the bottom:

- **Space** plays or pauses, **1 2 4 8** set the speed.
- **Left / Right** step 10 seconds back or forward (Shift: a minute).
  **N / P** jump to the next or previous incident. Click or drag on the
  timeline to go anywhere.
- **T takes over**: the replay becomes your shift again from that moment, and
  what happens next is up to you. Try the incident another way.
- **Esc** goes back to the debrief.

The sim is deterministic, so a replay is only the shift's settings and what you
said: each finished shift is saved, a few KB, in a `replays` folder next to
the settings file. **R** on the start menu lists them (the newest 30 are kept)
to watch again; Delete removes one.

### Shift codes

Every shift has a code, such as `KSUS-7KQ4-XM2P-AB3CD`, on the debrief and at
the foot of the pause menu. It holds the airport, the position, the
difficulty, VFR, the shift length and the random seed. Press **C** on the
debrief to copy it. **C** on the start menu opens a box to type or paste
(Ctrl+V) a code into, and Enter plays that exact traffic, to try again or to
challenge a friend with the same shift. A mistyped code is caught by its
check digits.

## Tutorials

The start menu has a lesson for each position (F5 Approach, F6 Local, F7
Ground, F8 Clearance Delivery). Each puts a few aircraft at Harrowgate and walks you through working
them, one instruction at a time, with a ring around the aircraft or spot it's
about. Press S on any step to have it done for you. If a step goes wrong (an
arrival goes around, say), the lesson takes you back and lets you try again.

## Features

- **STARS scope:** position symbols, full and partial data blocks with
  STARS's timeshared fields, ATPA cones and in-trail distances on final,
  history trails, a predicted track line and a system status area.
- **Safety net:** conflict alerts 60 seconds ahead, loss of separation (3 NM or
  1,000 ft), ATPA wake spacing on final, and low-altitude warnings
  against the MVA.
- **Real procedures at KSUS and KMEM:** headings are shown and spoken in magnetic,
  departures pick a runway by aircraft type, and arrivals and departures share
  the runway.
- **Realistic callsigns:** airline flight numbers, U.S. registrations that
  follow 14 CFR 47.15, and the occasional foreign business jet.
- **Surface movement:** aircraft taxi along the real taxiway graph, stop at
  hold lines without a crossing clearance, queue behind traffic, and turn off
  the runway at the first exit ahead they can stop for, never one that
  doubles back. Departures line up where their taxiway meets the runway, an
  intersection departure if there's enough runway left for the type.
- **Approaches:** ILS, RNAV (GPS) from the published fixes or on vectors,
  and visual approaches, alone or following traffic with visual separation,
  to the landing runway or its parallel, with dependent and independent
  parallel finals and ILS outages.
- **Weather:** wind shifts and runway changes, low cloud and fog with
  low-visibility procedures and Category III, and thunderstorms pilots
  deviate round.
- **Pilot mistakes:** hearback errors, altitude busts, wrong turns, and the
  rare incursion or takeoff without clearance, for you to catch.
- **Spoken radio:** pilots read back and call in their own voices, your
  instructions are spoken in the controller's, and landline calls are heard
  too, all from clips that play the same on every platform.
- **Runway safety:** a landing needs a clearance by 1 NM final and a clear
  runway. Entering a runway with traffic inside 2 NM or rolling is an
  incursion; two aircraft nose to nose on a taxiway is a deadlock.
- **Scoring:** points for landings, parked arrivals, IFR clearances and clean handoffs,
  penalties for incidents, and a rating at the end. You only earn points for
  your own positions' work: a Ground controller isn't credited for the
  landings an automated Tower made. Five separation losses
  and you're decertified; a near collision ends the shift.
- **Debrief and replay:** a timeline of each shift's incidents, a replay of
  any moment you can take over from, saved replays, and shift codes to play
  the same traffic again.

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

Or run the workflow by hand from the Actions tab (Release, then Run workflow)
and enter a version such as `v0.2.0`. It tags the latest commit on the branch
you pick and publishes the release the same way. Untick "Publish a release"
to only build, and download the zips from the run.

The macOS build is a universal app (Apple Silicon and Intel), signed ad hoc
but not notarized.

## Controls

| Input | Action |
|---|---|
| Click or Tab | Select an aircraft |
| Drag from a target | Assign a heading, or drop on a fix to go direct |
| Left / Right | Heading -5 / +5 (Shift for 1). On Local: a departure's heading after takeoff, or a go-around's |
| Up / Down | Altitude +1,000 / -1,000 ft |
| `+` / `-` | Speed +10 / -10 kt |
| C | Step through the approaches: ILS, RNAV, visual, then the parallel's |
| R | Resume the published route |
| T | Accept a handoff, or hand off to the next controller. For an emergency, first tell the next controller on the landline |
| G | Go around. On a takeoff roll: stop immediately. On Local, for a go-around Approach hasn't been told about (or with nothing selected): call Approach |
| Click a hold line, gate, ramp spot or junction | Taxi the selected aircraft there |
| Ctrl-click a taxiway | Add it to the taxi route ("via") |
| H | On the ground: hold position, or continue. In the air: hold at the fix it's flying to |
| Ctrl-drag onto a fix (Cmd on a Mac) | Hold at that fix |
| X | Cross the runway ahead. On Ground with Tower automated: first ask Tower on the landline |
| Y / N | Approve, or say unable to, a landline request (the selected aircraft's, else the oldest) |
| F | Approach with Tower automated: stop departures (call for release), or release them all |
| D | Approve a deviation round weather (`WX` on the strip) |
| I | Delivery: the IFR clearance; again once cleared, repeat the code and altitude. Airborne: say the assigned altitude and heading again, and reset a wrong code |
| Up / Down at Delivery | Amend the altitude to expect |
| S at Delivery | The next SID in use |
| F4 | Delivery: show or hide the course hint |
| W / K | Line up and wait / Cleared for takeoff. On call for release: K first asks Approach for the release |
| L | Cleared to land (a full stop, for VFR) |
| E | VFR: enter the downwind, or extend it once in the pattern |
| B | VFR: turn base now |
| O / U | VFR: cleared for the option / cleared touch-and-go |
| Shift-click an aircraft | VFR: follow that traffic. IFR arrival: follow it on a visual approach |
| F1 / F2 | Radar or surface full size (all combined) |
| F3 | Strip bay fills the side panel |
| Click a strip | Select that aircraft, and silence its alarms |
| Mouse wheel over the strips | Scroll the strip bay |
| Enter | Transmit the pending clearance |
| Backspace | Cancel the pending clearance |
| Space | Pause |
| Esc | Pause menu |
| F5 / F6 / F7 / F8 | Start menu: Approach, Local, Ground or Delivery tutorial |
| D | Start menu: the Difficulty screen (Up/Down a row, Left/Right change it) |
| O | Start menu: Settings (sound, UI scale, colours). The same rows are in the pause menu |
| C | Start menu: play a shift code. Debrief: copy this shift's code |
| R | Start menu: saved replays. Debrief: replay from the start |
| Space, 1 2 4 8, Left / Right, N / P | Replay: play or pause, speed, step 10 s, next or previous incident |
| T | Replay: take over and play on from here |
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

Each of `delivery`, `mistakes` and `visibility` has an ignored long run too,
of 100 shifts or more.

## Updating the KSUS and KMEM data

`assets/airports/ksus.toml` and `kmem.toml` are generated; don't edit them by
hand. To rebuild them from a newer FAA cycle:

```sh
python tools/import_faa.py --cycle 261029 --nasr 2026-10-29 --airport KSUS --tracon T75
python tools/import_faa.py --cycle 261029 --nasr 2026-10-29 --airport KMEM --tracon M03
```

The script uses only the Python standard library and caches downloads in
`tools/.cache`. Values that real data can't supply, such as the sector
boundary, handoff altitudes and traffic rate, live in
`tools/ksus_overrides.toml` and `tools/kmem_overrides.toml`.

The taxiway layouts in `assets/airports/ksus_ground.toml` and
`kmem_ground.toml` come from OpenStreetMap. Run the import after the FAA
import, which supplies the runways and map origin. Memphis needs a bigger box
to take in the whole field:

```sh
python tools/import_osm.py --airport KSUS --cycle 261029
python tools/import_osm.py --airport KMEM --cycle 261029 --box 0.035
```

## Project layout

```
src/sim/      headless simulation: aircraft, autopilot, clearances, positions, ground
              graph and movement, tower, traffic, conflicts, scoring
src/ui/       macroquad front end: radar scope, surface map, side panel, input, menus, audio
assets/       airport data files and the voice clips
tests/        scenario tests for each airport
tools/        FAA and OpenStreetMap importers and their tests
```

## Data notice

**Not for navigation.** KSUS and KMEM data is derived from FAA CIFP, NASR and MVA
products, simplified for a game and not kept current. See
[DATA_NOTICE.md](DATA_NOTICE.md) for sources and the FAA's required
disclaimers.

KSUS and KMEM taxiway data © OpenStreetMap
 contributors, available under the
[ODbL](https://opendatacommons.org/licenses/odbl/).
