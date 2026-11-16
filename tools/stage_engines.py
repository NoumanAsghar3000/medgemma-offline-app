#!/usr/bin/env python3
"""Unpack a llama.cpp Windows release zip and keep ONLY what llama-server needs (smaller installer).
Since ~b12xxx `llama-server.exe` is a thin launcher that loads `llama-server-impl.dll`; every other tool
(`llama-bench`, `llama-cli`, ...) ships its own exe + `*-impl.dll`, which we drop. Licences are kept.
usage: stage_engines.py <zip> <outdir>"""
import sys, zipfile, pathlib, re, shutil
zin, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
DROP_EXE = re.compile(r"^(?!llama-server\.exe$).*\.exe$", re.I)                       # every exe except llama-server.exe
DROP_DLL = re.compile(r"^llama-(?!server-impl\.dll$|common\.dll$).*-impl\.dll$|^llama-(bench|cli|batched-bench|completion|perplexity|quantize|fit-params)\.dll$", re.I)
if out.exists(): shutil.rmtree(out)
out.mkdir(parents=True)
kept = dropped = 0
with zipfile.ZipFile(zin) as z:
    for i in z.infolist():
        if i.is_dir(): continue
        name = pathlib.PurePosixPath(i.filename).name
        if DROP_EXE.match(name) or DROP_DLL.match(name): dropped += i.file_size; continue
        (out / name).write_bytes(z.read(i)); kept += i.file_size
need = ["llama-server.exe", "llama-server-impl.dll", "llama.dll", "ggml.dll", "ggml-base.dll"]
missing = [n for n in need if not (out / n).is_file()]
if missing: sys.exit(f"missing after staging: {missing}")
print(f"{zin.name}: kept {kept/1e6:.1f} MB, dropped {dropped/1e6:.1f} MB -> {out}")
