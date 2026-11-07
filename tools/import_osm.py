#!/usr/bin/env python3
"""Generate an airport's surface layout from OpenStreetMap. Standard library only.

    python tools/import_osm.py --airport KSUS --cycle 261029
    python tools/import_osm.py --airport KMEM --cycle 261029 --box 0.035

Reads taxiways, runways and aprons from OSM through the Overpass API, and the
runways already generated from FAA data in assets/airports/<airport>.toml, then
writes assets/airports/<airport>_ground.toml. The FAA CIFP in tools/.cache gives
the projection origin, so both files share one coordinate system.

OSM taxiway geometry is used as mapped. Two things are derived, because KSUS has
neither in OSM, and are estimates rather than surveyed positions:

- Hold lines: on every taxiway leaving a runway, at the FAA design-standard
  runway-to-holding-position distance (AC 150/5300-13): HOLD_FT_WIDE for runways
  at least WIDE_RUNWAY_M wide, HOLD_FT_NARROW otherwise.
- Parking: the stands mapped in OSM as parking positions, joined to the
  nearest taxiway; where there are none, each taxilane dead end inside an OSM
  apron area. Mapped stands are marked as gates (at a jet bridge or
  terminal), FBO spots (on an apron named as an FBO, which also gets spots at
  its taxilane dead ends), or ramps (the rest).



The output is derived from OpenStreetMap, (c) OpenStreetMap contributors, and is
licensed under the Open Database License (ODbL).
"""

import argparse
import json
import math
import re
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
#: A mapped stand joins the taxiway graph if its lead-in starts this close to
#: a taxiway, NM. Terminal stands are often drawn short of the taxilane.
LEAD_IN_NM = 0.11
#: A stand this close to a jet bridge, or to a terminal building, is a gate, NM.
BRIDGE_NM = 0.03
TERMINAL_NM = 0.06
#: An apron named like this is a general aviation ramp.
FBO_APRON = re.compile(r"\bFBO\b|general aviation", re.IGNORECASE)
#: A loose taxilane or taxiway end this close to another line, and pointing at
#: it, joins it: OSM sometimes leaves small gaps, NM.
GAP_NM = 0.012
#: An FAA-style taxiway name ("A", "M5"). Mostly outside the aprons, such a
#: taxiway is the movement area; ramp lanes have other names or lie on aprons.
MOVEMENT_NAME = re.compile(r"^[A-Z]{1,2}\d{0,2}$")
#: Stands whose ramp exits are this close share one spot, NM.
SPOT_MERGE_NM = 0.08


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
    """Taxiways, apron rings, runway widths, stands, and what tells gates from
    ramps (FBO aprons, jet bridges, terminals) from an Overpass response."""
    nodes = {e["id"]: (e["lat"], e["lon"]) for e in osm["elements"] if e["type"] == "node"}
    ways = [e for e in osm["elements"] if e["type"] == "way" and "tags" in e]
    xy = lambda n: project(nodes[n], origin)  # noqa: E731

    taxiways, aprons, widths, stands = [], [], {}, []
    context = {"fbo": [], "fbo_names": [], "bridges": [], "terminals": []}
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
            label = tags.get("ref", "") + " " + tags.get("name", "")
            if FBO_APRON.search(label):
                context["fbo"].append(aprons[-1])
                context["fbo_names"].append(fbo_name(label))
        elif kind == "jet_bridge":
            context["bridges"].append([xy(n) for n in way["nodes"]])
        elif kind == "terminal" and way["nodes"][0] == way["nodes"][-1]:
            context["terminals"].append([xy(n) for n in way["nodes"]])
        elif kind == "runway" and "width" in tags:
            widths[tags.get("ref", "")] = float(tags["width"])
        elif kind == "parking_position" and len(way["nodes"]) >= 2:
            stands.append({"name": tags.get("ref", ""), "osm": way["id"], "points": [xy(n) for n in way["nodes"]]})
    if not taxiways:
        raise ImportError_("OSM has no taxiways here")
    return taxiways, aprons, widths, stands, context


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


def fbo_name(label):
    """How pilots name an FBO from its apron's label: "Wilson Air FBO" is "Wilson Air"."""
    words = [w for w in label.split() if w.lower() not in ("fbo", "apron", "ramp", "general", "aviation")]
    return " ".join(words) or "FBO"


