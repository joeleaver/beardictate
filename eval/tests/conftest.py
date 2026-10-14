import sys
from pathlib import Path

EVAL = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVAL))
sys.path.insert(0, str(EVAL.parent / "training"))
