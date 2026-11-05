#!/usr/bin/env python3
"""Generate a game airport file from FAA data. Standard library only.

    python tools/import_faa.py --cycle 261029 --nasr 2026-10-29 --airport KSUS --tracon T75

Reads CIFP (procedures, runways, ILS), NASR (physical runway ends) and MVA charts,
then merges the hand-set values in tools/<airport>_overrides.toml. Downloads go to
tools/.cache, which git ignores. Any fix that can't be resolved stops the import:
it must never guess at a position.
"""

import argparse
import math
import re
import sys
import tomllib
import urllib.request
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "tools" / ".cache"
USER_AGENT = "Mozilla/5.0 (compatible; atcgame-import)"
FT_PER_NM = 6076.12
CIFP_MEMBER = "FAACIFP18"

STAR_COMMON = "5"
STAR_RUNWAY_TRANSITION = "6"
SID_COMMON = "2"
SID_TRANSITION = "3"


class ImportError_(Exception):
    """The source data can't be turned into a valid airport."""


# --- Decoding -------------------------------------------------------------


def parse_coord(text):
    """ARINC 424 'N38394201W090380869' to (lat, lon) in degrees."""
    match = re.fullmatch(r"([NS])(\d{2})(\d{2})(\d{4})([EW])(\d{3})(\d{2})(\d{4})", text)
    if not match:
        raise ImportError_(f"bad coordinate {text!r}")
    ns, lat_d, lat_m, lat_s, ew, lon_d, lon_m, lon_s = match.groups()
    lat = int(lat_d) + int(lat_m) / 60 + int(lat_s) / 360000
    lon = int(lon_d) + int(lon_m) / 60 + int(lon_s) / 360000
    return (-lat if ns == "S" else lat, -lon if ew == "W" else lon)


def parse_variation(text):
    """ARINC 'W0010' to degrees, east positive."""
    value = int(text[1:]) / 10
    return -value if text[0] == "W" else value


def parse_dms(text):
    """NASR '38-39-42.9923N' to signed degrees."""
    match = re.fullmatch(r"(\d+)-(\d+)-([\d.]+)([NSEW])", text)
    if not match:
        raise ImportError_(f"bad NASR coordinate {text!r}")
    d, m, s, hemi = match.groups()
    value = int(d) + int(m) / 60 + float(s) / 3600
    return -value if hemi in "SW" else value


def parse_airport(line):
    return {
        "pos": parse_coord(line[32:51]),
        "variation": parse_variation(line[51:56]),
        "elevation": float(line[56:61]),
    }


def parse_runway(line):
    return {
        "id": line[15:18].strip(),
        "length_ft": int(line[22:27]),
        "threshold": parse_coord(line[32:51]),
        "displaced_ft": int(line[71:75]),
    }


def parse_ils(line):
    return {
        "runway": line[29:32].strip(),
        "glideslope_deg": int(line[87:90]) / 100,
        "tch": int(line[95:97]),
    }


def parse_leg(line):
    return {
        "procedure": line[13:19].strip(),
        "route_type": line[19],
        "transition": line[20:25].strip(),
        "sequence": int(line[26:29]),
        "fix": line[29:34].strip(),
        "region": line[34:36],
        "section": line[36:38],
        "path": line[47:49],
        "course": line[70:74].strip(),
    }


def parse_nasr_runway_ends(line):
    """Physical ends of one NASR RWY record, as {runway id: (lat, lon)}."""
    base, _, reciprocal = line[16:23].strip().partition("/")
    coord = re.compile(r"(\d\d-\d\d-\d\d\.\d{4}[NS])\s*\S*?(\d{3}-\d\d-\d\d\.\d{4}[EW])")
    split = re.search(re.escape(reciprocal) + r"\d{3}", line[60:])
    if not base or not reciprocal or not split:
        raise ImportError_(f"can't read NASR runway {line[16:30]!r}")
    middle = 60 + split.start()
    ends = {}
    for runway, part in ((base, line[60:middle]), (reciprocal, line[middle:])):
        match = coord.search(part)
        if not match:
            raise ImportError_(f"no position for NASR runway end {runway}")
        ends[runway] = (parse_dms(match.group(1)), parse_dms(match.group(2)))
    return ends


# --- Geometry -------------------------------------------------------------