def join_gaps(taxiways):
    """Join each loose line end to another line just ahead of it, if within
    GAP_NM: a vertex is added on the other line and the end extended to it."""
    cells = {}
    for t in taxiways:
        for p in t["points"]:
            cells.setdefault(grid_cell(p), []).append(p)

    def loose(p):
        # Only its own vertex here: nothing else meets this end.
        cx, cy = grid_cell(p)
        near = (q for dx in (-1, 0, 1) for dy in (-1, 0, 1) for q in cells.get((cx + dx, cy + dy), ()))
        return sum(dist(p, q) < MERGE_NM for q in near) == 1

    joined = 0
    for t in taxiways:
        for end in (0, -1):
            pts = t["points"]
            p, before = pts[end], pts[1 if end == 0 else -2]
            if not loose(p):
                continue
            ahead = sub(p, before)
            best = None
            for other in taxiways:
                if other is t:
                    continue
                opts = other["points"]
                for k in range(len(opts) - 1):
                    a, b = opts[k], opts[k + 1]
                    ab = sub(b, a)
                    length2 = dot(ab, ab)
                    s = 0.0 if length2 == 0 else max(0.0, min(1.0, dot(sub(p, a), ab) / length2))
                    q = lerp(a, b, s)
                    d = dist(p, q)
                    if d >= GAP_NM or (best and d >= best[0]):
                        continue
                    # Only straight on: a gap, not a line that passes alongside.
                    to_q = sub(q, p)
                    if d >= MERGE_NM and dot(to_q, ahead) < 0.7 * d * math.hypot(*ahead):
                        continue
                    best = (d, other, k, q)
            if best is None:
                continue
            _, other, k, q = best
            opts = other["points"]
            snapped = next((v for v in (opts[k], opts[k + 1]) if dist(v, q) < MERGE_NM), None)
            if snapped is None:
                opts.insert(k + 1, q)
                snapped = q
            if dist(p, snapped) >= MERGE_NM:
                if end == 0:
                    pts.insert(0, snapped)
                else:
                    pts.append(snapped)
            else:
                pts[end] = snapped
            cells.setdefault(grid_cell(snapped), []).append(snapped)
            joined += 1
    return joined


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


def distance_to_segment(p, a, b):
    ab = sub(b, a)
    length2 = dot(ab, ab)
    s = 0.0 if length2 == 0 else max(0.0, min(1.0, dot(sub(p, a), ab) / length2))
    return dist(p, lerp(a, b, s))


def near_ring(p, ring, nm):
    return point_in_polygon(p, ring) or any(distance_to_segment(p, a, b) < nm for a, b in zip(ring, ring[1:]))


def stand_use(points, context):
    """What a mapped stand is for: "ga" on an FBO apron, "gate" at a jet
    bridge or terminal, otherwise "ramp" (cargo or remote)."""
    if any(point_in_polygon(p, ring) for ring in context["fbo"] for p in points):
        return "ga"
    if any(dist(p, q) < BRIDGE_NM for bridge in context["bridges"] for q in bridge for p in points):
        return "gate"
    if any(near_ring(p, ring, TERMINAL_NM) for ring in context["terminals"] for p in points):
        return "gate"
    return "ramp"


def nearest_on_taxiways(p, taxiways):
    """The closest point on any taxiway segment to p: (distance, taxiway, segment index, point)."""
    best = None
    for t in taxiways:
        pts = t["points"]
        for k in range(len(pts) - 1):
            a, b = pts[k], pts[k + 1]
            ab = sub(b, a)
            length2 = dot(ab, ab)
            s = 0.0 if length2 == 0 else max(0.0, min(1.0, dot(sub(p, a), ab) / length2))
            q = lerp(a, b, s)
            d = dist(p, q)
            if best is None or d < best[0]:
                best = (d, t, k, q)
    return best


def mapped_parking(stands, taxiways, runways, context=None):
    """Stands mapped in OSM as parking positions: each lead-in line joins the
    nearest taxiway or taxilane, and the spot is at its other end, facing in.
    Unnumbered stands are named P1, P2, and so on. With `context`, each is
    marked with what it's for."""
    spots, lead_ins, used = [], [], set()
    for stand in stands:
        pts = stand["points"]
        first, last = nearest_on_taxiways(pts[0], taxiways), nearest_on_taxiways(pts[-1], taxiways)
        if last[0] < first[0]:
            pts, first = pts[::-1], last
        d, taxiway, k, joint = first
        if d > LEAD_IN_NM:
            continue
        stop = pts[-1]
        if any(r.within(r.locate(stop)[0]) and abs(r.locate(stop)[1]) < r.hold_nm * 1.5 for r in runways):
            continue

        # A vertex where the lead-in leaves the taxiway, so the two connect.
        tpts = taxiway["points"]
        if dist(joint, tpts[k]) >= MERGE_NM and dist(joint, tpts[k + 1]) >= MERGE_NM:
            tpts.insert(k + 1, joint)
        line = [joint] + [p for p in pts if dist(p, joint) >= MERGE_NM]
        if len(line) < 2:
            continue
        before = line[-2]
        heading = math.degrees(math.atan2(stop[0] - before[0], stop[1] - before[1])) % 360
        lead_ins.append({"name": "", "osm": stand["osm"], "points": line})
        spot = {"at": stop, "heading": heading, "ref": stand["name"]}
        if context is not None:
            spot["use"] = stand_use(pts, context)
        spots.append(spot)
    spots.sort(key=lambda s: (-s["at"][1], s["at"][0]))
    unnamed = 0
    for spot in spots:
        name = spot.pop("ref")
        if not name or name in used:
            unnamed += 1
            name = f"P{unnamed}"
        used.add(name)
        spot["name"] = name
    return spots, lead_ins


