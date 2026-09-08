#!/usr/bin/env python3
"""Compare `sequin group <dir>` output against the golden fixture.

The fixture stores filenames only -- no pixels, no hashes -- because photos
are never committed (see CLAUDE.md, "No photos in git, ever"). So the check is
set equality over group membership: the grouping is correct iff every group's
sorted filename set matches, 34 groups covering 62 photos.

Usage:
    sequin group <dir> > out.json
    scripts/golden_check.py out.json [fixture.json]

Exits non-zero and prints the group-set diff on any mismatch.
Self-test: python3 -m doctest scripts/golden_check.py
"""
import json
import sys
from collections import Counter
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_FIXTURE = REPO_ROOT / "fixtures" / "expected_groups_archive1-2.json"


class InputError(Exception):
    """A bad input file, reported as a message rather than a traceback."""


def scan_groups(doc, path):
    """Basenames per group from `sequin group` JSON.

    The shape mirrors sequin-core's `Arrangement`: groups of photo objects
    keyed by `path`. `scan_dir` does not recurse, so basenames are unique and
    safe to compare against the fixture. Deliberately strict about the shape:
    the fixture's bare-filename groups must NOT parse here, or passing the
    fixture as the actual output would report a false pass.

    >>> doc = {"groups": [{"photos": [{"path": "/d/b.jpg"}, {"path": "/d/a.jpg"}]}]}
    >>> scan_groups(doc, "out.json")
    [['b.jpg', 'a.jpg']]
    >>> try:
    ...     scan_groups({"groups": [["a.jpg"]]}, "out.json")   # the fixture's shape
    ... except InputError as e:
    ...     print(e)
    out.json is not `sequin group` output (expected groups[].photos[].path)
    """
    try:
        return [[Path(p["path"]).name for p in g["photos"]] for g in doc["groups"]]
    except (KeyError, TypeError) as e:
        raise InputError(
            f"{path} is not `sequin group` output (expected groups[].photos[].path)"
        ) from e


def fixture_groups(doc, path):
    """Filename lists from a golden fixture, whose groups are already bare names.

    >>> fixture_groups({"groups": [["b.jpg", "a.jpg"]]}, "fixture.json")
    [['b.jpg', 'a.jpg']]
    >>> try:
    ...     fixture_groups({"groups": [42]}, "fixture.json")
    ... except InputError as e:
    ...     print(e)
    fixture.json is not a golden fixture (expected groups[] of filename lists)

    Entries must be strings too -- a nested list would otherwise reach
    `normalize` and blow up on `sorted` with an unorderable type:

    >>> try:
    ...     fixture_groups({"groups": [["a.jpg", ["b.jpg"]]]}, "fixture.json")
    ... except InputError as e:
    ...     print(e)
    fixture.json is not a golden fixture (expected groups[] of filename lists)
    """
    groups = doc.get("groups") if isinstance(doc, dict) else None
    if (
        not isinstance(groups, list)
        or not all(isinstance(g, list) for g in groups)
        or not all(isinstance(name, str) for g in groups for name in g)
    ):
        raise InputError(
            f"{path} is not a golden fixture (expected groups[] of filename lists)"
        )
    return groups


def normalize(groups):
    """Group -> sorted tuple of basenames; whole set sorted for order-independence.

    >>> normalize([["b.jpg", "a.jpg"], ["c.jpg"]]) == normalize([["c.jpg"], ["a.jpg", "b.jpg"]])
    True
    >>> normalize([["a.jpg", "b.jpg"]]) == normalize([["a.jpg"], ["b.jpg"]])
    False
    """
    return sorted(tuple(sorted(g)) for g in groups)


def diff_groups(actual, expected):
    """Groups present the wrong number of times on each side.

    A duplicate is a genuine mismatch that set membership cannot see:

    >>> diff_groups([("a",), ("a",)], [("a",)])
    ([], [('a',)])
    >>> diff_groups([("a",)], [("a",), ("b",)])
    ([('b',)], [])
    """
    a_counts, e_counts = Counter(actual), Counter(expected)
    missing = sorted((e_counts - a_counts).elements())
    extra = sorted((a_counts - e_counts).elements())
    return missing, extra


def read_json(path):
    """Parse a JSON file, reporting an unreadable or malformed one as InputError.

    The malformed case is the one that bites: a run whose redirect was
    forgotten, or whose CLI died partway, leaves an empty or truncated file
    rather than valid output.

    >>> try:
    ...     read_json("/nonexistent/out.json")
    ... except InputError as e:
    ...     print(e)
    cannot read /nonexistent/out.json: No such file or directory
    >>> import tempfile
    >>> with tempfile.TemporaryDirectory() as d:
    ...     truncated = Path(d) / "out.json"
    ...     _ = truncated.write_text('{"groups": [')   # CLI died mid-write
    ...     try:
    ...         read_json(truncated)
    ...     except InputError as e:
    ...         print(str(e).startswith(f"{truncated} is not valid JSON:"))
    True
    """
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except OSError as e:
        raise InputError(f"cannot read {path}: {e.strerror or e}") from e
    except UnicodeDecodeError as e:
        # A binary file handed over by mistake (a .jpg, a stray .dmg) decodes
        # as neither UTF-8 nor JSON; report the path, not the codec offsets.
        raise InputError(f"{path} is not UTF-8 text: {e.reason}") from e
    except json.JSONDecodeError as e:
        raise InputError(f"{path} is not valid JSON: {e}") from e


def main():
    if len(sys.argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    actual_path = sys.argv[1]
    fixture_path = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_FIXTURE

    try:
        actual = normalize(scan_groups(read_json(actual_path), actual_path))
        expected = normalize(fixture_groups(read_json(fixture_path), fixture_path))
    except InputError as e:
        print(e, file=sys.stderr)
        return 2

    actual_photos = sum(len(g) for g in actual)
    expected_photos = sum(len(g) for g in expected)
    print(f"actual:   {len(actual)} groups / {actual_photos} photos")
    print(f"expected: {len(expected)} groups / {expected_photos} photos")

    if actual == expected:
        print("\nGOLDEN TEST PASSED — group sets match exactly.")
        return 0

    missing, extra = diff_groups(actual, expected)
    print(f"\nGOLDEN TEST FAILED — {len(missing)} expected group(s) absent, "
          f"{len(extra)} unexpected group(s).")
    for g in missing:
        print(f"  - expected: {list(g)}")
    for g in extra:
        print(f"  + actual:   {list(g)}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
