"""Real-model integration check against this repository, with local measurements."""
import json
import platform
import threading
import time
from pathlib import Path
import psutil
from loft_bench import ROOT
from loft_engine import Engine


def main():
    output = ROOT / "results" / ("engine-" + str(time.time_ns()))
    output.mkdir(parents=True)
    engine = Engine(output / "library.sqlite3")
    cancel = threading.Event()
    report = {"platform": platform.platform(), "memory_bytes": psutil.virtual_memory().total,
              "logical_cpus": psutil.cpu_count(), "models": engine.models.manifest, "status": "incomplete"}
    def progress(kind, value):
        if kind == "status":
            print(value, flush=True)
    try:
        report["initial_index"] = engine.index_folder(ROOT, cancel, progress)
        report["unchanged_index"] = engine.index_folder(ROOT, cancel, progress)
        report["queries"] = []
        for question, scope in [("How long does the expand and collapse animation take?", "Documents"),
                                ("What does NomicEmbedding.embed add before the input text?", "Code"),
                                ("What is the registration number of my car?", "Documents")]:
            started = time.perf_counter()
            first = []
            warnings = []
            def emit(kind, value):
                progress(kind, value)
                if kind == "token" and not first:
                    first.append(time.perf_counter()-started)
                if kind == "warning":
                    warnings.append(value)
            result = engine.ask(question, scope, cancel, emit)
            report["queries"].append({"question": question, "scope": scope, **result,
                                      "first_content_seconds": first[0] if first else None, "warnings": warnings})
            print(result["answer"], flush=True)
        report["runtime_measurements"] = engine.models.measurements
        assert report["unchanged_index"]["indexed"] == 0
        first = report["queries"][0]
        assert "240" in (first["answer"] or "") and "[S" in first["answer"]
        assert any("240 ms" in hit["text"] for hit in first["hits"])
        assert report["queries"][-1]["answer"] is None, "Missing evidence must not trigger generation"
        report["status"] = "complete"
    finally:
        (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"Report: {output / 'report.json'}", flush=True)


if __name__ == "__main__":
    main()
