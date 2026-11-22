import hashlib
import json
from pathlib import Path

project = Path(__file__).resolve().parents[1]
root = project / "dev-data/performance-acceptance"

def read(name):
    return json.loads((root / name).read_text())

local = read("local-search/results.json")
semantic = read("semantic/report.json")
audit = read("semantic/diagnose-report.json")
mcp = read("semantic/mcp-report.json")
cold = read("semantic/mcp-cold-recheck.json")
archive = read("archive-idempotency/report.json")
state = read("user-state-after.json")
environment = read("environment.json")
corpus_unchanged = hashlib.sha256((project / "src-tauri/tests/fixtures/semantic-corpus.json").read_bytes()).hexdigest() == environment["sourceSha256"]
matrix = []
for dataset in local["datasets"]:
    scopes = dataset["scopes"]
    matrix.append({
        "perRepository": dataset["perRepository"],
        "totalMemories": dataset["totalMemories"],
        "currentRepositoryWarmLocalP95MaxMs": max(scope["warmCached"]["localSearchMs"]["p95"] for scope in scopes if scope["repositoryId"]),
        "allRepositoriesWarmLocalP95Ms": next(scope["warmCached"]["localSearchMs"]["p95"] for scope in scopes if scope["scope"] == "all-repositories"),
        "processColdLocalP95MaxMs": max(scope[phase]["localSearchMs"]["p95"] for scope in scopes for phase in ["processColdSnapshotCachedQuery", "processColdSnapshotFreshQuery"]),
    })

tokens = audit["memoryVectors"]["inputTokens"] + sum(scale["queryInputTokens"] for scale in audit["scales"])
source_checks = all(memory["sourceExact"] and memory["markdownExact"] for scale in semantic["scales"] for memory in scale["reads"])
mcp_reads = all(memory["originalSourceMatched"] and memory["sectionTitleAndIdAgree"] and memory["fullBodyMatched"] for scale in mcp["scales"] for memory in scale["reads"])
local_target = all(row["processColdLocalP95MaxMs"] < 200 and row["currentRepositoryWarmLocalP95MaxMs"] < 200 and row["allRepositoriesWarmLocalP95Ms"] < 200 for row in matrix)
all_known = all(scale["metrics"]["recallAt5"] == 1 for scale in semantic["scales"])
data_preserved = all(value for key, value in state["checks"].items() if key != "settingsHash")
result = {
    "date": "2026-11-21",
    "platform": {key: environment[key] for key in ["arch", "macOS", "build"]},
    "acceptanceWorkCompleted": True,
    "allCriteriaPassed": all([corpus_unchanged, local_target, all_known, source_checks, mcp_reads, data_preserved]),
    "verdict": "本地检索性能、用量记账、缓存、逐字读取与归档幂等性通过；召回存在一个未通过题，MCP首次整库加载另列。",
    "predeclaredCorpusUnchanged": corpus_unchanged,
    "localSearch": matrix,
    "localSearch200MsTargetPassed": local_target,
    "semantic": [{"perRepository": scale["perRepository"], **scale["metrics"]} for scale in semantic["scales"]],
    "allKnownAnswersInTop5": all_known,
    "exactLocalReads": {"passed": source_checks, "goldInstances": sum(len(scale["reads"]) for scale in semantic["scales"])},
    "actualMcpReads": {"passed": mcp_reads, "goldInstances": sum(len(scale["reads"]) for scale in mcp["scales"])},
    "actualMcpCacheUsageUnchanged": all(scale["apiUsageBefore"] == scale["apiUsageAfter"] for scale in mcp["scales"] + cold["scales"]),
    "remoteEmbeddingUsage": {"requests": audit["journal"]["pairs"], "inputTokens": tokens, "allUsageReported": audit["journal"]["allUsageReported"], "model": semantic["model"]},
    "duplicateArchive": {"transport": archive["transport"], "cases": archive["repeatChecks"], "usageOrigin": archive["usageOrigin"]},
    "originalUserDataAndUsagePreserved": data_preserved,
    "originalSettingsSameBytes": state["checks"]["settingsHash"],
    "settingsObservation": "期间聊天配置发生变化；测试仅加载本机服务配置与读取向量凭据，未回滚当前配置。变化来源未由本次验收确定。",
    "limitations": [
        "本地基准使用合成1536维向量；真实向量召回语料只有48种不同检索文本，以重复干扰条目扩容，不代表3万种独立语义。",
        "冷进程允许操作系统文件缓存仍热；200毫秒目标是核心本地检索，不含API、MCP首次Markdown全库刷新、凭据读取与传输。",
        "Q-S04的预期记忆在独立检索文本中排第4，但重复候选令实际行排名为298/2998；未改题或修改检索算法。",
        "MCP最初1k整库工具调用24.13秒异常样本保留，三新进程复测单列，未确定该单次异常的具体原因。",
        "工具返回仍占调用方上下文；未使用tokenizer，其token数保持null，另记录UTF-8字节。",
        "归档零新增调用采用持续监听的localhost协议样本；不是本轮真实远程聊天调用验收。",
        "本机macOS已测；Windows未测。",
    ],
}
(root / "summary.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({key: result[key] for key in ["allCriteriaPassed", "localSearch200MsTargetPassed", "allKnownAnswersInTop5", "remoteEmbeddingUsage", "originalUserDataAndUsagePreserved", "originalSettingsSameBytes"]}, ensure_ascii=False))
