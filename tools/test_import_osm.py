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
    insert_holds,
    physical_runways,
    snap_to_runways,
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
