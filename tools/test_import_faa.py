"""Importer tests, using real KSUS records from FAA CIFP cycle 2611 as fixtures.

Run from the repository root: python -m unittest discover tools
"""

import math
import unittest
import xml.etree.ElementTree as ET

import import_faa as faa

AIRPORT = "SUSAP KSUSK3ASUS     0     074YHN38394362W090390737W001000463         1800018000C    MNAR    SPIRIT OF ST LOUI"
RUNWAY_26L = "SUSAP KSUSK3GRW26L   0074862590 N38394201W090380869         +0109600462048155150IIFZU1                        "
ILS_26L = "SUSAP KSUSK3IIFZU1   011110RW26LN38392557W0903947962591N38393737W0903820981044 10510498300W00105500458        "
FM_LEG = "SUSAP KSUSK3ESLVER16RW26B 030EMKAAK3EA0EE      FM SPI K5      204808163460    D                     "
DME_ONLY = "SUSAD        CSX   K3011645 DLW                    CSX N38450988W090213946W00190" + " " * 30
NASR_26L = (
    "RWY" + "12082.5*A  " + "MO" + "08R/26L" + " 7486 150CONC-G      GRVD PCR/310 /R/"
    "C/W/THIGH 08R078ILS       YPIR  G38-39-27.7049N 139167.7049N090-39-35.0903W326375.0903W  461.1 553.00"
    "                                                                   461.9V2R  T   MALSR    YN               PIR  50"
    "                 26L258ILS        PIR  G38-39-42.9923N 139182.9923N090-38-02.7554W326282.7554W"
)
HM_TIMED = "SUSAP KSUSK3FI08R  I      060FTZ  K3D 0VE  R   HM                     1000T010    + 02400                     "
HF_DISTANCE = "SUSAP KSUSK3FR08L  AEXESS 010EXESSK3PC0EE AR   HF                     07890040    + 03000     18000           "
HM_DISTANCE = "SUSAP KSUSK3FR08L  R      080EXESSK3PC0EE  L   HM                     12000050    + 04000                     "
ORIGIN = faa.parse_airport(AIRPORT)["pos"]


class Decoding(unittest.TestCase):
    def test_coordinate(self):
        lat, lon = faa.parse_coord("N38394201W090380869")
        self.assertAlmostEqual(lat, 38 + 39 / 60 + 42.01 / 3600, places=6)
        self.assertAlmostEqual(lon, -(90 + 38 / 60 + 8.69 / 3600), places=6)

    def test_bad_coordinate_is_rejected(self):
        with self.assertRaises(faa.ImportError_):
            faa.parse_coord("                   ")

    def test_airport(self):
        airport = faa.parse_airport(AIRPORT)
        self.assertEqual(airport["elevation"], 463)
        self.assertEqual(airport["variation"], -1.0)

    def test_runway_and_displaced_threshold(self):
        runway = faa.parse_runway(RUNWAY_26L)
        self.assertEqual(runway["id"], "26L")
        self.assertEqual(runway["length_ft"], 7486)
        self.assertEqual(runway["displaced_ft"], 481)

    def test_ils_glideslope_and_crossing_height(self):
        ils = faa.parse_ils(ILS_26L)
        self.assertEqual(ils["runway"], "26L")
        self.assertEqual(ils["glideslope_deg"], 3.0)
        self.assertEqual(ils["tch"], 55)

    def test_star_vector_leg(self):
        leg = faa.parse_leg(FM_LEG)
        self.assertEqual(leg["procedure"], "SLVER1")
        self.assertEqual(leg["transition"], "RW26B")
        self.assertEqual(leg["fix"], "EMKAA")
        self.assertEqual(leg["path"], "FM")
        self.assertEqual(leg["course"], "3460")

    def test_timed_hold_converted_to_true(self):
        hold = faa.parse_hold(HM_TIMED, -1.0)
        self.assertEqual(hold["fix"], "FTZ")
        self.assertAlmostEqual(hold["inbound"], 99.0)
        self.assertEqual(hold["turns"], "right")
        self.assertEqual(hold["leg_minutes"], 1.0)
        self.assertNotIn("leg_nm", hold)
        self.assertEqual(hold["altitude"], 2400)

    def test_distance_hold(self):
        hold = faa.parse_hold(HF_DISTANCE, 0.0)
        self.assertAlmostEqual(hold["inbound"], 78.9)
        self.assertEqual(hold["leg_nm"], 4.0)

    def test_missed_approach_hold_wins_at_a_shared_fix(self):
        holds = faa.build_holds([HF_DISTANCE, HM_DISTANCE], 0.0)
        self.assertEqual(len(holds), 1)
        self.assertEqual(holds[0]["turns"], "left")
        self.assertEqual(holds[0]["leg_nm"], 5.0)

    def test_nasr_physical_ends(self):
        ends = faa.parse_nasr_runway_ends(NASR_26L)
        self.assertAlmostEqual(ends["26L"][0], 38 + 39 / 60 + 42.9923 / 3600, places=6)
        self.assertAlmostEqual(ends["08R"][1], -(90 + 39 / 60 + 35.0903 / 3600), places=6)

    def test_nasr_right_traffic(self):
        # KSUS 08R has a right-hand pattern; 26L's flag is blank, so left-hand.
        self.assertEqual(faa.parse_nasr_right_traffic(NASR_26L), {"08R": True, "26L": False})


