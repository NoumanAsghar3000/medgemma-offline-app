#!/usr/bin/env python3
"""Supervisor helper: confirm that a GitHub Actions run of this repository succeeded and that the downloadable installers are sound.
Needs the GitHub CLI (`gh auth login` once).   usage: python tools/verify_actions_run.py [--repo OWNER/REPO] [--run RUN_ID] [--keep DIR]
1. picks the latest run of the 'build-installer' workflow (or --run), 2. prints every job and step conclusion and fails if any job failed,
3. downloads the artifacts, 4. runs tools/check_artifacts.py on them. Deterministic - no browser, no LLM."""
import argparse, json, pathlib, subprocess, sys, tempfile

def gh(*a, check=True):
    r = subprocess.run(["gh", *a], capture_output=True, text=True)
    if check and r.returncode: sys.exit(f"gh {' '.join(a)} failed:\n{r.stderr.strip()}")
    return r.stdout

ap = argparse.ArgumentParser(); ap.add_argument("--repo"); ap.add_argument("--run"); ap.add_argument("--keep")
a = ap.parse_args(); repo = ["-R", a.repo] if a.repo else []
run = a.run or json.loads(gh("run", "list", *repo, "--workflow", "build-installer", "--limit", "1", "--json", "databaseId"))[0]["databaseId"]
info = json.loads(gh("run", "view", str(run), *repo, "--json", "conclusion,status,url,headSha,jobs"))
print(f"run {run}  status={info['status']}  conclusion={info['conclusion']}\n{info['url']}  commit {info['headSha'][:8]}")
bad = 0
for j in info["jobs"]:
    print(f"  job {j['name']}: {j['conclusion']}")
    for st in j.get("steps", []):
        if st.get("conclusion") not in ("success", "skipped"): print(f"      step '{st['name']}': {st['conclusion']}"); bad += 1
if info["conclusion"] != "success" or bad: sys.exit("RUN DID NOT SUCCEED - see the failing steps above")
dest = pathlib.Path(a.keep or tempfile.mkdtemp(prefix="mg_run_"))
gh("run", "download", str(run), *repo, "-D", str(dest))
print(f"artifacts downloaded to {dest}:"); [print("  ", p.relative_to(dest)) for p in sorted(dest.rglob("*")) if p.is_file()]
sys.exit(subprocess.call([sys.executable, str(pathlib.Path(__file__).with_name("check_artifacts.py")), str(dest)]))
