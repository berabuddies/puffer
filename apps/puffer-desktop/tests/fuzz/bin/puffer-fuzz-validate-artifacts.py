#!/usr/bin/env python3
import argparse
import json
from pathlib import Path

import jsonschema


def main():
    parser = argparse.ArgumentParser(description="Validate Puffer UI fuzz artifacts against local schemas.")
    parser.add_argument("--evidence", action="append", default=[], help="Replay report with evidence_index/evidence_source")
    parser.add_argument("--verdict", action="append", default=[], help="Strict verdict JSON artifact")
    parser.add_argument("--gate", action="append", default=[], help="Verdict gate JSON artifact")
    parser.add_argument("--reviewer", action="append", default=[], help="Reviewer decision JSON artifact")
    args = parser.parse_args()

    root = Path(__file__).resolve().parents[1]
    schemas = {
        "evidence": load_json(root / "schemas" / "evidence-index.schema.json"),
        "verdict": load_json(root / "schemas" / "verdict.schema.json"),
        "gate": load_json(root / "schemas" / "verdict-gate.schema.json"),
        "reviewer": load_json(root / "schemas" / "reviewer.schema.json"),
    }
    checks = []
    for kind, files in {
        "evidence": args.evidence,
        "verdict": args.verdict,
        "gate": args.gate,
        "reviewer": args.reviewer,
    }.items():
        for file_name in files:
            file_path = Path(file_name)
            jsonschema.validate(load_json(file_path), schemas[kind])
            checks.append({"kind": kind, "path": str(file_path)})

    print(json.dumps({"version": 1, "validated": checks}, indent=2))


def load_json(path):
    with Path(path).open("r", encoding="utf-8") as handle:
        return json.load(handle)


if __name__ == "__main__":
    main()
