import hashlib
import json
import sqlite3
import sys
from pathlib import Path

project = Path(__file__).resolve().parents[1]
reports = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else project / "dev-data/performance-acceptance"
reports.mkdir(parents=True, exist_ok=True)
data = project / "dev-data"
vault = Path(json.loads((data / "workspace.json").read_text())["root"])
hash_file = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
files = {
    str(path.relative_to(vault)): hash_file(path)
    for path in vault.rglob("*.md")
    if ".aimemory" not in path.parts
}
db = sqlite3.connect((vault / ".aimemory/pipeline.sqlite").as_uri() + "?mode=ro", uri=True)
snapshot = {
    "vault": str(vault),
    "markdownHashes": files,
    "settingsHash": hash_file(data / "services.json"),
    "workspaceHash": hash_file(data / "workspace.json"),
    "apiRequestsAndInputTokens": list(db.execute("SELECT COUNT(*),COALESCE(SUM(input_tokens),0) FROM api_usage").fetchone()),
    "chatInputAndOutputTokens": list(db.execute("SELECT COALESCE(SUM(chat_input),0),COALESCE(SUM(chat_output),0) FROM turns").fetchone()),
}
db.close()
phase = sys.argv[1]
if phase == "before":
    (reports / "user-state-before.json").write_text(json.dumps(snapshot, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"phase": phase, "memories": len(files), "apiUsage": snapshot["apiRequestsAndInputTokens"]}))
elif phase == "after":
    before = json.loads((reports / "user-state-before.json").read_text())
    result = {"passed": before == snapshot, "checks": {key: before[key] == value for key, value in snapshot.items()}, "snapshot": snapshot}
    (reports / "user-state-after.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"phase": phase, "passed": result["passed"], "checks": result["checks"]}))
    assert result["passed"], "验收期间原工作区数据或用量发生变化，请检查报告"
else:
    raise SystemExit("使用 before 或 after")