def project(pos, origin):
    """Lat/long to NM from the origin: x east, y north."""
    x = (pos[1] - origin[1]) * 60 * math.cos(math.radians(origin[0]))
    y = (pos[0] - origin[0]) * 60
    return (x, y)


def bearing(a, b):
    return math.degrees(math.atan2(b[0] - a[0], b[1] - a[1])) % 360


def in_circle(point, radius):
    return math.hypot(*point) <= radius


def dedupe_consecutive(items):
    out = []
    for item in items:
        if not out or out[-1] != item:
            out.append(item)
    return out


# --- Fix lookup -----------------------------------------------------------


def index_fixes(lines, airport):
    """All positions a procedure could reference, keyed by (section, ident, region)."""
    fixes = {}
    for line in lines:
        if not line.startswith("S"):
            continue
        section = line[4:6]
        if section == "EA":
            fixes[("EA", line[13:18].strip(), line[19:21])] = parse_coord(line[32:51])
        elif section == "P " and line[12] == "C" and line[6:10] == airport:
            fixes[("PC", line[13:18].strip(), line[19:21])] = parse_coord(line[32:51])
        elif section == "D ":
            # DME-only stations have no VOR position: theirs is in the DME columns.
            vor, dme = line[32:51].strip(), line[55:74].strip()
            if vor or dme:
                fixes[("D ", line[13:17].strip(), line[19:21])] = parse_coord(vor or dme)
    return fixes


def resolve(leg, fixes):
    """Position of a procedure leg's fix. Raises rather than guesses."""
    key = (leg["section"], leg["fix"], leg["region"])
    if key not in fixes:
        raise ImportError_(f"unresolved fix {key} on {leg['procedure']}")
    return fixes[key]


def legs_for(records, kind):
    # Column 39 above 1 marks a continuation record, which isn't a leg of its own.
    legs = [parse_leg(r) for r in records if r[12] == kind and r[38] in "01"]
    return sorted(legs, key=lambda l: (l["procedure"], l["route_type"], l["transition"], l["sequence"]))


def build_stars(legs, config_by_transition, variation):
    """One route per STAR and runway direction: common part plus runway transition."""
    stars = []
    for name in dict.fromkeys(l["procedure"] for l in legs):
        common = [l for l in legs if l["procedure"] == name and l["route_type"] == STAR_COMMON]
        for transition, config in config_by_transition.items():
            part = [
                l
                for l in legs
                if l["procedure"] == name
                and l["route_type"] == STAR_RUNWAY_TRANSITION
                and l["transition"] == transition
            ]
            if not common or not part:
                raise ImportError_(f"{name}: missing common route or {transition} transition")
            route = common + part
            final = [l for l in route if l["path"] == "FM"]
            heading = None
            if final and final[-1]["course"]:
                heading = (int(final[-1]["course"]) / 10 + variation) % 360
            stars.append({"name": name, "config": config, "legs": route, "final_heading": heading})
    return stars


def clip_to_sector(legs, fixes, origin, radius):
    """Stop at the first fix outside the sector, keeping it so the aircraft flies out."""
    kept = []
    for leg in legs:
        kept.append(leg)
        if not in_circle(project(resolve(leg, fixes), origin), radius):
            break
    return kept


def build_sids(legs, fixes, origin, radius):
    """Every SID transition, clipped at the sector and with duplicates removed."""
    sids, seen = [], set()
    for name in dict.fromkeys(l["procedure"] for l in legs):
        common = [l for l in legs if l["procedure"] == name and l["route_type"] == SID_COMMON]
        transitions = dict.fromkeys(
            l["transition"]
            for l in legs
            if l["procedure"] == name and l["route_type"] == SID_TRANSITION
        )
        for transition in transitions or [""]:
            part = [
                l
                for l in legs
                if l["procedure"] == name
                and l["route_type"] == SID_TRANSITION
                and l["transition"] == transition
            ]
            route = clip_to_sector(common + part, fixes, origin, radius)
            names = tuple(dedupe_consecutive(l["fix"] for l in route))
            if names not in seen:
                seen.add(names)
                sids.append({"name": name, "legs": route})
    return sids


# --- MVA ------------------------------------------------------------------


def local_name(element):
    return element.tag.rsplit("}", 1)[-1]


