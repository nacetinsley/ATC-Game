#!/usr/bin/env python3
"""Generate an airport's surface layout from OpenStreetMap. Standard library only.

    python tools/import_osm.py --airport KSUS --cycle 261029

Reads taxiways, runways and aprons from OSM through the Overpass API, and the
runways already generated from FAA data in assets/airports/<airport>.toml, then
writes assets/airports/<airport>_ground.toml. The FAA CIFP in tools/.cache gives
the projection origin, so both files share one coordinate system.

OSM taxiway geometry is used as mapped. Two things are derived, because KSUS has
neither in OSM, and are estimates rather than surveyed positions:

- Hold lines: on every taxiway leaving a runway, at the FAA design-standard
  runway-to-holding-position distance (AC 150/5300-13): HOLD_FT_WIDE for runways
  at least WIDE_RUNWAY_M wide, HOLD_FT_NARROW otherwise.
- Parking: at each taxilane dead end inside an OSM apron area.

The output is derived from OpenStreetMap, (c) OpenStreetMap contributors, and is
licensed under the Open Database License (ODbL).
"""

import argparse
import json
import math
import sys
import tomllib
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from import_faa import CACHE, CIFP_MEMBER, FT_PER_NM, ROOT, ImportError_, parse_airport, project  # noqa: E402

OVERPASS = "https://overpass-api.de/api/interpreter"
#: Overpass refuses browser-like agents.
OSM_USER_AGENT = "atcgame-osm-import/0.1"
#: Half-size of the download box around the field, degrees.
BOX_DEG = 0.03
WIDE_RUNWAY_M = 45.0
HOLD_FT_WIDE = 250.0
HOLD_FT_NARROW = 200.0
#: A taxiway vertex this close to a runway centerline is on it, NM.
SNAP_NM = 60.0 / FT_PER_NM
#: Must match MERGE_NM in src/sim/ground_map.rs.
MERGE_NM = 0.002
#: Must stay under MAX_HOLD_DISTANCE_NM in src/sim/ground_map.rs.
MAX_ZONE_PATH_NM = 0.18
SKIP_TAGS = ("abandoned", "disused", "construction")


# --- Geometry ---------------------------------------------------------------


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1]


def lerp(a, b, t):
    return (a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)


def dist(a, b):
    return math.hypot(a[0] - b[0], a[1] - b[1])


def point_in_polygon(p, ring):
    inside = False
    j = len(ring) - 1
    for i in range(len(ring)):
        (xa, ya), (xb, yb) = ring[i], ring[j]
        if (ya > p[1]) != (yb > p[1]) and p[0] < (xb - xa) * (p[1] - ya) / (yb - ya) + xa:
            inside = not inside
        j = i
    return inside


class Centerline:
    """One physical runway: pavement end to pavement end, with a hold distance."""

    def __init__(self, name, start, end, hold_nm):
        self.name = name
        self.start = start
        self.end = end
        self.length = dist(start, end)
        self.dir = ((end[0] - start[0]) / self.length, (end[1] - start[1]) / self.length)
        self.hold_nm = hold_nm

    def locate(self, p):
        """(distance along from start, signed distance off the centerline)."""
        rel = sub(p, self.start)
        return dot(rel, self.dir), rel[0] * self.dir[1] - rel[1] * self.dir[0]

    def within(self, along):
        return -MERGE_NM <= along <= self.length + MERGE_NM

    def snap(self, p):
        along, _ = self.locate(p)
        along = min(max(along, 0.0), self.length)
        return (self.start[0] + self.dir[0] * along, self.start[1] + self.dir[1] * along)


# --- Input ------------------------------------------------------------------


