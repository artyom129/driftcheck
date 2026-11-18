"""Time SyncProof's own loader and matcher on a generated migration.

    python bench/syncproof_bench.py <path to the syncproof repo> <data dir>

The data dir comes from `driftcheck gen`. The script prints one JSON line with
the load time, the reconcile time and the counts, so the two tools can be
compared on the same files.
"""
import json
import sys
import time
from pathlib import Path

repo, data = Path(sys.argv[1]), Path(sys.argv[2])
sys.path.insert(0, str(repo))

from app.matcher import reconcile  # noqa: E402
from app.models import ReconciliationConfig  # noqa: E402
from app.profiler import load_records  # noqa: E402

started = time.perf_counter()
source = load_records(data / "legacy.csv")
target = load_records(data / "migrated.csv")
loaded = time.perf_counter()
config = ReconciliationConfig.model_validate(json.loads((data / "mapping.json").read_text(encoding="utf-8")))
result = reconcile(source, target, config)
done = time.perf_counter()

print(json.dumps({
    "tool": "syncproof",
    "rows": len(source),
    "load_s": round(loaded - started, 3),
    "reconcile_s": round(done - loaded, 3),
    "counts": result.summary["counts"],
    "quality_score": result.summary["quality_score"],
}))