def parse_mva(xml_root):
    """MVA sectors as {altitude, exterior, holes}, with rings as (lat, lon) lists."""
    areas = []
    for airspace in xml_root.iter():
        if local_name(airspace) != "Airspace":
            continue
        altitude, exterior, holes = None, None, []
        for element in airspace.iter():
            name = local_name(element)
            if name == "minimumLimit":
                altitude = float(element.text)
            elif name in ("exterior", "interior"):
                ring = parse_pos_list(element)
                if name == "exterior":
                    exterior = ring
                else:
                    holes.append(ring)
        if altitude is None or exterior is None:
            raise ImportError_("MVA sector missing altitude or boundary")
        areas.append({"altitude": altitude, "exterior": exterior, "holes": holes})
    if not areas:
        raise ImportError_("no MVA sectors found")
    return areas


def parse_pos_list(ring_element):
    for element in ring_element.iter():
        if local_name(element) == "posList":
            values = [float(v) for v in element.text.split()]
            # AIXM here lists longitude first.
            return [(lat, lon) for lon, lat in zip(values[0::2], values[1::2])]
    raise ImportError_("MVA ring without a position list")


# --- Output ---------------------------------------------------------------


def fmt(value):
    return f"{value:.4f}".rstrip("0").rstrip(".")


def toml_ring(points):
    return "[" + ", ".join(f"[{fmt(x)}, {fmt(y)}]" for x, y in points) + "]"


def toml_names(names):
    return "[" + ", ".join(f'"{n}"' for n in names) + "]"


def render(data):
    out = list(data["header"])
    for key in ("name", "code", "data_source"):
        out.append(f'{key} = "{data[key]}"')
    for key in ("elevation", "magnetic_variation", "go_around_altitude", "mva_default",
                "departure_initial_altitude"):
        out.append(f"{key} = {fmt(data[key])}")
    out.append("")
    out.append("# Game-defined sector: the real T75 boundary isn't published.")
    out.append(f"sector = {toml_ring(data['sector'])}")
    out.append("")
    out.append("[traffic]")
    traffic = data["traffic"]
    out.append(f"start_rate = {fmt(traffic['start_rate'])}")
    out.append(f"end_rate = {fmt(traffic['end_rate'])}")
    out.append(f"arrival_share = {fmt(traffic['arrival_share'])}")
    out.append(f"callsign_prefixes = {toml_names(traffic['callsign_prefixes'])}")
    out.append(f"fleet = {toml_names(traffic['fleet'])}")
    out.append(f"fleet_weights = [{', '.join(fmt(w) for w in traffic['fleet_weights'])}]")
    for reference in data["references"]:
        out += ["", "[[references]]", f'name = "{reference["name"]}"']
        out += [f"x = {fmt(reference['x'])}", f"y = {fmt(reference['y'])}"]
    for runway in data["runways"]:
        out += ["", "[[runways]]", f'id = "{runway["id"]}"']
        for key in ("x", "y", "heading", "length", "start_x", "start_y"):
            out.append(f"{key} = {fmt(runway[key])}")
        out.append(f"ils = {'true' if runway['ils'] else 'false'}")
        if runway["ils"]:
            out.append(f"glideslope_deg = {fmt(runway['glideslope_deg'])}")
            out.append(f"threshold_crossing_height = {fmt(runway['tch'])}")
    for config in data["configs"]:
        out += ["", "[[configs]]"]
        for key in ("name", "arrival", "departure", "departure_long"):
            out.append(f'{key} = "{config[key]}"')
    for fix in data["fixes"]:
        out += ["", "[[fixes]]", f'name = "{fix["name"]}"']
        out += [f"x = {fmt(fix['x'])}", f"y = {fmt(fix['y'])}"]
    for star in data["stars"]:
        out += ["", "[[stars]]", f'name = "{star["name"]}"', f'config = "{star["config"]}"']
        out.append(f"fixes = {toml_names(star['fixes'])}")
        if star["final_heading"] is not None:
            out.append(f"final_heading = {fmt(star['final_heading'])}")
    for sid in data["sids"]:
        out += ["", "[[sids]]", f'name = "{sid["name"]}"', f'config = "{sid["config"]}"']
        out.append(f"fixes = {toml_names(sid['fixes'])}")
        out.append(f"exit_altitude = {fmt(sid['exit_altitude'])}")
    for area in data["mva"]:
        out += ["", "[[mva]]", f"altitude = {fmt(area['altitude'])}"]
        out.append(f"polygon = {toml_ring(area['polygon'])}")
        if area["holes"]:
            out.append("holes = [" + ", ".join(toml_ring(h) for h in area["holes"]) + "]")
    return "\n".join(out) + "\n"


