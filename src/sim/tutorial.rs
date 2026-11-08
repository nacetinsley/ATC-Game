//! Guided lessons, one per position. Each step places the traffic it needs,
//! says what to do, and waits until the player has done it. A step that goes
//! wrong (an arrival goes around, say) starts again from an earlier step.

use super::aircraft::{Aircraft, Flow, Lateral, Leg, Phase, Taxi, VfrIntent};
use super::airspace::Airport;
use super::clearance::Clearance;
use super::delivery;
use super::ground_map::GroundMap;
use super::performance::aircraft_type;
use super::positions::{Position, Staffing};
use super::units::{Wind, heading_diff, heading_vec};
use super::{Speaker, World, WorldConfig};
use glam::Vec2;

/// Long enough that the shift clock never ends a lesson, seconds.
const LESSON_SECONDS: f32 = 4.0 * 3600.0;
/// The arrival the Approach and Local lessons work.
const ARRIVAL: u32 = 101;
/// The departure the Local lesson clears for takeoff.
const DEPARTURE: u32 = 102;
/// The cargo aircraft that crosses 27L in the Local lesson.
const CARGO: u32 = 103;
/// The arrival the Ground lesson taxis in.
const TAXI_IN: u32 = 104;
/// Exit altitude for the Approach lesson's departure.
const DEPARTURE_ALTITUDE: f32 = 10000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LessonKind {
    Approach,
    Local,
    Ground,
    Delivery,
}

impl LessonKind {
    pub const ALL: [LessonKind; 4] =
        [LessonKind::Approach, LessonKind::Local, LessonKind::Ground, LessonKind::Delivery];

    pub fn label(self) -> &'static str {
        match self {
            LessonKind::Approach => "Approach",
            LessonKind::Local => "Local (Tower)",
            LessonKind::Ground => "Ground",
            LessonKind::Delivery => "Clearance Delivery",
        }
    }

    /// The name on the menu's lesson button.
    pub fn short_label(self) -> &'static str {
        match self {
            LessonKind::Local => "Local",
            LessonKind::Delivery => "Delivery",
            _ => self.label(),
        }
    }

    pub fn staffing(self) -> Staffing {
        match self {
            LessonKind::Approach => Staffing::APPROACH,
            LessonKind::Local => Staffing::LOCAL,
            LessonKind::Ground => Staffing::GROUND,
            LessonKind::Delivery => Staffing::DELIVERY,
        }
    }
}

/// Something on the map to draw the player's eye to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Aircraft(u32),
    Node(usize),
    Point(Vec2),
}

type Check = fn(&World) -> bool;
type Act = fn(&mut World);

pub struct Step {
    pub text: &'static str,
    /// Only something to read: the player moves on with Enter.
    pub read: bool,
    setup: Act,
    done: Check,
    failed: Check,
    /// Step to start again from when this one fails.
    retry_from: usize,
    solve: Act,
    highlight: fn(&World) -> Option<Target>,
}

fn nothing(_: &mut World) {}
fn never(_: &World) -> bool {
    false
}
fn no_target(_: &World) -> Option<Target> {
    None
}

impl Step {
    fn read(text: &'static str) -> Step {
        Step {
            text,
            read: true,
            setup: nothing,
            done: never,
            failed: never,
            retry_from: 0,
            solve: nothing,
            highlight: no_target,
        }
    }

    fn act(text: &'static str, done: Check, solve: Act) -> Step {
        Step { read: false, done, solve, ..Step::read(text) }
    }

    fn setup(mut self, setup: Act) -> Step {
        self.setup = setup;
        self
    }

    fn fails(mut self, failed: Check, retry_from: usize) -> Step {
        self.failed = failed;
        self.retry_from = retry_from;
        self
    }

    fn highlight(mut self, highlight: fn(&World) -> Option<Target>) -> Step {
        self.highlight = highlight;
        self
    }
}

pub struct Tutorial {
    pub kind: LessonKind,
    steps: Vec<Step>,
    step: usize,
    /// The current step's setup has run.
    entered: bool,
    acknowledged: bool,
}

