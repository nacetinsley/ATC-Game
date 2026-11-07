"""Tests for import_osm. Run with: python -m unittest discover -s tools

These use small made-up layouts (not real OSM data) to pin down the geometry
rules; the generated KSUS file is checked against the game in tests/ksus.rs.
"""

import math
import unittest

from import_faa import FT_PER_NM, ImportError_
from import_osm import (
    HOLD_FT_NARROW,
    HOLD_FT_WIDE,
    Centerline,
    find_parking,
    find_spots,
    is_movement,
    join_gaps,
    insert_holds,
    mapped_parking,
    physical_runways,
    snap_to_runways,
    stand_use,
)


def east_west_runway(hold_ft):
    """A runway along the x axis from -1 to 1 NM."""
    return Centerline("09/27", (-1.0, 0.0), (1.0, 0.0), hold_ft / FT_PER_NM)


def taxiway(name, *points):
    return {"name": name, "osm": 0, "points": list(points)}


class HoldLines(unittest.TestCase):
    def test_hold_goes_at_the_design_distance_on_each_taxiway_off_the_runway(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        connector = taxiway("A1", (0.0, 0.0), (0.0, 0.1), (0.0, 0.2))
        holds = insert_holds([connector], [runway])
        self.assertEqual(len(holds), 1)
        (x, y), name = holds[0]
        self.assertEqual(name, "09/27")
        self.assertAlmostEqual(y * FT_PER_NM, HOLD_FT_WIDE, places=1)
        self.assertIn((x, y), connector["points"], "hold has a vertex on the taxiway")

    def test_both_sides_of_a_crossing_get_a_hold(self):
        runway = east_west_runway(HOLD_FT_NARROW)
        crossing = taxiway("B", (0.3, -0.2), (0.3, 0.0), (0.3, 0.2))
        holds = insert_holds([crossing], [runway])
        offsets = sorted(round(p[1] * FT_PER_NM) for p, _ in holds)
        self.assertEqual(offsets, [-round(HOLD_FT_NARROW), round(HOLD_FT_NARROW)])

    def test_a_taxiway_that_never_leaves_the_runway_area_fails(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        # Runs alongside only 100 ft off the centerline, never reaching the hold distance.
        hugging = taxiway("C", (0.0, 0.0), (0.0, 0.016), (0.5, 0.016), (0.9, 0.016))
        with self.assertRaises(ImportError_):
            insert_holds([hugging], [runway])


class Snapping(unittest.TestCase):
    def test_a_taxiway_crossing_without_a_shared_node_gets_a_runway_vertex(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        crossing = taxiway("D", (0.2, -0.3), (0.2, 0.3))
        snap_to_runways([crossing], [runway])
        self.assertIn((0.2, 0.0), [(round(x, 6), round(y, 6)) for x, y in crossing["points"]])

    def test_a_vertex_near_the_centerline_moves_onto_it(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        near = taxiway("E", (0.4, 30 / FT_PER_NM), (0.4, 0.3))
        snap_to_runways([near], [runway])
        self.assertAlmostEqual(near["points"][0][1], 0.0, places=9)


class Parking(unittest.TestCase):
    def test_dead_ends_inside_an_apron_park_facing_along_the_lane(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        apron = [(-0.5, 0.5), (0.5, 0.5), (0.5, 1.0), (-0.5, 1.0), (-0.5, 0.5)]
        lanes = [
            taxiway("", (0.0, 0.4), (0.0, 0.8)),     # dead end inside the apron, heading north
            taxiway("", (0.0, 0.4), (0.9, 0.4)),     # dead end outside any apron
        ]
        spots = find_parking(lanes, [apron], [runway])
        self.assertEqual([(s["name"], s["at"]) for s in spots], [("P1", (0.0, 0.8))])
        self.assertAlmostEqual(spots[0]["heading"], 0.0, places=6)

    def test_a_mapped_stand_joins_the_taxilane_and_parks_at_its_far_end(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        lane = taxiway("", (-0.5, 0.5), (0.5, 0.5))
        # Drawn from the gate out to the lane, ending 50 ft short of it.
        stand = {"name": "12", "osm": 7, "points": [(0.1, 0.6), (0.1, 0.5 + 50 / FT_PER_NM)]}
        spots, lead_ins = mapped_parking([stand], [lane], [runway])
        self.assertEqual([(s["name"], s["at"]) for s in spots], [("12", (0.1, 0.6))])
        self.assertAlmostEqual(spots[0]["heading"], 0.0, places=6)
        rounded = lambda pts: [(round(x, 9), round(y, 9)) for x, y in pts]
        self.assertIn((0.1, 0.5), rounded(lane["points"]), "the lane gets a vertex where the lead-in leaves")
        self.assertEqual(rounded(lead_ins[0]["points"])[0], (0.1, 0.5))

    def test_stands_far_from_a_taxiway_or_by_a_runway_are_left_out_and_repeats_renamed(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        lane = taxiway("", (-0.5, 0.5), (0.5, 0.5), (0.5, 0.05))
        stands = [
            {"name": "", "osm": 1, "points": [(-0.2, 0.5), (-0.2, 0.6)]},
            {"name": "", "osm": 2, "points": [(0.0, 0.5), (0.0, 0.6)]},
            {"name": "", "osm": 3, "points": [(0.2, 0.7), (0.2, 0.8)]},    # 0.2 NM from the lane
            {"name": "", "osm": 4, "points": [(0.5, 0.05), (0.55, 0.05)]},  # inside the runway's hold line
        ]
        spots, _ = mapped_parking(stands, [lane], [runway])
        self.assertEqual([(s["name"], s["at"]) for s in spots], [("P1", (-0.2, 0.6)), ("P2", (0.0, 0.6))])


    def test_stands_are_gates_at_a_bridge_or_terminal_ga_on_an_fbo_apron_and_ramps_otherwise(self):
        context = {
            "fbo": [[(2.0, 0.0), (3.0, 0.0), (3.0, 1.0), (2.0, 1.0), (2.0, 0.0)]],
            "bridges": [[(0.0, 0.0), (0.0, 0.01)]],
            "terminals": [[(0.5, 0.5), (0.6, 0.5), (0.6, 0.6), (0.5, 0.6), (0.5, 0.5)]],
        }
        self.assertEqual(stand_use([(0.0, 0.02), (0.0, 0.08)], context), "gate")
        self.assertEqual(stand_use([(0.55, 0.65), (0.55, 0.7)], context), "gate")
        self.assertEqual(stand_use([(2.5, 0.5), (2.5, 0.6)], context), "ga")
        self.assertEqual(stand_use([(1.0, 1.0), (1.0, 1.1)], context), "ramp")

    def test_a_stand_past_a_runway_end_is_kept(self):
        runway = east_west_runway(HOLD_FT_WIDE)
        # On the extended centerline, half a mile beyond the end.
        lane = taxiway("", (1.5, -0.2), (1.5, 0.2))
        stand = {"name": "1", "osm": 1, "points": [(1.6, 0.0), (1.52, 0.0)]}
        spots, _ = mapped_parking([stand], [lane], [runway])
        self.assertEqual(len(spots), 1)


class Ramps(unittest.TestCase):
    def test_a_small_gap_straight_ahead_is_joined(self):
        main = taxiway("A", (0.0, 0.0), (1.0, 0.0))
        # Stops 40 ft short of A, pointing at it.
        lane = taxiway("", (0.5, 0.3), (0.5, 40 / FT_PER_NM))
        self.assertEqual(join_gaps([main, lane]), 1)
        self.assertEqual(lane["points"][-1], (0.5, 0.0))
        self.assertIn((0.5, 0.0), main["points"])

    def test_a_line_passing_alongside_or_far_off_is_not_joined(self):
        main = taxiway("A", (0.0, 0.0), (1.0, 0.0))
        # Ends 40 ft from A but runs parallel to it.
        alongside = taxiway("", (0.2, 40 / FT_PER_NM), (0.4, 40 / FT_PER_NM))
        far = taxiway("", (0.7, 0.3), (0.7, 0.1))
        self.assertEqual(join_gaps([main, alongside, far]), 0)

    def test_movement_area_is_faa_named_taxiways_off_the_aprons(self):
        apron = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]
        self.assertTrue(is_movement(taxiway("M5", (2.0, 0.0), (3.0, 0.0)), [apron]))
        self.assertFalse(is_movement(taxiway("CL", (0.2, 0.5), (0.8, 0.5)), [apron]))
        self.assertFalse(is_movement(taxiway("Southeast", (2.0, 0.0), (3.0, 0.0)), [apron]))
        self.assertFalse(is_movement(taxiway("", (2.0, 0.0), (3.0, 0.0)), [apron]))

    def test_stands_share_a_spot_where_their_ramp_meets_the_movement_area(self):
        apron = [(-1.0, 0.1), (1.0, 0.1), (1.0, 1.0), (-1.0, 1.0), (-1.0, 0.1)]
        fbo = [(2.0, 0.1), (3.0, 0.1), (3.0, 1.0), (2.0, 1.0), (2.0, 0.1)]
        lines = [
            taxiway("A", (-1.0, 0.0), (0.0, 0.0), (2.5, 0.0), (3.0, 0.0)),
            taxiway("Ramp", (0.0, 0.0), (0.0, 0.4), (0.0, 0.5)),  # a ramp lane off A
            taxiway("", (0.0, 0.5), (0.1, 0.5)),            # stand lead-ins
            taxiway("", (0.0, 0.4), (-0.1, 0.4)),
            taxiway("", (2.5, 0.0), (2.5, 0.3)),            # into the FBO
        ]
        parking = [
            {"name": "1", "at": (0.1, 0.5), "use": "ramp"},
            {"name": "2", "at": (-0.1, 0.4), "use": "ramp"},
            {"name": "G1", "at": (2.5, 0.3), "use": "ga"},
            {"name": "B1", "at": (0.0, 0.0), "use": "gate"},
        ]
        context = {"fbo": [fbo], "fbo_names": ["Wilson Air"]}
        spots = find_spots(lines, parking, [apron, fbo], context)
        self.assertEqual([(s["name"], s["at"]) for s in spots], [("1", (0.0, 0.0)), ("Wilson Air", (2.5, 0.0))])
        self.assertEqual([p.get("spot") for p in parking], ["1", "1", "Wilson Air", None])
        # The ramp's own lanes are flagged; A, Ground's, isn't.
        self.assertEqual([bool(t.get("ramp")) for t in lines], [False, True, True, True, True])


class Runways(unittest.TestCase):
    def test_width_picks_the_hold_distance(self):
        airport = {"runways": [
            {"id": "08L", "x": 0.0, "y": 1.0, "start_x": 0.0, "start_y": 1.0},
            {"id": "26R", "x": 1.0, "y": 1.0, "start_x": 1.0, "start_y": 1.0},
            {"id": "08R", "x": 0.0, "y": 0.0},
            {"id": "26L", "x": 1.0, "y": 0.0},
        ]}
        lines = {r.name: r for r in physical_runways(airport, {"08L/26R": 23.0, "08R/26L": 46.0})}
        self.assertAlmostEqual(lines["08L/26R"].hold_nm * FT_PER_NM, HOLD_FT_NARROW)
        self.assertAlmostEqual(lines["08R/26L"].hold_nm * FT_PER_NM, HOLD_FT_WIDE)
        self.assertTrue(math.isclose(lines["08R/26L"].length, 1.0))

    def test_a_runway_osm_has_no_width_for_fails(self):
        airport = {"runways": [{"id": "09", "x": 0.0, "y": 0.0}, {"id": "27", "x": 1.0, "y": 0.0}]}
        with self.assertRaises(ImportError_):
            physical_runways(airport, {})


if __name__ == "__main__":
    unittest.main()