def physical_runways(airport, osm_widths):
    """Pair runway ends from the generated airport file into centerlines."""
    ends = {r["id"]: r for r in airport["runways"]}
    seen, lines = set(), []
    for rid, r in ends.items():
        if rid in seen:
            continue
        number, side = int(rid[:2]), rid[2:]
        opposite = f"{(number + 18 - 1) % 36 + 1:02d}{ {'L': 'R', 'R': 'L'}.get(side, side) }"
        if opposite not in ends:
            raise ImportError_(f"runway {rid} has no opposite end {opposite}")
        seen.update({rid, opposite})
        o = ends[opposite]
        start = (r.get("start_x", r["x"]), r.get("start_y", r["y"]))
        end = (o.get("start_x", o["x"]), o.get("start_y", o["y"]))
        name = f"{rid}/{opposite}"
        width = osm_widths.get(name) or osm_widths.get(f"{opposite}/{rid}")
        if width is None:
            raise ImportError_(f"OSM has no width for runway {name}")
        hold_ft = HOLD_FT_WIDE if width >= WIDE_RUNWAY_M else HOLD_FT_NARROW
        lines.append(Centerline(name, start, end, hold_ft / FT_PER_NM))
    return lines


def osm_layout(osm, origin):
    """Taxiways, apron rings and runway widths from an Overpass response."""
    nodes = {e["id"]: (e["lat"], e["lon"]) for e in osm["elements"] if e["type"] == "node"}
    ways = [e for e in osm["elements"] if e["type"] == "way" and "tags" in e]
    xy = lambda n: project(nodes[n], origin)  # noqa: E731

    taxiways, aprons, widths = [], [], {}
    for way in ways:
        tags = way["tags"]
        kind = tags.get("aeroway")
        if any(t in tags for t in SKIP_TAGS):
            continue
        if kind in ("taxiway", "taxilane"):
            name = tags.get("ref", "") if kind == "taxiway" else ""
            taxiways.append({"name": name, "osm": way["id"], "points": [xy(n) for n in way["nodes"]]})
        elif kind == "apron" and way["nodes"][0] == way["nodes"][-1]:
            aprons.append([xy(n) for n in way["nodes"]])
        elif kind == "runway" and "width" in tags:
            widths[tags.get("ref", "")] = float(tags["width"])
    if not taxiways:
        raise ImportError_("OSM has no taxiways here")
    return taxiways, aprons, widths


# --- Building ---------------------------------------------------------------


class Graph:
    """Taxiway vertices merged into nodes, so holds can be found by walking."""

    def __init__(self):
        self.points = []
        self.edges = {}

    def node(self, p):
        for i, q in enumerate(self.points):
            if dist(p, q) < MERGE_NM:
                return i
        self.points.append(p)
        self.edges[len(self.points) - 1] = set()
        return len(self.points) - 1

    def link(self, a, b):
        if a != b:
            self.edges[a].add(b)
            self.edges[b].add(a)


def snap_to_runways(taxiways, runways):
    """Put vertices near a centerline on it, and add a vertex where a taxiway crosses one."""
    for taxiway in taxiways:
        points = []
        for a, b in zip(taxiway["points"], taxiway["points"][1:] + [None]):
            points.append(a)
            if b is None:
                continue
            for r in runways:
                (aa, ao), (ba, bo) = r.locate(a), r.locate(b)
                if (ao > SNAP_NM and bo < -SNAP_NM) or (ao < -SNAP_NM and bo > SNAP_NM):
                    t = ao / (ao - bo)
                    if r.within(aa + (ba - aa) * t):
                        points.append(r.snap(lerp(a, b, t)))
        snapped = []
        for p in points:
            for r in runways:
                along, off = r.locate(p)
                if abs(off) <= SNAP_NM and r.within(along):
                    p = r.snap(p)
                    break
            snapped.append(p)
        taxiway["points"] = snapped


