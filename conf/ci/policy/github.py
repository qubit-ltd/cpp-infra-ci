# SPDX-License-Identifier: Apache-2.0
"""Expose a validated matrix plan as GitHub job outputs without executing tools."""
import json
import os
import sys
from pathlib import Path


def main():
    data = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    outputs = {
        "matrix": json.dumps({"include": data["include"]}, separators=(",", ":")),
        "has_matrix": str(bool(data["include"])).lower(),
        "max_parallel": str(data["max_parallel"]),
        "light": data["light_task_ids"],
        "analysis": data["analysis_task_ids"],
        "hooks": data["hook_task_ids"],
        "pages": data["pages_task_ids"],
        "run_id": data["plan"]["run_id"],
    }
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for name, value in outputs.items():
            if "\n" in value or "\r" in value:
                raise ValueError("GitHub output contains a line break")
            output.write(f"{name}={value}\n")


if __name__ == "__main__":
    main()