impl Tutorial {
    pub fn new(kind: LessonKind) -> Tutorial {
        let steps = match kind {
            LessonKind::Approach => approach_lesson(),
            LessonKind::Local => local_lesson(),
            LessonKind::Ground => ground_lesson(),
            LessonKind::Delivery => delivery_lesson(),
        };
        Tutorial { kind, steps, step: 0, entered: false, acknowledged: false }
    }

    /// The world a lesson runs in: Harrowgate on the 27s, steady wind, no
    /// traffic but what the lesson places.
    pub fn world(&self) -> World {
        let config = WorldConfig {
            seed: 1,
            traffic: false,
            wind: Some(Wind { from: 270.0, speed: 8.0 }),
            shift_seconds: LESSON_SECONDS,
            staffing: self.kind.staffing(),
            vfr: self.kind == LessonKind::Local,
            ..Default::default()
        };
        World::new(Airport::default_airport(), config)
    }

    pub fn current(&self) -> Option<&Step> {
        self.steps.get(self.step)
    }

    /// Step number (from 1) and how many there are.
    pub fn progress(&self) -> (usize, usize) {
        ((self.step + 1).min(self.steps.len()), self.steps.len())
    }

    pub fn finished(&self) -> bool {
        self.step >= self.steps.len()
    }

    /// Enter on a step that's only for reading.
    pub fn acknowledge(&mut self) {
        if self.current().is_some_and(|s| s.read) {
            self.acknowledged = true;
        }
    }

    /// Do the current step for the player.
    pub fn show_me(&self, world: &mut World) {
        // Wait for the last call to be answered, as a controller would.
        if !world.radio.is_empty() {
            return;
        }
        if let Some(step) = self.current() {
            (step.solve)(world);
        }
    }

    pub fn highlight(&self, world: &World) -> Option<Target> {
        self.current().and_then(|s| (s.highlight)(world))
    }

    /// Run once a frame or tick: set up the current step, and move on when
    /// it's done or back when it's gone wrong.
    pub fn update(&mut self, world: &mut World) {
        let Some(step) = self.steps.get(self.step) else { return };
        if !self.entered {
            self.entered = true;
            (step.setup)(world);
            return;
        }
        if (step.failed)(world) {
            world.say(Speaker::System, "That didn't work out. Let's try that again.".into());
            self.step = step.retry_from;
            self.entered = false;
            return;
        }
        let done = if step.read { self.acknowledged } else { (step.done)(world) };
        if done {
            self.step += 1;
            self.entered = false;
            self.acknowledged = false;
        }
    }
}

// Finding things in the lesson world.

fn map(w: &World) -> &GroundMap {
    w.airport.surface.as_ref().expect("Harrowgate has a ground layout")
}

fn end(w: &World, id: &str) -> usize {
    w.airport.runway_index(id).expect("Harrowgate runway")
}

fn physical(w: &World, id: &str) -> usize {
    map(w).runway_of_end[end(w, id)]
}

fn hold_27r(w: &World) -> usize {
    map(w).departure_hold(end(w, "27R")).expect("27R has a hold line")
}

fn spot(w: &World, name: &str) -> usize {
    let map = map(w);
    map.parking.iter().position(|p| p.name == name).expect("Harrowgate gate")
}

fn spot_node(w: &World, name: &str) -> usize {
    map(w).parking[spot(w, name)].node
}

fn departures(w: &World) -> impl Iterator<Item = &Aircraft> {
    w.aircraft.iter().filter(|a| a.flow == Flow::Departure && a.vfr.is_none())
}

fn first_departure(w: &World) -> Option<&Aircraft> {
    departures(w).min_by_key(|a| a.id)
}

fn newest_departure(w: &World) -> Option<&Aircraft> {
    departures(w).max_by_key(|a| a.id)
}

fn vfr_aircraft(w: &World) -> Option<&Aircraft> {
    w.aircraft.iter().find(|a| a.vfr.is_some())
}

