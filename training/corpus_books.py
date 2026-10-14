"""Candidate Gutenberg books: dialog-heavy novels by authors long dead.

`died` is the author's death year; fetch_corpus.py filters on it (default:
died in or before 1915, i.e. well over 100 years ago). A row whose title
does not match the downloaded header is marked `title-mismatch` in the
manifest and skipped by make_pairs.py, so a wrong ID is harmless.
"""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Book:
    id: int
    title: str
    author: str
    died: int


BOOKS: list[Book] = [
    # Jane Austen (1817)
    Book(1342, "Pride and Prejudice", "Austen", 1817),
    Book(158, "Emma", "Austen", 1817),
    Book(161, "Sense and Sensibility", "Austen", 1817),
    Book(105, "Persuasion", "Austen", 1817),
    Book(121, "Northanger Abbey", "Austen", 1817),
    Book(141, "Mansfield Park", "Austen", 1817),
    # Charles Dickens (1870)
    Book(98, "A Tale of Two Cities", "Dickens", 1870),
    Book(1400, "Great Expectations", "Dickens", 1870),
    Book(730, "Oliver Twist", "Dickens", 1870),
    Book(766, "David Copperfield", "Dickens", 1870),
    Book(46, "A Christmas Carol", "Dickens", 1870),
    Book(580, "Pickwick Papers", "Dickens", 1870),
    Book(967, "Nicholas Nickleby", "Dickens", 1870),
    Book(1023, "Bleak House", "Dickens", 1870),
    Book(963, "Little Dorrit", "Dickens", 1870),
    Book(821, "Dombey and Son", "Dickens", 1870),
    Book(917, "Barnaby Rudge", "Dickens", 1870),
    Book(700, "The Old Curiosity Shop", "Dickens", 1870),
    Book(883, "Our Mutual Friend", "Dickens", 1870),
    Book(786, "Hard Times", "Dickens", 1870),
    Book(968, "Martin Chuzzlewit", "Dickens", 1870),
    # Mark Twain (1910)
    Book(74, "The Adventures of Tom Sawyer", "Twain", 1910),
    Book(86, "A Connecticut Yankee in King Arthur's Court", "Twain", 1910),
    Book(1837, "The Prince and the Pauper", "Twain", 1910),
    Book(102, "Pudd'nhead Wilson", "Twain", 1910),
    # Louisa May Alcott (1888)
    Book(514, "Little Women", "Alcott", 1888),
    Book(2726, "Eight Cousins", "Alcott", 1888),
    Book(2788, "Little Men", "Alcott", 1888),
    Book(3499, "Jo's Boys", "Alcott", 1888),
    Book(2804, "Rose in Bloom", "Alcott", 1888),
    # Brontës (1848-1855)
    Book(1260, "Jane Eyre", "C. Bronte", 1855),
    Book(768, "Wuthering Heights", "E. Bronte", 1848),
    Book(969, "The Tenant of Wildfell Hall", "A. Bronte", 1849),
    Book(767, "Agnes Grey", "A. Bronte", 1849),
    Book(9182, "Villette", "C. Bronte", 1855),
    # George Eliot (1880)
    Book(145, "Middlemarch", "Eliot", 1880),
    Book(6688, "The Mill on the Floss", "Eliot", 1880),
    Book(550, "Silas Marner", "Eliot", 1880),
    Book(507, "Adam Bede", "Eliot", 1880),
    # Elizabeth Gaskell (1865)
    Book(4276, "North and South", "Gaskell", 1865),
    Book(394, "Cranford", "Gaskell", 1865),
    Book(4274, "Wives and Daughters", "Gaskell", 1865),
    # Wilkie Collins (1889)
    Book(583, "The Woman in White", "Collins", 1889),
    Book(155, "The Moonstone", "Collins", 1889),
    # Others
    Book(599, "Vanity Fair", "Thackeray", 1863),
    Book(120, "Treasure Island", "Stevenson", 1894),
    Book(421, "Kidnapped", "Stevenson", 1894),
    Book(43, "The Strange Case of Dr. Jekyll and Mr. Hyde", "Stevenson", 1894),
    Book(33, "The Scarlet Letter", "Hawthorne", 1864),
    Book(77, "The House of the Seven Gables", "Hawthorne", 1864),
    Book(11, "Alice's Adventures in Wonderland", "Carroll", 1898),
    Book(12, "Through the Looking-Glass", "Carroll", 1898),
    Book(174, "The Picture of Dorian Gray", "Wilde", 1900),
    Book(73, "The Red Badge of Courage", "Crane", 1900),
    Book(160, "The Awakening", "Chopin", 1904),
    Book(619, "The Warden", "Trollope", 1882),
    Book(8954, "Lady Audley's Secret", "Braddon", 1915),
    Book(1480, "Tom Brown's School Days", "Hughes", 1896),
    Book(2145, "Ben-Hur", "Wallace", 1905),
    Book(271, "Black Beauty", "Sewell", 1878),
    # Excluded by the default --max-died 1915 (public domain, but the authors
    # died less than ~110 years ago): Doyle (1930), Wharton (1937), Hardy (1928).
    Book(1661, "The Adventures of Sherlock Holmes", "Doyle", 1930),
    Book(541, "The Age of Innocence", "Wharton", 1937),
]
