//! Every lesson can be finished by doing what it says.

use atcgame::sim::TICK;
use atcgame::sim::tutorial::{LessonKind, Tutorial};

/// Play a lesson the way a player who follows the instructions would: read
/// each page, and do each step every few seconds until it takes.
fn play(kind: LessonKind, max_seconds: f32) -> (Tutorial, atcgame::sim::World) {
    let mut tutorial = Tutorial::new(kind);
    let mut world = tutorial.world();
    let mut since_try = 0.0;
    while !tutorial.finished() {
        assert!(world.time < max_seconds, "{kind:?} stuck at step {:?}: {:?}", tutorial.progress(), tutorial.current().map(|s| s.text));
        tutorial.update(&mut world);
        if tutorial.current().is_some_and(|s| s.read) {
            tutorial.acknowledge();
        } else if since_try >= 5.0 {
            since_try = 0.0;
            tutorial.show_me(&mut world);
        }
        world.tick();
        world.drain_events();
        since_try += TICK;
    }
    (tutorial, world)
}

/// "Show me" only ever gives an instruction the pilot can follow.
fn assert_no_refusals(world: &atcgame::sim::World) {
    let refused: Vec<_> = world.log.iter().filter(|l| l.text.contains("unable")).map(|l| &l.text).collect();
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn the_approach_lesson_can_be_finished() {
    let (_, world) = play(LessonKind::Approach, 1800.0);
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
    assert_no_refusals(&world);
}

#[test]
fn the_local_lesson_can_be_finished() {
    let (_, world) = play(LessonKind::Local, 1800.0);
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
    assert_no_refusals(&world);
}

#[test]
fn the_ground_lesson_can_be_finished() {
    let (_, world) = play(LessonKind::Ground, 1800.0);
    assert!(world.score.incidents.is_empty(), "{:?}", world.score.incidents);
    assert_no_refusals(&world);
}

#[test]
fn an_arrival_left_without_a_landing_clearance_brings_the_step_back() {
    let mut tutorial = Tutorial::new(LessonKind::Local);
    let mut world = tutorial.world();
    // Read the first page, then sit on the landing clearance.
    tutorial.update(&mut world);
    tutorial.acknowledge();
    tutorial.update(&mut world);
    tutorial.update(&mut world);
    assert_eq!(tutorial.progress().0, 2);
    let mut retried = false;
    for _ in 0..(300.0 / TICK) as usize {
        world.tick();
        tutorial.update(&mut world);
        retried |= world.log.iter().any(|l| l.text.contains("try that again"));
    }
    assert!(retried, "the go-around should restart the step");
    assert_eq!(tutorial.progress().0, 2, "back on the landing step");
}