fn target(w: &World, id: u32) -> Option<Target> {
    w.find(id).map(|_| Target::Aircraft(id))
}

fn arrival_target(w: &World) -> Option<Target> {
    target(w, ARRIVAL)
}

fn cargo_target(w: &World) -> Option<Target> {
    target(w, CARGO)
}

fn taxi_in_target(w: &World) -> Option<Target> {
    target(w, TAXI_IN)
}

fn taxi_goal(ac: &Aircraft) -> Option<usize> {
    let taxi = ac.taxi.as_ref()?;
    Some(taxi.path.last().copied().unwrap_or(taxi.node))
}

// Placing traffic.

fn add(w: &mut World, ac: Aircraft) {
    w.aircraft.retain(|a| a.id != ac.id);
    w.add_aircraft(ac);
}

/// A departure through the normal path: queued, then placed at a gate (or the
/// hold line, with nobody working the surface).
fn request_departure(w: &mut World) {
    let route = w.airport.sids.iter().find(|s| s.config == "West").expect("West SID").fixes.clone();
    let route = w.airport.route_indices(&route);
    w.request_departure(aircraft_type("B738").expect("type"), route, DEPARTURE_ALTITUDE);
}

/// A departure that will call Delivery from the gate. Its plan is filed
/// right or, with `wrong`, at an altitude wrong for its direction of flight,
/// and then the pilot reads the code back wrong once.
fn delivery_departure(w: &mut World, wrong: bool) {
    request_departure(w);
    let ac = w.departure_queue.back_mut().expect("just queued");
    let course = delivery::course(&w.airport, &ac.route);
    let route = ac.route.clone();
    let plan = ac.flight_plan.as_mut().expect("IFR departures file a plan");
    let right = delivery::level_for(plan.altitude, course);
    plan.route = route;
    plan.altitude = if wrong { delivery::step_altitude(right, -1) } else { right };
    plan.mishear = Some(wrong);
}

/// The departure at the gate on Delivery's frequency.
fn at_gate(w: &World) -> Option<&Aircraft> {
    departures(w).find(|a| a.on_ground && a.phase == Phase::Parked && a.frequency() == Position::Delivery)
}

/// What a sharp controller clears it for: the filed SID, at an altitude
/// right for its direction.
fn right_clearance(w: &World, ac: &Aircraft) -> Option<Clearance> {
    let plan = ac.flight_plan.as_ref()?;
    let course = delivery::course(&w.airport, &plan.route);
    let altitude = delivery::level_for(plan.altitude, course);
    Some(Clearance::Ifr { route: plan.route.clone(), altitude, squawk: plan.squawk })
}

fn cleared_right(w: &World, ac: &Aircraft) -> bool {
    let cleared = ac.flight_plan.as_ref().and_then(|p| p.cleared.as_ref());
    cleared.is_some_and(|c| delivery::direction_ok(c.altitude, delivery::course(&w.airport, &c.route)))
}

fn gate_target(w: &World) -> Option<Target> {
    at_gate(w).map(|a| Target::Aircraft(a.id))
}

/// Clear the departure at the gate as it should be cleared.
fn clear_at_gate(w: &mut World) {
    if let Some((id, c)) = at_gate(w).and_then(|a| Some((a.id, right_clearance(w, a)?))) {
        w.transmit(id, vec![c]);
    }
}

/// An arrival east of the field, being offered by Center.
fn offered_arrival(w: &mut World) {
    let ty = aircraft_type("B738").expect("type");
    let mut ac = Aircraft::new(ARRIVAL, "NVA101", Flow::Arrival, ty, Vec2::new(30.0, -5.6), 270.0, 7000.0, 210.0);
    ac.phase = Phase::Offered;
    ac.lateral = Lateral::Heading(270.0);
    ac.target_altitude = 7000.0;
    ac.target_speed = 210.0;
    ac.entered_sector = true;
    ac.spawn_time = w.time;
    add(w, ac);
    w.say(Speaker::System, "Center: offering NVA101.".into());
}

