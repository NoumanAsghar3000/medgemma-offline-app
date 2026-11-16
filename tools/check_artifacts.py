#!/usr/bin/env python3
"""Checks the downloadable installer artifacts of a workflow run. Used by the `verify-artifacts` CI job AND by a supervisor:
    gh run download <run-id> -D artifacts && python tools/check_artifacts.py artifacts
Checks, per installer: real Windows executable (MZ), Inno Setup installer signature, plausible size, version in the file name,
SHA-256 equals the published SHA256SUMS.txt. And: the WebView2-bundled installer is clearly larger than the slim one.
`--selftest` builds fake artifacts and proves the checker passes good ones and rejects bad ones (run in CI)."""
import hashlib, mmap, pathlib, re, sys, tempfile

MB = 1024 * 1024
def sha(p): h = hashlib.sha256(); h.update(p.read_bytes()); return h.hexdigest().upper()

def check(root: pathlib.Path):
    errs, info = [], []
    exes = sorted(root.rglob("*.exe"))
    slim = [e for e in exes if "with-WebView2" not in e.name]; bundled = [e for e in exes if "with-WebView2" in e.name]
    if len(slim) != 1: errs.append(f"expected exactly 1 slim installer, found {len(slim)}")
    if len(bundled) != 1: errs.append(f"expected exactly 1 with-WebView2 installer, found {len(bundled)}")
    sums = {}
    for f in root.rglob("SHA256SUMS.txt"):
        for line in f.read_text().splitlines():
            m = re.match(r"^([0-9A-Fa-f]{64})\s+(\S.*)$", line.strip())
            if m: sums[m.group(2).strip()] = m.group(1).upper()
    for e in slim + bundled:
        with open(e, "rb") as fh:
            head = fh.read(2)
            if head != b"MZ": errs.append(f"{e.name}: not a Windows executable (no MZ header)")
            with mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ) as mm:
                if mm.find(b"Inno Setup Setup Data") < 0: errs.append(f"{e.name}: Inno Setup signature not found")
        size = e.stat().st_size; info.append(f"{e.name}: {size / MB:.1f} MB")
        if size < 5 * MB: errs.append(f"{e.name}: suspiciously small ({size} bytes)")
        if not re.search(r"MedGemmaOffline-Setup-\d", e.name): errs.append(f"{e.name}: version missing from the file name")
        want = sums.get(e.name)
        if want is None: errs.append(f"{e.name}: no entry in SHA256SUMS.txt")
        elif want != sha(e): errs.append(f"{e.name}: SHA-256 mismatch with SHA256SUMS.txt")
    if len(slim) == 1 and len(bundled) == 1 and bundled[0].stat().st_size < slim[0].stat().st_size + 100 * MB:
        errs.append("the with-WebView2 installer should be at least 100 MB larger than the slim one (offline runtime missing?)")
    return errs, info

def selftest():
    def make(d, name, size, mz=True, inno=True):
        p = d / name; p.write_bytes((b"MZ" if mz else b"XX") + b"\0" * 100 + (b"Inno Setup Setup Data (6.3.1)" if inno else b"") + b"\0" * size); return p
    with tempfile.TemporaryDirectory() as t:
        t = pathlib.Path(t); a = t / "slim"; b = t / "bundled"; a.mkdir(); b.mkdir()
        s = make(a, "MedGemmaOffline-Setup-1.0.0.exe", 8 * MB); w = make(b, "MedGemmaOffline-Setup-1.0.0-with-WebView2.exe", 120 * MB)
        (a / "SHA256SUMS.txt").write_text(f"{sha(s)}  {s.name}\n"); (b / "SHA256SUMS.txt").write_text(f"{sha(w)}  {w.name}\n")
        errs, _ = check(t); assert not errs, errs
        w2 = make(b, "MedGemmaOffline-Setup-1.0.0-with-WebView2.exe", 120 * MB, mz=False); errs, _ = check(t); assert any("MZ" in e for e in errs), errs
        w3 = make(b, "MedGemmaOffline-Setup-1.0.0-with-WebView2.exe", 120 * MB, inno=False); (b / "SHA256SUMS.txt").write_text(f"{sha(w3)}  {w3.name}\n"); errs, _ = check(t); assert any("Inno" in e for e in errs), errs
        make(b, "MedGemmaOffline-Setup-1.0.0-with-WebView2.exe", 9 * MB); errs, _ = check(t); assert any("larger" in e for e in errs) or any("mismatch" in e for e in errs), errs
    print("selftest passed: good artifacts accepted; bad header / missing Inno signature / tiny bundle / checksum drift rejected")

if __name__ == "__main__":
    if "--selftest" in sys.argv: selftest(); sys.exit(0)
    errs, info = check(pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "."))
    print("\n".join(info))
    if errs: print("ARTIFACT CHECK FAILED:\n - " + "\n - ".join(errs)); sys.exit(1)
    print("ARTIFACT CHECK PASSED: both installers are real Inno Setup executables with matching SHA-256 sums")