def grid_cell(p):
    return (math.floor(p[0] / MERGE_NM), math.floor(p[1] / MERGE_NM))


def main_network(taxiways):
    """Grid cells of the largest connected piece of the taxiway network. Lines
    meet where their vertices lie within MERGE_NM of each other."""
    parent = {}

    def find(k):
        while parent.setdefault(k, k) != k:
            parent[k] = parent[parent[k]]
            k = parent[k]
        return k

    def union(a, b):
        parent[find(a)] = find(b)

    by_cell = {}
    for t in taxiways:
        pts = t["points"]
        for p in pts:
            by_cell.setdefault(grid_cell(p), []).append(p)
        for a, b in zip(pts, pts[1:]):
            union(grid_cell(a), grid_cell(b))
    for (cx, cy), pts in by_cell.items():
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                other = by_cell.get((cx + dx, cy + dy))
                if other and any(dist(p, q) < MERGE_NM for p in pts for q in other):
                    union((cx, cy), (cx + dx, cy + dy))
    sizes = {}
    for k, pts in by_cell.items():
        sizes[find(k)] = sizes.get(find(k), 0) + len(pts)
    biggest = max(sizes, key=sizes.get)
    return {k for k in by_cell if find(k) == biggest}


class PointIndex:
    """Merges points within MERGE_NM into numbered nodes, as the game does."""

    def __init__(self):
        self.points = []
        self.cells = {}

    def id(self, p):
        cx, cy = grid_cell(p)
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for i in self.cells.get((cx + dx, cy + dy), ()):
                    if dist(self.points[i], p) < MERGE_NM:
                        return i
        self.points.append(p)
        self.cells.setdefault((cx, cy), []).append(len(self.points) - 1)
        return len(self.points) - 1


def is_movement(taxiway, aprons):
    """An FAA-named taxiway mostly off the aprons: Ground's, not the ramp's."""
    if not MOVEMENT_NAME.match(taxiway["name"]):
        return False
    inside = sum(any(point_in_polygon(p, ring) for ring in aprons) for p in taxiway["points"])
    return inside * 2 < len(taxiway["points"])


def find_spots(taxiways, parking, aprons, context):
    """Ramp spots: where the way out from cargo and FBO stands first reaches
    the movement area. Nearby exits share one spot. Cargo spots are numbered
    from the north-west; an FBO's spot is named after it. Each such stand
    gets the name of its spot."""
    index = PointIndex()
    edges = {}
    movement_nodes = set()
    for t in taxiways:
        ids = [index.id(p) for p in t["points"]]
        moving = is_movement(t, aprons)
        for a, b in zip(ids, ids[1:]):
            if moving:
                movement_nodes.update((a, b))
            else:
                d = dist(index.points[a], index.points[b])
                edges.setdefault(a, []).append((b, d))
                edges.setdefault(b, []).append((a, d))

    def exit_of(start):
        # Nearest movement-area node along ramp lines only.
        best, frontier, done = {start: 0.0}, [(0.0, start)], set()
        while frontier:
            frontier.sort(reverse=True)
            d, n = frontier.pop()
            if n in done:
                continue
            done.add(n)
            if n in movement_nodes:
                return n
            for m, w in edges.get(n, ()):
                if d + w < best.get(m, math.inf):
                    best[m] = d + w
                    frontier.append((d + w, m))
        return None

    groups = {}
    for spot in parking:
        use = spot.get("use")
        if use not in ("ramp", "ga"):
            continue
        out = exit_of(index.id(spot["at"]))
        if out is None:
            continue
        fbo = ""
        if use == "ga":
            fbo = next((name for name, ring in zip(context["fbo_names"], context["fbo"])
                        if point_in_polygon(spot["at"], ring)), "FBO")
        groups.setdefault(fbo, {}).setdefault(out, []).append(spot)

    spots = []
    for fbo, exits in groups.items():
        if fbo:
            # An FBO has one spot: its busiest way out.
            busiest = max(exits, key=lambda n: len(exits[n]))
            stands = [stand for group in exits.values() for stand in group]
            spots.append({"name": fbo, "at": index.points[busiest], "stands": stands})
            continue
        clusters = []
        for out in sorted(exits, key=lambda n: (-len(exits[n]), n)):
            here = index.points[out]
            home = next((c for c in clusters if dist(index.points[c[0]], here) < SPOT_MERGE_NM), None)
            if home is None:
                home = [out, []]
                clusters.append(home)
            home[1].extend(exits[out])
        for out, stands in clusters:
            spots.append({"name": "", "at": index.points[out], "stands": stands})
    numbered = sorted((s for s in spots if not s["name"]), key=lambda s: (-s["at"][1], s["at"][0]))
    for n, s in enumerate(numbered, 1):
        s["name"] = str(n)
    for s in spots:
        for stand in s.pop("stands"):
            stand["spot"] = s["name"]
    mark_ramp_lanes(taxiways, parking, index, edges, movement_nodes)
    return sorted(spots, key=lambda s: (not s["name"].isdigit(), int(s["name"]) if s["name"].isdigit() else 0, s["name"]))