/// An arrival on a six mile final for 27L, talking to Tower.
fn final_arrival(w: &mut World) {
    let rwy = end(w, "27L");
    let runway = w.airport.runways[rwy].clone();
    let miles = 6.0;
    let pos = runway.threshold() - heading_vec(runway.heading) * miles;
    let altitude = w.airport.glideslope_altitude(rwy, miles);
    let ty = aircraft_type("B738").expect("type");
    let mut ac = Aircraft::new(ARRIVAL, "NVA101", Flow::Arrival, ty, pos, runway.heading, altitude, 160.0);
    ac.phase = Phase::TowerHandedOff;
    ac.lateral = Lateral::Localizer;
    ac.approach_runway = Some(rwy);
    ac.on_glideslope = true;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.spawn_time = w.time;
    add(w, ac);
    w.pilot_say(Speaker::Pilot, "NVA101".into(), "Tower, NVA101, six mile final runway 27L.".into());
}

/// An aircraft stopped on the ground at a node, talking to Ground.
fn on_the_ground(w: &World, id: u32, callsign: &str, flow: Flow, node: usize, heading: f32) -> Aircraft {
    let ty = aircraft_type("B738").expect("type");
    let pos = map(w).nodes[node].pos;
    let mut ac = Aircraft::new(id, callsign, flow, ty, pos, heading, w.airport.elevation, 0.0);
    ac.on_ground = true;
    ac.phase = Phase::Taxiing;
    ac.taxi = Some(Taxi::at(node));
    ac.surface_freq = Position::Ground;
    ac.msaw_armed = false;
    ac.entered_sector = true;
    ac.freq_since = w.time;
    ac
}

/// A cargo arrival at the ramp south of 27L, heading for a gate: it has to cross.
fn cargo_to_the_terminal(w: &mut World) {
    // On the ramp taxiway, not in the spot: an arrival in a spot has parked.
    let node = map(w).nearest_node(Vec2::new(0.3, -0.85));
    let mut ac = on_the_ground(w, CARGO, "CWN880", Flow::Arrival, node, 0.0);
    ac.parking = Some(spot(w, "G7"));
    add(w, ac);
}

/// A departure on taxiway B, nearly at the 27R hold line, still with Ground.
fn departure_near_the_hold(w: &mut World) {
    let node = map(w).nearest_node(Vec2::new(1.5, 0.4));
    let route = w.airport.sids.iter().find(|s| s.config == "West").expect("West SID").fixes.clone();
    let route = w.airport.route_indices(&route);
    let mut ac = on_the_ground(w, DEPARTURE, "NVA230", Flow::Departure, node, 90.0);
    ac.departure_end = Some(end(w, "27R"));
    ac.exit_fix = route.last().copied();
    ac.exit_altitude = DEPARTURE_ALTITUDE;
    ac.route = route;
    ac.target_altitude = w.airport.departure_initial_altitude;
    ac.target_speed = 250.0;
    add(w, ac);
}

/// An arrival that's just turned off 27L at A2, calling Ground.
fn just_landed(w: &mut World) {
    let node = map(w).nearest_node(Vec2::new(-0.75, -0.4));
    let ac = on_the_ground(w, TAXI_IN, "TRL412", Flow::Arrival, node, 0.0);
    add(w, ac);
    w.pilot_say(Speaker::Pilot, "TRL412".into(), "Ground, TRL412, clear of 27L at A2.".into());
}

// The lessons.