def insert_holds(taxiways, runways):
    """Walk out from each runway along the taxiways and add a hold at its distance.

    Returns [(point, runway name)]. Each taxiway gets new vertices at the holds.
    """
    graph = Graph()
    for t in taxiways:
        ids = [graph.node(p) for p in t["points"]]
        for a, b in zip(ids, ids[1:]):
            graph.link(a, b)

    def on_runway(i):
        for r in runways:
            along, off = r.locate(graph.points[i])
            if abs(off) < MERGE_NM and r.within(along):
                return r
        return None

    holds = []
    for r in runways:
        starts = [i for i in graph.edges if on_runway(i) is r]
        visited = set(starts)
        stack = [(i, 0.0) for i in starts]
        while stack:
            i, walked = stack.pop()
            for j in graph.edges[i]:
                if j in visited or on_runway(j) is not None:
                    continue
                pi, pj = graph.points[i], graph.points[j]
                oi, oj = abs(r.locate(pi)[1]), abs(r.locate(pj)[1])
                if oj >= r.hold_nm > oi:
                    t = (r.hold_nm - oi) / (oj - oi)
                    hold = lerp(pi, pj, t)
                    if dist(hold, pj) < MERGE_NM:
                        hold = pj
                    elif dist(hold, pi) < MERGE_NM:
                        hold = pi
                    holds.append((hold, r.name))
                    continue
                if oj >= r.hold_nm:
                    continue
                step = walked + dist(pi, pj)
                if step > MAX_ZONE_PATH_NM:
                    raise ImportError_(
                        f"taxiway at ({pj[0]:.3f}, {pj[1]:.3f}) stays within the hold distance of "
                        f"runway {r.name} for over {MAX_ZONE_PATH_NM * FT_PER_NM:.0f} ft"
                    )
                visited.add(j)
                stack.append((j, step))

    # Give each hold a vertex on its taxiway, or move it onto a vertex already there.
    for index, (hold, name) in enumerate(holds):
        for t in taxiways:
            pts = t["points"]
            near = [p for p in pts if dist(hold, p) < MERGE_NM]
            if near:
                holds[index] = (near[0], name)
                break
            for k in range(len(pts) - 1):
                a, b = pts[k], pts[k + 1]
                ab = sub(b, a)
                length2 = dot(ab, ab)
                if length2 == 0:
                    continue
                u = dot(sub(hold, a), ab) / length2
                if 0 < u < 1 and dist(lerp(a, b, u), hold) < MERGE_NM:
                    pts.insert(k + 1, hold)
                    break
            else:
                continue
            break
    unique = []
    for hold, name in holds:
        if not any(dist(hold, h) < MERGE_NM and n == name for h, n in unique):
            unique.append((hold, name))
    return unique


def find_parking(taxiways, aprons, runways):
    """Taxilane dead ends inside an apron become parking spots, facing along the lane."""
    degree, last_segment = {}, {}
    for t in taxiways:
        pts = t["points"]
        for k, p in enumerate(pts):
            key = (round(p[0] / MERGE_NM), round(p[1] / MERGE_NM))
            degree[key] = degree.get(key, 0) + (1 if k in (0, len(pts) - 1) else 2)
            if k == 0:
                last_segment.setdefault(key, (pts[1], p))
            elif k == len(pts) - 1:
                last_segment.setdefault(key, (pts[-2], p))
    spots = []
    for key, d in sorted(degree.items()):
        if d != 1:
            continue
        before, p = last_segment[key]
        if not any(point_in_polygon(p, ring) for ring in aprons):
            continue
        if any(abs(r.locate(p)[1]) < r.hold_nm * 1.5 for r in runways):
            continue
        heading = math.degrees(math.atan2(p[0] - before[0], p[1] - before[1])) % 360
        spots.append({"at": p, "heading": heading})
    spots.sort(key=lambda s: (-s["at"][1], s["at"][0]))
    for n, spot in enumerate(spots, 1):
        spot["name"] = f"P{n}"
    return spots


def build(osm, airport, origin):
    taxiways, aprons, widths = osm_layout(osm, origin)
    runways = physical_runways(airport, widths)
    snap_to_runways(taxiways, runways)
    holds = insert_holds(taxiways, runways)
    parking = find_parking(taxiways, aprons, runways)
    if not parking:
        raise ImportError_("no taxilane dead ends inside an apron to park at")
    end_of = {r.name: r.name.split("/")[0] for r in runways}
    return {
        "taxiways": taxiways,
        "holds": [{"at": h, "runway": end_of[name]} for h, name in holds],
        "parking": parking,
    }


