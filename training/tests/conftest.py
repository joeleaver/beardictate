import sys
from pathlib import Path

TRAINING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(TRAINING))
sys.path.insert(0, str(TRAINING.parent / "eval"))