fn approach_lesson() -> Vec<Step> {
    // Steps 1 to 6 work one arrival; if it goes wrong, it starts again at step 1.
    let lost = |w: &World| {
        w.find(ARRIVAL).is_none_or(|a| a.phase == Phase::GoAround || (a.pos.x < 8.0 && a.approach_runway.is_none()))
    };
    vec![
        Step::read(
            "This is the radar scope. Harrowgate is in the middle, landing 27L and departing 27R. \
             Each target shows its callsign, altitude in hundreds of feet, and speed. Press Enter.",
        ),
        Step::act(
            "Center is offering NVA101, east of the field. Select it (click it, or Tab) and press T to \
             accept the handoff. It's yours once it shows white.",
            |w| w.find(ARRIVAL).is_some_and(|a| a.phase != Phase::Offered),
            |w| {
                w.accept_handoff(ARRIVAL);
            },
        )
        .setup(offered_arrival)
        .fails(lost, 1)
        .highlight(arrival_target),
        Step::act(
            "Descend it to 3,000 ft: press Down until the pending clearance says 3,000, then Enter to \
             transmit. It has to be below the glideslope to join the ILS.",
            |w| w.find(ARRIVAL).is_some_and(|a| a.target_altitude <= 3000.0),
            |w| {
                w.transmit(ARRIVAL, vec![Clearance::Altitude(3000.0)]);
            },
        )
        .fails(lost, 1)
        .highlight(arrival_target),
        Step::act(
            "Turn it onto an intercept: heading 290, 20 degrees onto the final approach course. Use Right \
             or drag from the target, then Enter. Intercepts must be 30 degrees or less.",
            |w| {
                w.find(ARRIVAL).is_some_and(|a| match a.lateral {
                    Lateral::Heading(h) => (10.0..=30.0).contains(&heading_diff(270.0, h)),
                    _ => false,
                })
            },
            |w| {
                w.transmit(ARRIVAL, vec![Clearance::Heading(290.0)]);
            },
        )
        .fails(lost, 1)
        .highlight(arrival_target),
        Step::act(
            "Clear it for the ILS: press C, then Enter. It will turn onto the localizer by itself and \
             follow the glideslope down.",
            |w| w.find(ARRIVAL).is_some_and(|a| a.approach_runway.is_some()),
            |w| {
                let rwy = end(w, "27L");
                w.transmit(ARRIVAL, vec![Clearance::ClearedIls(rwy)]);
            },
        )
        .fails(lost, 1)
        .highlight(arrival_target),
        Step::act(
            "Once it's established on the localizer, hand it to Tower: press T, then Enter. Do it before \
             about 5 miles out, or it has to go around.",
            |w| w.find(ARRIVAL).is_none_or(|a| a.phase == Phase::TowerHandedOff || a.on_ground),
            |w| {
                // Tower only takes it once it's on the localizer.
                if w.find(ARRIVAL).is_some_and(|a| matches!(a.phase, Phase::Localizer | Phase::Glideslope)) {
                    w.transmit(ARRIVAL, vec![Clearance::HandoffTower]);
                }
            },
        )
        .fails(|w| w.find(ARRIVAL).is_some_and(|a| a.phase == Phase::GoAround), 1)
        .highlight(arrival_target),
        Step::act(
            "A departure is rolling on 27R. When it checks in, climb it to 10,000 ft, the altitude on its \
             strip: press Up, then Enter.",
            |w| newest_departure(w).is_some_and(|a| a.phase == Phase::Controlled && a.target_altitude >= DEPARTURE_ALTITUDE),
            |w| {
                if let Some(id) = newest_departure(w).filter(|a| a.phase == Phase::Controlled).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::Altitude(DEPARTURE_ALTITUDE)]);
                }
            },
        )
        .setup(request_departure)
        .highlight(|w| newest_departure(w).filter(|a| !a.on_ground).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "Hand it to Center before it leaves the sector near its exit fix: press T, then Enter.",
            |w| newest_departure(w).is_none_or(|a| a.phase == Phase::OfferedToCenter),
            |w| {
                if let Some(id) = newest_departure(w).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::HandoffCenter]);
                }
            },
        )
        .highlight(|w| newest_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::read(
            "That's the job. Keep everyone 3 miles or 1,000 ft apart, more behind heavies on final, and \
             above the minimum vectoring altitude (M shows it). Press Enter to finish.",
        ),
    ]
}

