#!/usr/bin/env python3
"""
Prepare the MedGemma model folder that the installer / app will use.   (Run ONCE, on any PC with internet.)

Resulting folder (this is exactly what mg-core scans - names only need to contain "27b"/"4b",
the quant tag, and "mmproj" for the vision projector):

    MedGemmaModels/
      medgemma-27b-it-Q8_0.gguf   medgemma-27b-it-Q6_K.gguf   medgemma-27b-it-Q5_K_M.gguf
      medgemma-27b-it-Q4_K_M.gguf medgemma-27b-it-Q3_K_M.gguf
      mmproj-medgemma-27b-it-F16.gguf                        <-- REQUIRED for 27B (vision)
      medgemma-4b-it-Q8_0.gguf    medgemma-4b-it-Q4_K_M.gguf
      mmproj-medgemma-4b-it-F16.gguf                         <-- REQUIRED for 4B vision

Why a ladder of pre-made quantizations instead of quantizing during installation?
Quantizing 27B needs the 54 GB F16 file + ~60 GB RAM/disk and takes 10-30 min. Doing that inside an
installer on a consumer PC is slow and fragile. Instead the installer *selects* (and optionally copies)
the largest pre-built quantization that fits free RAM - 1 GB. Which rungs you ship = your quality floor.

Modes
  download : fetch ready-made GGUF + mmproj from Hugging Face (repos below; override with --repo-27b/--repo-4b)
  convert  : you have your own HF safetensors (e.g. your fine-tune) -> F16 GGUF + mmproj -> quant ladder
             needs a llama.cpp checkout (convert_hf_to_gguf.py) and the llama-quantize binary.
"""
import argparse, os, subprocess, sys, shutil

LADDER_27B = ["Q8_0", "Q6_K", "Q5_K_M", "Q4_K_M", "Q3_K_M"]   # add "Q2_K" only if you accept the quality loss
LADDER_4B = ["Q8_0", "Q6_K", "Q4_K_M"]

# Community/official GGUF repos. VERIFY these names on huggingface.co before relying on them.
DEFAULT_REPOS = {"27b": "unsloth/medgemma-27b-it-GGUF", "4b": "unsloth/medgemma-4b-it-GGUF"}


def download(a):
    from huggingface_hub import hf_hub_download, list_repo_files  # pip install huggingface_hub
    os.makedirs(a.out, exist_ok=True)
    for size, repo, ladder in (("27b", a.repo_27b, LADDER_27B), ("4b", a.repo_4b, LADDER_4B)):
        if size == "27b" and a.skip_27b:
            continue
        files = list_repo_files(repo)
        want = [f for f in files if f.endswith(".gguf") and ("mmproj" in f.lower() and "f16" in f.lower()
                or any(q.lower() in f.lower() for q in ladder) and "mmproj" not in f.lower())]
        print(f"[{size}] {repo}: downloading {len(want)} files")
        for f in want:
            p = hf_hub_download(repo, f, local_dir=a.out, token=a.token)
            print("  ", p)
    print("Done. If a file name lacks '27b'/'4b' or the quant tag, rename it - the app classifies by name.")


def run(cmd):
    print("+", " ".join(cmd)); subprocess.check_call(cmd)


def convert(a):
    conv = os.path.join(a.llama_cpp, "convert_hf_to_gguf.py")
    quant = a.llama_quantize or shutil.which("llama-quantize")
    if not (os.path.isfile(conv) and quant):
        sys.exit("Need --llama-cpp <checkout> and llama-quantize (--llama-quantize or on PATH).")
    os.makedirs(a.out, exist_ok=True)
    size = a.size
    f16 = os.path.join(a.out, f"medgemma-{size}-it-F16.gguf")
    run([sys.executable, conv, a.hf_dir, "--outfile", f16, "--outtype", "f16"])
    run([sys.executable, conv, a.hf_dir, "--outfile", os.path.join(a.out, f"mmproj-medgemma-{size}-it-F16.gguf"),
         "--outtype", "f16", "--mmproj"])
    for q in (LADDER_27B if size == "27b" else LADDER_4B):
        run([quant, f16, os.path.join(a.out, f"medgemma-{size}-it-{q}.gguf"), q])
    if not a.keep_f16:
        os.remove(f16)  # F16 would be picked as the 'largest' candidate and never fits; keep it out of the folder


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("download"); d.add_argument("--out", required=True); d.add_argument("--token")
    d.add_argument("--repo-27b", default=DEFAULT_REPOS["27b"]); d.add_argument("--repo-4b", default=DEFAULT_REPOS["4b"])
    d.add_argument("--skip-27b", action="store_true"); d.set_defaults(fn=download)
    c = sub.add_parser("convert"); c.add_argument("--hf-dir", required=True, help="local HF safetensors folder")
    c.add_argument("--size", choices=["27b", "4b"], required=True); c.add_argument("--out", required=True)
    c.add_argument("--llama-cpp", required=True); c.add_argument("--llama-quantize"); c.add_argument("--keep-f16", action="store_true")
    c.set_defaults(fn=convert)
    a = ap.parse_args(); a.fn(a)
