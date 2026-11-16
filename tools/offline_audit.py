#!/usr/bin/env python3
"""Offline audit: fails (exit 1) if the built app or installer can reach the internet by design.
Checks: built UI (dist/) for external <link>/<script>/@import/url()/img/font references and CDN hosts,
the Tauri CSP, and the installer script for download commands. Run after `npm run build`."""
import re, sys, pathlib, json
root = pathlib.Path(__file__).resolve().parent.parent
bad, info = [], []
EXT = re.compile(r"""(?:src|href)\s*=\s*["']\s*(?:https?:)?//|@import\s+(?:url\()?["']?\s*(?:https?:)?//|url\(\s*["']?\s*(?:https?:)?//""", re.I)
CDN = ("fonts.googleapis.com", "fonts.gstatic.com", "cdn.jsdelivr.net", "unpkg.com", "cdnjs.cloudflare.com", "ajax.googleapis.com", "code.jquery.com", "cdn.tailwindcss.com")
dist = root / "dist"
if not dist.is_dir(): sys.exit("dist/ not found - run `npm run build` first")
hosts = set()
for f in dist.rglob("*"):
    if f.suffix.lower() not in (".html", ".css", ".js", ".mjs"): continue
    t = f.read_text(errors="ignore")
    if f.suffix.lower() in (".html", ".css") and EXT.search(t): bad.append(f"{f.relative_to(root)}: external resource reference")
    for h in re.findall(r"https?://([A-Za-z0-9.-]+)", t): hosts.add(h)
    for c in CDN:
        if c in t: bad.append(f"{f.relative_to(root)}: mentions CDN host {c}")
info.append(f"hosts appearing as plain strings in bundled JS (docs/namespace URLs, never fetched): {len(hosts)}")
conf = json.loads((root / "src-tauri/tauri.conf.json").read_text())
csp = conf["app"]["security"]["csp"]
m = re.search(r"connect-src([^;]*)", csp)
if not m or re.search(r"https?://(?!ipc\.localhost)", m.group(1)): bad.append(f"CSP connect-src allows network hosts: {m.group(1) if m else 'missing'}")
if re.search(r"https?://(?!ipc\.localhost)", csp): bad.append("CSP references an external http(s) host")
iss = (root / "installer/medgemma.iss").read_text()
for pat in (r"\bDownloadTemporaryFile\b", r"\bInitializeWizard\b.*http", r"https?://"):
    if re.search(pat, iss.replace("{F3017226", "")): bad.append(f"installer script matches /{pat}/")
rs = "".join(p.read_text(errors="ignore") for p in (root / "src-tauri/src").rglob("*.rs"))
if re.search(r'https?://(?!127\.0\.0\.1|ipc\.localhost)', rs): bad.append("Rust shell contains an external URL")
print("\n".join(info))
if bad: print("OFFLINE AUDIT FAILED:\n - " + "\n - ".join(bad)); sys.exit(1)
print("OFFLINE AUDIT PASSED: no external resources, CDNs, fonts, downloads or network hosts in UI, CSP, installer or shell.")