# --- Output -----------------------------------------------------------------


def fmt(v):
    return f"{v:.5f}".rstrip("0").rstrip(".") if "." in f"{v:.5f}" else f"{v}"


def pt(p):
    return f"[{fmt(p[0])}, {fmt(p[1])}]"


def render(data, airport_code, timestamp):
    out = [
        f"# GENERATED by tools/import_osm.py from OpenStreetMap ({timestamp}). Do not edit by hand.",
        "# (c) OpenStreetMap contributors. This file is licensed under the Open Database",
        "# License (ODbL) 1.0: https://opendatacommons.org/licenses/odbl/",
        "# Hold lines and parking are derived estimates, not surveyed: see the importer.",
        "# NOT FOR NAVIGATION.",
        "",
        "[ground]",
    ]
    for t in data["taxiways"]:
        out += ["", "[[ground.taxiways]]"]
        if t["name"]:
            out.append(f'name = "{t["name"]}"')
        out.append(f"# OSM way {t['osm']}")
        out.append("points = [" + ", ".join(pt(p) for p in t["points"]) + "]")
    for h in data["holds"]:
        out += ["", "[[ground.holds]]", f"at = {pt(h['at'])}", f'runway = "{h["runway"]}"']
    for s in data["parking"]:
        out += [
            "",
            "[[ground.parking]]",
            f'name = "{s["name"]}"',
            f"at = {pt(s['at'])}",
            f"heading = {s['heading']:.1f}",
        ]
    return "\n".join(out) + "\n"


def fetch_osm(airport_code, origin):
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / "osm" / f"{airport_code.lower()}_raw.json"
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        lat, lon = origin
        box = f"{lat - BOX_DEG},{lon - BOX_DEG * 1.3},{lat + BOX_DEG},{lon + BOX_DEG * 1.3}"
        query = f'[out:json][timeout:60];(way["aeroway"]({box}););out body;>;out skel qt;'
        print(f"downloading OSM aeroways around {airport_code}")
        body = urllib.parse.urlencode({"data": query}).encode()
        request = urllib.request.Request(OVERPASS, data=body, headers={"User-Agent": OSM_USER_AGENT})
        with urllib.request.urlopen(request, timeout=120) as response:
            path.write_bytes(response.read())
    return json.loads(path.read_text(encoding="utf-8"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--airport", required=True, help="ICAO code, e.g. KSUS")
    parser.add_argument("--cycle", required=True, help="CIFP cycle used for the airport file")
    args = parser.parse_args()

    cifp = CACHE / f"CIFP_{args.cycle}.zip"
    if not cifp.exists():
        raise ImportError_(f"{cifp} missing: run tools/import_faa.py first")
    lines = zipfile.ZipFile(cifp).read(CIFP_MEMBER).decode("latin-1").splitlines()
    record = next(
        l for l in lines
        if l.startswith("S") and l[6:10] == args.airport and l[4:6] == "P " and l[12] == "A"
    )
    origin = parse_airport(record)["pos"]
    airport = tomllib.loads(
        (ROOT / "assets" / "airports" / f"{args.airport.lower()}.toml").read_text(encoding="utf-8")
    )
    osm = fetch_osm(args.airport, origin)
    data = build(osm, airport, origin)
    timestamp = osm.get("osm3s", {}).get("timestamp_osm_base", "unknown date")
    output = ROOT / "assets" / "airports" / f"{args.airport.lower()}_ground.toml"
    output.write_text(render(data, args.airport, timestamp), encoding="utf-8", newline="\n")
    print(
        f"wrote {output}: {len(data['taxiways'])} taxiways, {len(data['holds'])} hold lines, "
        f"{len(data['parking'])} parking spots"
    )


if __name__ == "__main__":
    try:
        main()
    except ImportError_ as e:
        sys.exit(f"import failed: {e}")