def mark_ramp_lanes(taxiways, parking, index, edges, movement_nodes):
    """Flag the lanes of ramps Ground hands off at (`ramp`): every
    non-movement line reached from a stand with a spot without crossing the
    movement area. The ramp's own business, so the map doesn't label them."""
    reached = set()
    frontier = [index.id(s["at"]) for s in parking if s.get("spot")]
    while frontier:
        n = frontier.pop()
        if n in reached:
            continue
        reached.add(n)
        if n in movement_nodes:
            continue
        frontier.extend(m for m, _ in edges.get(n, ()))
    for t in taxiways:
        ids = [index.id(p) for p in t["points"]]
        # Connectors from the movement area onto the ramp count too.
        on_ramp = [i in reached and i not in movement_nodes for i in ids]
        if any(on_ramp) and all(i in reached or i in movement_nodes for i in ids):
            t["ramp"] = True


def build(osm, airport, origin):
    taxiways, aprons, widths, stands, context = osm_layout(osm, origin)
    runways = physical_runways(airport, widths)
    snap_to_runways(taxiways, runways)
    join_gaps(taxiways)
    holds = insert_holds(taxiways, runways)
    # FBO ramps rarely have stands mapped: park general aviation at their
    # taxilane dead ends.
    fbo = find_parking(taxiways, context["fbo"], runways) if context["fbo"] else []
    # Stands mapped in OSM where there are any; otherwise taxilane dead ends.
    parking, lead_ins = mapped_parking(stands, taxiways, runways, context)
    taxiways += lead_ins
    if parking:
        for n, spot in enumerate(fbo, 1):
            spot.update(name=f"G{n}", use="ga")
        parking += fbo
    else:
        parking = find_parking(taxiways, aprons, runways)
    # A spot on a stray piece of taxilane couldn't be taxied to.
    network = main_network(taxiways)
    parking = [s for s in parking if grid_cell(s["at"]) in network]

    if not parking:
        raise ImportError_("no mapped stands or taxilane dead ends inside an apron to park at")
    spots = find_spots(taxiways, parking, aprons, context)
    end_of = {r.name: r.name.split("/")[0] for r in runways}
    return {
        "taxiways": taxiways,
        "holds": [{"at": h, "runway": end_of[name]} for h, name in holds],
        "parking": parking,
        "spots": spots,
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
        if t.get("ramp"):
            out.append("ramp = true")
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
        if s.get("use"):
            out.append(f'use = "{s["use"]}"')
        if s.get("spot"):
            out.append(f'spot = "{s["spot"]}"')
    for s in data.get("spots", []):
        out += ["", "[[ground.spots]]", f'name = "{s["name"]}"', f"at = {pt(s['at'])}"]

    return "\n".join(out) + "\n"


def fetch_osm(airport_code, origin, box_deg=BOX_DEG):
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / "osm" / f"{airport_code.lower()}_raw.json"
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        lat, lon = origin
        box = f"{lat - box_deg},{lon - box_deg * 1.3},{lat + box_deg},{lon + box_deg * 1.3}"
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
    parser.add_argument("--box", type=float, default=BOX_DEG,
                        help=f"half-size of the download box, degrees of latitude (default {BOX_DEG})")
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
    osm = fetch_osm(args.airport, origin, args.box)
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
