#!/usr/bin/env python3
"""Build the offline place gazetteer for Kadr (city scale).

Sources (GeoNames, CC-BY 4.0 — attribution in NOTICE):
  - cities500.zip   all cities with population >= 500
  - alternateNames.zip  filtered to Ukrainian names
Output: src-tauri/resources/geobase.tsv
  columns: geonameid, lat, lon, population, name_ascii, name_uk (may be empty)
"""
import io
import os
import urllib.request
import zipfile

BASE = "http://download.geonames.org/export/dump/"
OUT = "src-tauri/resources/geobase.tsv"


def fetch(url: str) -> bytes:
    print("fetching", url)
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def main() -> None:
    cities_raw = zipfile.ZipFile(io.BytesIO(fetch(BASE + "cities500.zip"))).read("cities500.txt").decode("utf-8")
    alt_raw = zipfile.ZipFile(io.BytesIO(fetch(BASE + "alternateNames.zip"))).read("alternateNames.txt").decode("utf-8")

    uk_names: dict[str, str] = {}
    preferred: dict[str, str] = {}
    for line in alt_raw.splitlines():
        f = line.split("\t")
        # alternatenamingid, geonameid, isolang, name, isPreferred, ...
        if len(f) < 4 or f[2] != "uk":
            continue
        gid, name, pref = f[1], f[3], len(f) > 4 and f[4] == "1"
        if pref and gid not in preferred:
            preferred[gid] = name
        elif gid not in uk_names:
            uk_names[gid] = name

    rows = []
    for line in cities_raw.splitlines():
        f = line.split("\t")
        if len(f) < 19:
            continue
        gid, name, ascii_name, lat, lon, pop = f[0], f[1], f[2], f[4], f[5], f[14]
        uk = preferred.get(gid) or uk_names.get(gid) or ""
        rows.append((gid, lat, lon, pop, ascii_name or name, uk))

    rows.sort(key=lambda r: -int(r[3]))
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as o:
        o.write("geonameid\tlat\tlon\tpopulation\tname_ascii\tname_uk\n")
        for r in rows:
            o.write("\t".join(r) + "\n")
    have_uk = sum(1 for r in rows if r[5])
    print(f"wrote {len(rows)} places to {OUT} ({have_uk} with Ukrainian names)")


if __name__ == "__main__":
    main()