# --- Build ----------------------------------------------------------------


def build(lines, nasr_lines, mva_root, overrides, airport_code, cycle, mva_description):
    records = [l for l in lines if l.startswith("S") and l[6:10] == airport_code and l[4:6] == "P "]
    if not records:
        raise ImportError_(f"no CIFP records for {airport_code}")
    airport = parse_airport(next(r for r in records if r[12] == "A"))
    origin = airport["pos"]
    variation = airport["variation"]
    radius = overrides["sector_radius_nm"]
    fixes = index_fixes(lines, airport_code)

    ils = {i["runway"]: i for i in (parse_ils(r) for r in records if r[12] == "I")}
    runways_raw = [parse_runway(r) for r in records if r[12] == "G"]
    ends = {}
    for line in nasr_lines:
        ends.update(parse_nasr_runway_ends(line))

    runways = []
    reciprocal = {"L": "R", "R": "L", "C": "C", "": ""}
    for raw in runways_raw:
        number = int(raw["id"][:2])
        side = raw["id"][2:]
        opposite = f"{(number + 18 - 1) % 36 + 1:02d}{reciprocal[side]}"
        if raw["id"] not in ends or opposite not in ends:
            raise ImportError_(f"NASR has no physical ends for runway {raw['id']}")
        start, far = project(ends[raw["id"]], origin), project(ends[opposite], origin)
        threshold = project(raw["threshold"], origin)
        info = ils.get(raw["id"])
        runways.append({
            "id": raw["id"],
            "x": threshold[0],
            "y": threshold[1],
            "heading": bearing(start, far),
            "length": (raw["length_ft"] - raw["displaced_ft"]) / FT_PER_NM,
            "start_x": start[0],
            "start_y": start[1],
            "ils": info is not None,
            "glideslope_deg": info["glideslope_deg"] if info else 0,
            "tch": info["tch"] if info else 0,
        })

    config_by_transition = {c["star_transition"]: c["name"] for c in overrides["configs"]}
    stars_raw = build_stars(legs_for(records, "E"), config_by_transition, variation)
    sids_raw = build_sids(legs_for(records, "D"), fixes, origin, radius)

    used = {}
    def use(legs):
        names = []
        for leg in legs:
            used[leg["fix"]] = project(resolve(leg, fixes), origin)
            names.append(leg["fix"])
        return dedupe_consecutive(names)

    stars = [
        {"name": s["name"], "config": s["config"], "fixes": use(s["legs"]),
         "final_heading": s["final_heading"]}
        for s in stars_raw
    ]
    sids = [
        {"name": s["name"], "config": c["name"], "fixes": use(s["legs"]),
         "exit_altitude": overrides["exit_altitude"]}
        for c in overrides["configs"]
        for s in sids_raw
    ]

    # Missed approach altitude: the ILS procedures' published climb.
    missed = [r for r in records if r[12] == "F" and r[13] == "I" and r[47:49] == "HM"]
    if not missed:
        raise ImportError_("no ILS missed approach hold found")
    go_around = max(int(r[84:89]) for r in missed)

    mva_areas = parse_mva(mva_root)
    mva = []
    for area in mva_areas:
        ring = [project(p, origin) for p in area["exterior"]]
        if any(in_circle(p, radius * 1.5) for p in ring):
            mva.append({
                "altitude": area["altitude"],
                "polygon": ring,
                "holes": [[project(p, origin) for p in h] for h in area["holes"]],
            })
    if not mva:
        raise ImportError_("no MVA sector overlaps the game sector")

    references = []
    for ref in overrides.get("references", []):
        record = next(
            (l for l in lines if l.startswith("S") and l[6:10] == ref["code"] and l[12] == "A"),
            None,
        )
        if record is None:
            raise ImportError_(f"no CIFP airport record for reference {ref['code']}")
        x, y = project(parse_airport(record)["pos"], origin)
        references.append({"name": ref["code"], "x": x, "y": y})

    sector = [
        (radius * math.sin(math.radians(a)), radius * math.cos(math.radians(a)))
        for a in range(0, 360, 360 // overrides["sector_sides"])
    ]

    return {
        "header": [
            f"# GENERATED by tools/import_faa.py from FAA CIFP {cycle}, NASR and {mva_description}.",
            "# Do not edit by hand: hand-set values are in tools/" + airport_code.lower() + "_overrides.toml.",
            "# NOT FOR NAVIGATION. See DATA_NOTICE.md for the FAA's required disclaimers.",
            "",
        ],
        "name": overrides["name"],
        "code": airport_code,
        "data_source": f"FAA data cycle {cycle} - NOT FOR NAVIGATION",
        "elevation": airport["elevation"],
        "magnetic_variation": variation,
        "go_around_altitude": float(go_around),
        "mva_default": max(a["altitude"] for a in mva_areas),
        "departure_initial_altitude": overrides["departure_initial_altitude"],
        "sector": sector,
        "traffic": overrides["traffic"],
        "references": references,
        "runways": runways,
        "configs": [
            {k: c[k] for k in ("name", "arrival", "departure", "departure_long")}
            for c in overrides["configs"]
        ],
        "fixes": [{"name": n, "x": p[0], "y": p[1]} for n, p in sorted(used.items())],
        "stars": stars,
        "sids": sids,
        "mva": mva,
    }


# --- Files ----------------------------------------------------------------


def fetch(url, name):
    CACHE.mkdir(parents=True, exist_ok=True)
    path = CACHE / name
    if not path.exists():
        print(f"downloading {url}")
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        with urllib.request.urlopen(request, timeout=300) as response:
            path.write_bytes(response.read())
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycle", required=True, help="CIFP cycle, e.g. 261029")
    parser.add_argument("--nasr", required=True, help="NASR cycle date, e.g. 2026-10-29")
    parser.add_argument("--airport", required=True, help="ICAO code, e.g. KSUS")
    parser.add_argument("--tracon", required=True, help="MVA chart facility, e.g. T75")
    args = parser.parse_args()

    cifp = fetch(
        f"https://aeronav.faa.gov/Upload_313-d/cifp/CIFP_{args.cycle}.zip",
        f"CIFP_{args.cycle}.zip",
    )
    apt = fetch(
        f"https://nfdc.faa.gov/webContent/28DaySub/{args.nasr}/APT.zip", f"APT_{args.nasr}.zip"
    )
    # FUS3 is the chart for 3 NM radar separation, which is what the game uses.
    mva = fetch(
        f"https://aeronav.faa.gov/MVA_Charts/aixm/{args.tracon}_MVA_FUS3.xml",
        f"{args.tracon}_MVA_FUS3.xml",
    )

    lines = zipfile.ZipFile(cifp).read(CIFP_MEMBER).decode("latin-1").splitlines()
    apt_lines = zipfile.ZipFile(apt).read("APT.txt").decode("latin-1").splitlines()
    lid = args.airport[1:]
    site = next(l[3:14] for l in apt_lines if l.startswith("APT") and l[27:31].strip() == lid)
    nasr_runways = [l for l in apt_lines if l.startswith("RWY") and l[3:14] == site]

    overrides_path = ROOT / "tools" / f"{args.airport.lower()}_overrides.toml"
    overrides = tomllib.loads(overrides_path.read_text(encoding="utf-8"))
    mva_root = ET.parse(mva).getroot()
    description = next(
        (e.text for e in mva_root.iter() if local_name(e) == "description"), "MVA chart"
    )

    data = build(lines, nasr_runways, mva_root, overrides, args.airport, args.cycle, description)
    output = ROOT / "assets" / "airports" / f"{args.airport.lower()}.toml"
    output.write_text(render(data), encoding="utf-8", newline="\n")
    print(
        f"wrote {output}: {len(data['fixes'])} fixes, {len(data['stars'])} STARs, "
        f"{len(data['sids'])} SIDs, {len(data['mva'])} MVA sectors, "
        f"go-around {data['go_around_altitude']:.0f} ft"
    )


if __name__ == "__main__":
    try:
        main()
    except ImportError_ as error:
        sys.exit(f"import failed: {error}")