fn local_lesson() -> Vec<Step> {
    vec![
        Step::read(
            "This is the surface map, with a small radar in the corner. You're Tower: you own the runways. \
             Landing 27L (south), departing 27R (north). Press Enter.",
        ),
        Step::act(
            "NVA101 is on a six mile final for 27L. Select it and clear it to land: press L, then Enter. \
             Without a clearance by 1 mile it goes around.",
            |w| w.find(ARRIVAL).is_some_and(|a| a.cleared_to_land),
            |w| {
                let rwy = end(w, "27L");
                w.transmit(ARRIVAL, vec![Clearance::ClearedToLand(rwy)]);
            },
        )
        .setup(final_arrival)
        .fails(|w| w.find(ARRIVAL).is_none_or(|a| a.phase == Phase::GoAround), 1)
        .highlight(arrival_target),
        Step::act(
            "When it has turned off the runway, hand it to Ground: press T, then Enter. Ground taxis it to \
             a gate.",
            |w| w.find(ARRIVAL).is_none_or(|a| a.on_ground && a.frequency() == Position::Ground),
            |w| {
                // Only once it has turned off: still on the runway, it's Tower's.
                if w.find(ARRIVAL).is_some_and(|a| a.on_ground && a.phase == Phase::Taxiing) {
                    w.transmit(ARRIVAL, vec![Clearance::ContactGround]);
                }
            },
        )
        .fails(|w| w.find(ARRIVAL).is_some_and(|a| a.phase == Phase::GoAround), 1)
        .highlight(arrival_target),
        Step::act(
            "Ground is taxiing NVA230 to the 27R hold line, and will send it to you there. When it calls \
             ready, clear it for takeoff: press K, then Enter.",
            |w| first_departure(w).is_some_and(|a| a.rolling() || !a.on_ground),
            |w| {
                let rwy = end(w, "27R");
                if let Some(id) = first_departure(w).filter(|a| a.surface_freq == Position::Local).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::ClearedForTakeoff(rwy)]);
                }
            },
        )
        .setup(departure_near_the_hold)
        .highlight(|w| first_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "Once it's airborne, hand it to Departure before 3 miles: press T, then Enter.",
            |w| first_departure(w).is_none_or(|a| a.phase == Phase::Controlled),
            |w| {
                if let Some(id) = first_departure(w).filter(|a| !a.on_ground).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::ContactDeparture]);
                }
            },
        )
        .highlight(|w| first_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "A cargo aircraft is taxiing from the ramp to the terminal and will stop short of 27L. Nobody \
             crosses a runway without you: Ground asks on the landline. When its request shows above the \
             log, approve it: press Y.",
            |w| w.find(CARGO).is_some_and(|a| a.taxi.as_ref().is_some_and(|t| t.cleared.contains(&physical(w, "27L")))),
            |w| {
                if w.incoming().any(|c| c.aircraft == CARGO) {
                    w.answer_coordination(CARGO, true);
                }
            },
        )
        .setup(cargo_to_the_terminal)
        .highlight(cargo_target),
        Step::act(
            "A light aircraft is calling inbound and will hold outside until you say how to join. Tell it \
             to enter the downwind: press E, then Enter.",
            |w| vfr_aircraft(w).and_then(|a| a.vfr.as_ref()).is_some_and(|v| v.cleared_pattern),
            |w| {
                if let Some((id, end)) = vfr_aircraft(w).and_then(|a| a.vfr.as_ref().map(|v| (a.id, v.end))) {
                    w.transmit(id, vec![Clearance::EnterDownwind(end)]);
                }
            },
        )
        .setup(|w| {
            w.spawn_vfr(VfrIntent::Arrival);
        })
        .highlight(|w| vfr_aircraft(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "VFR pilots fly the pattern themselves; you sequence them and clear every landing. Clear it to \
             land: press L, then Enter.",
            |w| vfr_aircraft(w).is_none_or(|a| a.vfr.as_ref().is_some_and(|v| v.landing.is_some()) || a.on_ground),
            |w| {
                // Once it has joined: on the way in it isn't in the pattern yet.
                let joined = vfr_aircraft(w).filter(|a| {
                    a.phase == Phase::Pattern
                        && a.vfr.as_ref().is_some_and(|v| !matches!(v.leg, Leg::Entry | Leg::Holding | Leg::Departing))
                });
                if let Some((id, end)) = joined.and_then(|a| a.vfr.as_ref().map(|v| (a.id, v.end))) {
                    w.transmit(id, vec![Clearance::ClearedToLand(end)]);
                }
            },
        )
        .highlight(|w| vfr_aircraft(w).map(|a| Target::Aircraft(a.id))),
        Step::read(
            "That's Tower. Keep one aircraft on a runway at a time, nobody onto it with traffic inside \
             2 miles, and 90 seconds between departures. Press Enter to finish.",
        ),
    ]
}

