"""Builds names.txt, the words bd-vocab's `is_name` knows as names (people,
places, months, days, peoples), from SCOWL's American English word list
(http://wordlist.aspell.net/, Debian's wamerican package):

    python3 -I make_names.py /usr/share/dict/american-english > names.txt

Built 2026-11-10 from wamerican 2020.12.07-4build1.

A name is a capitalized entry (Sarah, Monday, Glasgow) whose lower-case
form isn't a word too: "Will", "Begin" and "Rather" are words first, so
they're left out, but for the names in KEEP (months, given names). Entries
of two letters are mostly chemical symbols ("Er", "Co"), so only the
titles and names among them are kept. Used to keep a capital the
dictation would otherwise lower (the first word after a comma).
"""

import sys

NOTICE = """# Made from SCOWL by crates/bd-vocab/data/make_names.py; see there.
#
# SCOWL: Copyright 2000-2011 by Kevin Atkinson. Permission to use, copy,
# modify, distribute and sell these word lists, the associated scripts, the
# output created from the scripts, and its documentation for any purpose is
# hereby granted without fee, provided that the above copyright notice
# appears in all copies and that both that copyright notice and this
# permission notice appear in supporting documentation. Kevin Atkinson
# makes no representations about the suitability of this array for any
# purpose. It is provided "as is" without express or implied warranty."""

# Names that are words too, kept as names: what would look wrong lowered.
KEEP = """
March May August God Lord
Will Mark Bill Bob Bobby Pat Jack Frank Rose Grace Hope Joy Ray Sue Don Tom
Max Ken Lee Guy Dawn Eve Ed Dean Drew Rich Rob Nick Mike Matt John Rick Josh
Brad Chad Hank Jay Peter Harry Billy Jimmy Sally Sandy Holly Heather Hazel
Jasmine Daisy Penny Carol Robin Martin Victor Roger Terry Faith Hector Homer
Kitty Lance Olive Pearl Ruby Violet Lily Ivy Iris Amber Crystal Ginger Melody
Sherry Dolly Patty Patsy Sonny Chuck Cliff Basil Gene Earl Glen Dale Wade
Bud Buddy Jerry Laurel Myrtle Prudence Felicity Charity Marina Maria Colleen
Bonnie Barney Mickey Rusty Dusty Misty Rocky Randy
French Welsh Polish Google Android Amazon Twitter
"""

TWO_LETTER = {"Mr", "Dr", "St", "Jr", "Sr", "Mt", "Lt", "Jo", "Al", "Ed", "Ty", "Bo"}


def main(path):
    words = [w.strip() for w in open(path, encoding="utf-8")]
    lower = {w for w in words if w and w == w.lower()}
    keep = set(KEEP.split())
    names = set()
    for w in words:
        if len(w) < 2 or not w[0].isupper() or not w[1:].islower() or not w.isalpha():
            continue
        if len(w) == 2 and w not in TWO_LETTER:
            continue
        if w.lower() in lower and w not in keep:
            continue
        names.add(w)
    print(NOTICE)
    for w in sorted(names):
        print(w)


if __name__ == "__main__":
    main(sys.argv[1])
