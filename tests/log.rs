//! What the log panel shows: radio on the positions the player works,
//! landline calls the player is on, and the game's own notices.

use atcgame::sim::positions::{Position, Seats};
use atcgame::sim::{LogEntry, Radio, Speaker, Talker};

fn entry(speaker: Speaker, radio: Option<Radio>) -> LogEntry {
    LogEntry { time: 0.0, speaker, text: String::new(), radio }
}

fn radio(talker: Talker, heard: bool) -> Option<Radio> {
    Some(Radio { talker, heard_by: if heard { Seats::of(0) } else { Seats::NONE } })
}

#[test]
fn radio_on_the_players_positions_is_shown() {
    assert!(entry(Speaker::Controller, radio(Talker::Controller(0), true)).shown());
    assert!(entry(Speaker::Pilot, radio(Talker::Pilot("DAL10".into()), true)).shown());
}

#[test]
fn radio_on_other_positions_is_not() {
    assert!(!entry(Speaker::Controller, radio(Talker::Facility(Position::Ground), false)).shown());
    assert!(!entry(Speaker::Pilot, radio(Talker::Pilot("DAL10".into()), false)).shown());
    assert!(!entry(Speaker::Emergency, radio(Talker::Pilot("DAL10".into()), false)).shown());
}

#[test]
fn landline_calls_the_player_is_on_are_shown() {
    assert!(entry(Speaker::Landline, radio(Talker::Controller(0), true)).shown());
    assert!(entry(Speaker::Landline, radio(Talker::Facility(Position::Ground), true)).shown());
}

#[test]
fn landline_calls_between_automated_positions_are_not() {
    assert!(!entry(Speaker::Landline, radio(Talker::Facility(Position::Ground), false)).shown());
}

#[test]
fn notices_are_shown() {
    assert!(entry(Speaker::System, None).shown());
}