fn ground_lesson() -> Vec<Step> {
    vec![
        Step::read(
            "This is the surface map. You're Ground: you own the taxiways. Tower owns the runways, and \
             nobody crosses one without Tower. Press Enter.",
        ),
        Step::act(
            "A departure at the terminal is ready to taxi. Select it, then click the hold line at the start \
             of 27R (highlighted), then Enter. Its route shows before you send it.",
            |w| first_departure(w).is_some_and(|a| taxi_goal(a) == Some(hold_27r(w))),
            |w| {
                let hold = hold_27r(w);
                if let Some(id) = first_departure(w).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::Taxi { to: hold, via: Vec::new() }]);
                }
            },
        )
        .setup(request_departure)
        .highlight(|w| Some(Target::Node(hold_27r(w)))),
        Step::act(
            "When it stops at the hold line, hand it to Tower: press T, then Enter.",
            |w| first_departure(w).is_none_or(|a| a.surface_freq == Position::Local),
            |w| {
                // As the step says: once it's waiting at the hold line, not before.
                let hold = hold_27r(w);
                let waiting = first_departure(w)
                    .filter(|a| a.taxi.as_ref().is_some_and(|t| t.done() && t.node == hold))
                    .map(|a| a.id);
                if let Some(id) = waiting {
                    w.transmit(id, vec![Clearance::ContactTower]);
                }
            },
        )
        .highlight(|w| first_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "TRL412 has landed and turned off at A2. Taxi it to gate G6: select it, click G6, then Enter.",
            |w| w.find(TAXI_IN).is_some_and(|a| taxi_goal(a) == Some(spot_node(w, "G6"))),
            |w| {
                let to = spot_node(w, "G6");
                w.transmit(TAXI_IN, vec![Clearance::Taxi { to, via: Vec::new() }]);
            },
        )
        .setup(just_landed)
        .highlight(|w| Some(Target::Node(spot_node(w, "G6")))),
        Step::act(
            "Traffic can't pass nose to nose on a taxiway, so you hold one to let another by. Tell TRL412 \
             to hold position: press H, then Enter.",
            |w| w.find(TAXI_IN).is_none_or(|a| a.taxi.as_ref().is_some_and(|t| t.holding)),
            |w| {
                w.transmit(TAXI_IN, vec![Clearance::HoldPosition]);
            },
        )
        .highlight(taxi_in_target),
        Step::act("Now let it carry on: press H again, then Enter.", |w| {
            w.find(TAXI_IN).is_none_or(|a| a.taxi.as_ref().is_some_and(|t| !t.holding))
        }, |w| {
            w.transmit(TAXI_IN, vec![Clearance::ContinueTaxi]);
        })
        .highlight(taxi_in_target),
        Step::act(
            "Another departure is ready. Route it the long way, via taxiways C and B: select it, \
             Ctrl-click C, then Ctrl-click B, then click the 27R hold line, and Enter.",
            |w| {
                let junction = map(w).nearest_node(Vec2::new(-0.6, 0.4));
                newest_departure(w).is_some_and(|a| {
                    taxi_goal(a) == Some(hold_27r(w)) && a.taxi.as_ref().is_some_and(|t| t.path.contains(&junction))
                })
            },
            |w| {
                let hold = hold_27r(w);
                if let Some(id) = newest_departure(w).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::Taxi { to: hold, via: vec!["C".into(), "B".into()] }]);
                }
            },
        )
        .setup(request_departure)
        .highlight(|w| newest_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::read(
            "That's Ground. Keep taxiing traffic flowing, hand departures to Tower at the hold line, and \
             watch for two aircraft meeting head on. Press Enter to finish.",
        ),
    ]
}

