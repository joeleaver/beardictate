"""Builds spelling.tsv, the American/British table bd-vocab's spelling pass
uses, from VarCon (SCOWL, http://wordlist.aspell.net/varcon/).

    curl -sSLo varcon.txt https://raw.githubusercontent.com/en-wl/wordlist/master/varcon/varcon.txt
    python3 -I make_spelling.py varcon.txt > spelling.tsv

Built 2026-10-20 from varcon.txt sha256 75af63da46ec12d7eb14b9f1ba8d3898d484dd6872755b73c921b215875a3629.

Only clusters at SCOWL level 70 or below (words a dictionary has). A word
becomes the other spelling only when VarCon never gives it as that
spelling's preferred form for any meaning: "check" (verify) is British too,
so it never becomes "cheque"; "cheque" always becomes "check". Meanings
VarCon marks rare or archaic, "(-)", are left out.

Output lines: `<word>\t<as American>\t<as British>`, either side empty when
the word stays as it is that way.
"""

import re
import sys
from collections import defaultdict

NOTICE = """# Made from VarCon by crates/bd-vocab/data/make_spelling.py; see there.
#
# VarCon: Copyright 2000-2019 by Kevin Atkinson; Copyright 2016 by
# Benjamin Titze. Permission to use, copy, modify, distribute and sell this
# array, the associated software, and its documentation for any purpose is
# hereby granted without fee, provided that the above copyright notice
# appears in all copies and that both that copyright notice and this
# permission notice appear in supporting documentation. Kevin Atkinson and
# Benjamin Titze make no representations about the suitability of this
# array for any purpose. It is provided "as is" without express or implied
# warranty. (The original word lists come from the Ispell distribution,
# Copyright 1993, Geoff Kuenning; see packaging/THIRD-PARTY.txt.) Modified:
# reduced to American/British word pairs.
"""

WORD = re.compile(r"^[a-z][a-z'-]*$")


def parse(lines):
    """Yields each entry line's {word: [tags]}, with its cluster's level."""
    level = 100
    for raw in lines:
        line = raw.rstrip("\n")
        if line.startswith("#"):
            m = re.search(r"\(level (\d+)\)", line)
            level = int(m.group(1)) if m else 100
            continue
        body, _, usage = line.partition(" | ")
        body = body.strip()
        # "(-)": a rare or archaic meaning (gray, the unit of radiation;
        # barque for bark), which neither blocks nor makes a conversion.
        if not body or "(-)" in usage:
            continue
        forms = {}
        for part in body.split(" / "):
            if ": " not in part:
                continue
            tags, word = part.split(": ", 1)
            forms.setdefault(word.strip(), []).extend(t for t in tags.split() if not t.isdigit())
        yield level, forms


def preferred(forms, cat):
    return next((w for w, tags in forms.items() if cat in tags), None)


def main(path):
    to_us, to_uk = defaultdict(set), defaultdict(set)
    pref_us, pref_uk = set(), set()
    with open(path, encoding="latin-1") as f:
        for level, forms in parse(f):
            us, uk = preferred(forms, "A"), preferred(forms, "B")
            if us:
                pref_us.add(us)
            if uk:
                pref_uk.add(uk)
            if level > 70 or not us or not uk or us == uk:
                continue
            for word, tags in forms.items():
                if word != us and any(t[0] in "BZ" for t in tags):
                    to_us[word].add(us)
            to_uk[us].add(uk)
    table = defaultdict(lambda: ["", ""])
    for word, targets in to_us.items():
        if len(targets) == 1 and word not in pref_us:
            table[word][0] = next(iter(targets))
    for word, targets in to_uk.items():
        if len(targets) == 1 and word not in pref_uk:
            table[word][1] = next(iter(targets))
    out = sys.stdout
    out.write(NOTICE)
    for word in sorted(table):
        us, uk = table[word]
        if WORD.match(word) and all(not t or WORD.match(t) for t in (us, uk)):
            out.write(f"{word}\t{us}\t{uk}\n")


if __name__ == "__main__":
    main(sys.argv[1])
