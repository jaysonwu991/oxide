#!/usr/bin/env python3
"""Inspect a PPTX without importing, extracting or changing it. Uses Python 3 only."""

import argparse
import json
import posixpath
import sys
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from urllib.parse import unquote, urlsplit


P = "http://schemas.openxmlformats.org/presentationml/2006/main"
A = "http://schemas.openxmlformats.org/drawingml/2006/main"
R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS = {"p": P, "a": A}


def relationship_part(part):
    folder, filename = posixpath.split(part)
    return posixpath.join(folder, "_rels", filename + ".rels")


def resolve_target(part, target):
    path = unquote(urlsplit(target).path)
    return posixpath.normpath(
        path.lstrip("/") if path.startswith("/")
        else posixpath.join(posixpath.dirname(part), path)
    )


def relationships(archive, part):
    name = relationship_part(part)
    if name not in archive.namelist():
        return {}
    root = ET.fromstring(archive.read(name))
    return {
        rel.get("Id"): {
            "type": rel.get("Type", "").rsplit("/", 1)[-1],
            "external": rel.get("TargetMode") == "External",
            "target": rel.get("Target", "") if rel.get("TargetMode") == "External"
            else resolve_target(part, rel.get("Target", "")),
        }
        for rel in root
    }


def paragraphs(root):
    return [
        "".join(el.text or "" for el in para.iter() if el.tag == f"{{{A}}}t")
        for para in root.findall(".//a:p", NS)
    ]


def graphic_counts(root):
    return {
        "images": len(root.findall(".//p:pic", NS)),
        "charts": sum(el.tag.rsplit("}", 1)[-1] == "chart" for el in root.iter()),
        "tables": len(root.findall(".//a:tbl", NS)),
        "connectors": len(root.findall(".//p:cxnSp", NS)),
        "custom_shapes": len(root.findall(".//a:custGeom", NS)),
        "groups": len(root.findall(".//p:grpSp", NS)),
    }


def declared_fonts(root):
    return sorted({
        el.get("typeface") for el in root.iter()
        if el.tag in {f"{{{A}}}{tag}" for tag in ("latin", "ea", "cs", "sym")}
        and el.get("typeface")
    })


def selected_slides(value, count):
    if not value:
        return set(range(1, count + 1))
    result = set()
    for item in value.split(","):
        bounds = item.strip().split("-")
        if len(bounds) > 2:
            raise ValueError("Use slide numbers or ranges, such as 1,3-5")
        start, end = int(bounds[0]), int(bounds[-1])
        if not 1 <= start <= end <= count:
            raise ValueError(f"Slide range {item!r} is outside 1-{count}")
        result.update(range(start, end + 1))
    return result


def inspect(path, selection=None, include_notes=False):
    with zipfile.ZipFile(path) as archive:
        names = set(archive.namelist())
        damaged = archive.testzip()
        errors = [f"CRC check failed: {damaged}"] if damaged else []
        for name in sorted(names):
            if not name.endswith(".rels"):
                continue
            folder, filename = posixpath.split(name)
            if name == "_rels/.rels":
                owner = ""
            elif posixpath.basename(folder) == "_rels":
                owner = posixpath.join(posixpath.dirname(folder), filename[:-5])
                if owner not in names:
                    errors.append(f"Relationship owner is missing: {owner}")
            else:
                continue
            for rel in ET.fromstring(archive.read(name)):
                if rel.get("TargetMode") == "External":
                    continue
                target = resolve_target(owner, rel.get("Target", ""))
                if target not in names:
                    errors.append(f"Missing target: {name} -> {target}")

        root = ET.fromstring(archive.read("ppt/presentation.xml"))
        if root.tag != f"{{{P}}}presentation":
            raise ValueError("Unsupported presentation namespace; use a compatible OOXML inspector")
        rels = relationships(archive, "ppt/presentation.xml")
        order = root.findall("p:sldIdLst/p:sldId", NS)
        selected = selected_slides(selection, len(order))
        size = root.find("p:sldSz", NS)
        if size is None:
            raise ValueError("Presentation has no slide dimensions")
        cx, cy = int(size.attrib["cx"]), int(size.attrib["cy"])
        report = {
            "path": str(Path(path).resolve()),
            "slide_count": len(order),
            "slide_size": {"emu": [cx, cy], "inches": [round(cx / 914400, 4), round(cy / 914400, 4)]},
            "package_errors": errors,
            "themes": [],
            "slides": [],
            "limitations": "Declared fonts are not resolved or verified as installed. Counts do not identify logos. No rendering, text-fit, chart-data, factual or native-app verification.",
        }
        for name in sorted(names):
            if name.startswith("ppt/theme/") and name.endswith(".xml"):
                theme = ET.fromstring(archive.read(name))
                palette = theme.find("a:themeElements/a:clrScheme", NS)
                report["themes"].append({
                    "part": name,
                    "name": theme.get("name"),
                    "declared_fonts": declared_fonts(theme),
                    "colors": {color.tag.rsplit("}", 1)[-1]: list(color)[0].attrib
                               for color in palette if len(color)} if palette is not None else {},
                })
        for number, entry in enumerate(order, 1):
            if number not in selected:
                continue
            rel = rels.get(entry.get(f"{{{R}}}id"))
            if not rel or rel["external"] or rel["type"] != "slide":
                errors.append(f"Slide {number} has no valid internal slide relationship")
                continue
            part = rel["target"]
            if part not in names:
                continue
            slide = ET.fromstring(archive.read(part))
            item = {
                "number": number,
                "part": part,
                "hidden": slide.get("show") in ("0", "false"),
                "paragraphs": paragraphs(slide),
                "declared_fonts": declared_fonts(slide),
                "graphics": graphic_counts(slide),
                "inherited": [],
            }
            current, seen = part, {part}
            for kind in ("slideLayout", "slideMaster"):
                parent = next((rel["target"] for rel in relationships(archive, current).values()
                               if rel["type"] == kind and not rel["external"]), None)
                if not parent or parent in seen or parent not in names:
                    break
                seen.add(parent)
                inherited = ET.fromstring(archive.read(parent))
                item["inherited"].append({"part": parent, "graphics": graphic_counts(inherited),
                                          "declared_fonts": declared_fonts(inherited)})
                current = parent
            if include_notes:
                note = next((rel["target"] for rel in relationships(archive, part).values()
                             if rel["type"] == "notesSlide" and not rel["external"]), None)
                item["notes"] = paragraphs(ET.fromstring(archive.read(note))) if note in names else []
            report["slides"].append(item)
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("presentation", type=Path)
    parser.add_argument("--slides", help="One-based slide numbers/ranges, e.g. 1,3-5")
    parser.add_argument("--notes", action="store_true", help="Include speaker-note paragraphs")
    parser.add_argument("--output", type=Path, help="Write complete JSON here and print a short summary")
    args = parser.parse_args()
    try:
        source = args.presentation.resolve()
        if args.output and (args.output.resolve() == source
                            or (args.output.exists() and args.output.samefile(source))):
            raise ValueError("The report path must differ from the source presentation")
        report = inspect(source, args.slides, args.notes)
        rendered = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
        if args.output:
            args.output.write_text(rendered, encoding="utf-8")
            print(json.dumps({"report": str(args.output.resolve()), "slide_count": report["slide_count"],
                              "selected_slides": len(report["slides"]), "package_errors": len(report["package_errors"])}))
        else:
            print(rendered, end="")
        return 2 if report["package_errors"] else 0
    except (OSError, ValueError, KeyError, ET.ParseError, zipfile.BadZipFile, RuntimeError) as error:
        print(f"PPTX inspection failed: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