fn delivery_lesson() -> Vec<Step> {
    vec![
        Step::read(
            "You're Clearance Delivery. Every IFR departure calls you from its gate for its clearance \
             before Ground will taxi it. Its strip waits in the CLEARANCE bay, marked CLNC. Press Enter.",
        ),
        Step::act(
            "When it calls, select it. Its flight plan is in the side panel: route, altitude, and the \
             course it flies. Eastbound (000 to 179) flies odd thousands, westbound even. This one is \
             right: press I, then Enter, to read it its clearance.",
            |w| first_departure(w).and_then(|a| a.flight_plan.as_ref()).is_some_and(|p| p.is_cleared()),
            clear_at_gate,
        )
        .setup(|w| delivery_departure(w, false))
        .highlight(|w| first_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "The pilot read back the route, altitude and squawk correctly. Send it to Ground, who will \
             taxi it: press T, then Enter.",
            |w| first_departure(w).is_none_or(|a| a.frequency() != Position::Delivery),
            |w| {
                if let Some(id) = first_departure(w).filter(|a| a.frequency() == Position::Delivery).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::ContactGround]);
                }
            },
        )
        .highlight(|w| first_departure(w).map(|a| Target::Aircraft(a.id))),
        Step::act(
            "The next one filed an altitude that's wrong for its direction of flight. Check its course, \
             press Up or Down until the altitude is right, then I and Enter. Cleared it wrong? Fix the \
             altitude and press I again: that's an amended clearance.",
            |w| at_gate(w).is_some_and(|a| cleared_right(w, a)),
            clear_at_gate,
        )
        .setup(|w| delivery_departure(w, true))
        // Sent to Ground cleared wrong: another one files, to try again.
        // (Not while the next one is still on its way to the gate.)
        .fails(
            |w| {
                at_gate(w).is_none()
                    && w.departure_queue.is_empty()
                    && newest_departure(w).is_some_and(|a| a.frequency() != Position::Delivery && !cleared_right(w, a))
            },
            3,
        )
        .highlight(gate_target),
        Step::act(
            "Listen to that readback: the pilot got the squawk wrong. Compare it with the code in the \
             side panel. Press I again, then Enter, to say the altitude and code again.",
            // Put right, or sent on without it: Ground sorts it out then.
            |w| newest_departure(w).and_then(|a| a.flight_plan.as_ref()).is_some_and(|p| p.readback.is_none()),
            |w| {
                let cleared = at_gate(w).and_then(|a| {
                    let c = a.flight_plan.as_ref()?.cleared.as_ref()?;
                    Some((a.id, Clearance::Ifr { route: c.route.clone(), altitude: c.altitude, squawk: c.squawk }))
                });
                if let Some((id, c)) = cleared {
                    w.transmit(id, vec![c]);
                }
            },
        )
        .highlight(gate_target),
        Step::act(
            "Read back right this time. Send it to Ground: press T, then Enter.",
            |w| at_gate(w).is_none(),
            |w| {
                if let Some(id) = at_gate(w).map(|a| a.id) {
                    w.transmit(id, vec![Clearance::ContactGround]);
                }
            },
        )
        .highlight(gate_target),
        Step::read(
            "That's Clearance Delivery. Check every altitude against its course, and every SID against \
             the runways in use (S picks another). Listen to every readback, and don't keep pilots \
             waiting more than 3 minutes. Press Enter to finish.",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lesson_starts_with_something_to_read_and_ends_with_a_summary() {
        for kind in LessonKind::ALL {
            let t = Tutorial::new(kind);
            assert!(t.steps.first().unwrap().read, "{kind:?}");
            assert!(t.steps.last().unwrap().read, "{kind:?}");
            for (i, step) in t.steps.iter().enumerate() {
                assert!(step.retry_from <= i, "{kind:?} step {i} retries forward");
            }
        }
    }
}