class Fixes(unittest.TestCase):
    def test_dme_only_station_uses_its_dme_position(self):
        fixes = faa.index_fixes([DME_ONLY], "KSUS")
        self.assertIn(("D ", "CSX", "K3"), fixes)
        self.assertAlmostEqual(fixes[("D ", "CSX", "K3")][0], 38.7527, places=3)

    def test_missing_fix_fails_instead_of_guessing(self):
        leg = faa.parse_leg(FM_LEG)
        with self.assertRaises(faa.ImportError_):
            faa.resolve(leg, {})

    def test_projection_distances(self):
        # Real CIFP 2611 positions for EAVES and DERDF.
        eaves = faa.project(faa.parse_coord("N38403451W090325075"), ORIGIN)
        self.assertAlmostEqual(math.hypot(*eaves), 5.0, delta=0.1)
        derdf = faa.project(faa.parse_coord("N38322457W090531945"), ORIGIN)
        self.assertAlmostEqual(math.hypot(*derdf), 13.3, delta=0.1)
        north = faa.project((ORIGIN[0] + 1, ORIGIN[1]), ORIGIN)
        self.assertAlmostEqual(north[1], 60.0, places=3)
        self.assertAlmostEqual(north[0], 0.0, places=3)

    def test_bearing(self):
        self.assertAlmostEqual(faa.bearing((0, 0), (0, 1)), 0.0)
        self.assertAlmostEqual(faa.bearing((0, 0), (-1, 0)), 270.0)

    def test_consecutive_duplicates_removed(self):
        self.assertEqual(faa.dedupe_consecutive(["A", "B", "B", "A"]), ["A", "B", "A"])


class Procedures(unittest.TestCase):
    def leg(self, route_type, transition, sequence, fix, path="TF", course=""):
        return {
            "procedure": "TEST1", "route_type": route_type, "transition": transition,
            "sequence": sequence, "fix": fix, "region": "K3", "section": "EA",
            "path": path, "course": course,
        }

    def test_star_joins_common_and_transition_and_keeps_vector_heading(self):
        legs = [
            self.leg("5", "", 10, "AAA"),
            self.leg("6", "RW26B", 10, "BBB"),
            self.leg("6", "RW26B", 20, "BBB", "FM", "3460"),
        ]
        star = faa.build_stars(legs, {"RW26B": "West"}, -1.0)[0]
        self.assertEqual([l["fix"] for l in star["legs"]], ["AAA", "BBB", "BBB"])
        self.assertAlmostEqual(star["final_heading"], 345.0)

    def test_missing_star_transition_fails(self):
        with self.assertRaises(faa.ImportError_):
            faa.build_stars([self.leg("5", "", 10, "AAA")], {"RW26B": "West"}, 0.0)

    def test_sids_clip_at_sector_and_merge_identical_results(self):
        origin = (0.0, 0.0)
        fixes = {
            ("EA", "NEAR", "K3"): (0.0, 0.5),   # 30 NM
            ("EA", "FAR", "K3"): (0.0, 2.0),    # 120 NM
            ("EA", "BEYND", "K3"): (0.0, 3.0),
            ("EA", "OTHER", "K3"): (0.0, 4.0),
        }
        legs = [
            self.leg("2", "ALL", 10, "NEAR"),
            self.leg("2", "ALL", 20, "FAR"),
            self.leg("3", "ONE", 10, "BEYND"),
            self.leg("3", "TWO", 10, "OTHER"),
        ]
        sids = faa.build_sids(legs, fixes, origin, 60)
        self.assertEqual(len(sids), 1)
        self.assertEqual([l["fix"] for l in sids[0]["legs"]], ["NEAR", "FAR"])


MVA_XML = """<m xmlns:a="http://www.aixm.aero/schema/5.1" xmlns:g="http://www.opengis.net/gml/3.2">
<a:Airspace><a:minimumLimit>2500</a:minimumLimit>
<g:exterior><g:LinearRing><g:posList>-90.0 38.0 -89.0 38.0 -89.0 39.0</g:posList></g:LinearRing></g:exterior>
<g:interior><g:LinearRing><g:posList>-89.5 38.2 -89.4 38.2 -89.4 38.3</g:posList></g:LinearRing></g:interior>
</a:Airspace></m>"""


class Mva(unittest.TestCase):
    def test_exterior_holes_and_longitude_first_order(self):
        area = faa.parse_mva(ET.fromstring(MVA_XML))[0]
        self.assertEqual(area["altitude"], 2500)
        self.assertEqual(area["exterior"][0], (38.0, -90.0))
        self.assertEqual(len(area["holes"]), 1)

    def test_empty_chart_is_rejected(self):
        with self.assertRaises(faa.ImportError_):
            faa.parse_mva(ET.fromstring("<m/>"))


if __name__ == "__main__":
    unittest.main()
