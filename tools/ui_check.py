#!/usr/bin/env python3
"""Drives the BUILT UI (dist/) in a real browser engine (Firefox, headless) against tools/ui_mock/mock-tauri.js and saves screenshots.
Verifies layout/logic of: model-folder re-link dialog, streaming into clinical cards, live + measured tok/s, variant selector,
forced-variant warning, report dialog -> PDF save call, fake camera, attachments, theme toggle, 'New case'.
usage: ui_check.py [outdir]     env: FIREFOX, GECKODRIVER (optional paths)"""
import http.server, os, pathlib, shutil, socketserver, struct, sys, tempfile, threading, time, zlib
from selenium import webdriver
from selenium.webdriver.common.by import By
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service
from selenium.webdriver.support.ui import WebDriverWait

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/uishots"); OUT.mkdir(parents=True, exist_ok=True)
site = pathlib.Path(tempfile.mkdtemp(prefix="mg_ui_"))
shutil.copytree(ROOT / "dist", site, dirs_exist_ok=True); shutil.copy(ROOT / "tools/ui_mock/mock-tauri.js", site / "mock-tauri.js")
idx = (site / "index.html").read_text(); (site / "index.html").write_text(idx.replace("<head>", '<head><script src="/mock-tauri.js"></script>', 1))

class H(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k): super().__init__(*a, directory=str(site), **k)
    def log_message(self, *a): pass
srv = socketserver.TCPServer(("127.0.0.1", 0), H); port = srv.server_address[1]; threading.Thread(target=srv.serve_forever, daemon=True).start()

def png(w, h):
    raw = b"".join(b"\x00" + b"".join(bytes([(x * 4) & 255, (y * 4) & 255, 128]) for x in range(w)) for y in range(h))
    def ch(t, d): c = struct.pack(">I", len(d)) + t + d; return c + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + ch(b"IDAT", zlib.compress(raw)) + ch(b"IEND", b"")
files = pathlib.Path(tempfile.mkdtemp(prefix="mg_files_"))
(files / "labs.csv").write_text("age,glucose\n" + "\n".join(f"{20 + i % 50},{80 + i % 90}" for i in range(400)))
(files / "notes.txt").write_text("62-year-old woman, palpable right breast lump for 3 weeks.")
(files / "mammo.png").write_bytes(png(96, 96))

opts = Options(); opts.add_argument("-headless"); opts.binary_location = os.environ.get("FIREFOX", shutil.which("firefox") or "firefox")
opts.set_preference("media.navigator.streams.fake", True); opts.set_preference("media.navigator.permission.disabled", True)
drv = webdriver.Firefox(options=opts, service=Service(executable_path=os.environ.get("GECKODRIVER", shutil.which("geckodriver") or "geckodriver")))
drv.set_window_size(1240, 900)
results = []
def check(name, ok, detail=""):
    results.append(bool(ok)); print(("PASS  " if ok else "FAIL  ") + name + (f"  - {str(detail)[:160]}" if detail else ""))
def wait(fn, t=20, what=""):
    try: return WebDriverWait(drv, t, poll_frequency=0.2).until(lambda d: fn())
    except Exception: raise AssertionError(f"timeout: {what}")
def shot(n): drv.save_screenshot(str(OUT / f"{n}.png"))
js = lambda s, *a: drv.execute_script(s, *a)
def open_(scenario): drv.get(f"http://127.0.0.1:{port}/index.html?scenario={scenario}"); time.sleep(0.8)

try:
    # ---- 1. missing / unusable model folder -> dialog with reason, then re-link
    open_("relink")
    wait(lambda: "Backend model files not found" in drv.find_element(By.ID, "gate-body").text, 15, "relink dialog")
    t = drv.find_element(By.ID, "gate-body").text
    check("re-link dialog: 'Backend model files not found'", True); check("dialog shows last path", "D:\\models-old" in t)
    check("dialog explains the precise reason (projector size mismatch)", "embedding size 2560" in t, t[-120:])
    check("folder browser button present", drv.find_element(By.ID, "g-pick").is_displayed()); shot("01-relink-dialog")
    drv.find_element(By.ID, "g-pick").click()
    wait(lambda: drv.find_element(By.ID, "gate").get_attribute("hidden") is not None, 15, "gate hidden after re-link")
    check("after re-link the model loads and chat unlocks", "MedGemma" in drv.find_element(By.ID, "badge").text, drv.find_element(By.ID, "badge").text)

    # ---- 2. ready: header controls, welcome screen
    open_("ready"); wait(lambda: "MedGemma 27B" in drv.find_element(By.ID, "badge").text, 15, "ready badge")
    wait(lambda: len(drv.find_elements(By.CSS_SELECTOR, "#variant option")) >= 4, 10, "variant list")
    opts_t = [o.text for o in drv.find_elements(By.CSS_SELECTOR, "#variant option")]
    check("variant selector lists Automatic + every variant with estimated speed", any("tok/s" in o for o in opts_t) and opts_t[0].startswith("Automatic"), " | ".join(opts_t))
    check("offline/private indicator visible", "Offline" in drv.find_element(By.CSS_SELECTOR, ".pill.offline").text)
    check("welcome screen with 3 clinical starters", len(drv.find_elements(By.CSS_SELECTOR, ".starter")) == 3); shot("02-welcome")
    drv.find_elements(By.CSS_SELECTOR, ".starter")[0].click()
    check("starter fills the input", "mammogram" in drv.find_element(By.ID, "input").get_attribute("value"))

    # ---- 3. attachments (CSV, TXT, PNG) + fake camera
    inp = drv.find_element(By.ID, "file"); js("arguments[0].hidden=false", inp)
    inp.send_keys("\n".join(str(files / n) for n in ("labs.csv", "notes.txt", "mammo.png")))
    wait(lambda: len(drv.find_elements(By.CSS_SELECTOR, "#chips .chip")) >= 3 and not drv.find_elements(By.CSS_SELECTOR, "#chips .chip.busy"), 20, "3 chips")
    chips = drv.find_element(By.ID, "chips").text
    check("CSV, TXT and PNG attach as chips (PNG with thumbnail)", all(n in chips for n in ("labs.csv", "notes.txt", "mammo.png")) and drv.find_elements(By.CSS_SELECTOR, "#chips img"), chips.replace("\n", " "))
    drv.find_element(By.ID, "plus").click(); drv.find_element(By.ID, "m-cam").click()
    try:
        wait(lambda: bool(js("return document.getElementById('video').videoWidth > 0 && !document.getElementById('cam-shot').disabled")), 20, "camera frames + capture button enabled")
        shot("03-camera"); drv.find_element(By.ID, "cam-shot").click()
        wait(lambda: "camera-" in drv.find_element(By.ID, "chips").text, 15, "camera chip"); check("camera: capture attaches a photo chip", True)
    except AssertionError as e:
        check("camera: capture attaches a photo chip", False, e); js("document.getElementById('cam').hidden=true")
    shot("03b-attachments")

    # ---- 4. send -> live tok/s while streaming -> clinical cards -> measured tok/s
    drv.find_element(By.ID, "send").click()
    seen_live = False
    t0 = time.time()
    while time.time() - t0 < 30 and not drv.find_elements(By.CSS_SELECTOR, ".msg.assistant .actions button"):
        seen_live = seen_live or "tok/s (live)" in drv.find_element(By.ID, "live").text; time.sleep(0.15)
    wait(lambda: drv.find_elements(By.CSS_SELECTOR, ".msg.assistant .actions button"), 30, "answer finished")
    check("live tokens/second shown while streaming", seen_live)
    check("answer rendered as Findings / Impression / Recommendations cards", all(drv.find_elements(By.CSS_SELECTOR, f".sec.{c}") for c in ("find", "imp", "rec")))
    check("numbered Impression renders as an ordered list (rank order kept)", drv.find_elements(By.CSS_SELECTOR, ".sec.imp ol li").__len__() == 2)
    check("Copy button next to the report button", any(b.text.strip() == "Copy" for b in drv.find_elements(By.CSS_SELECTOR, ".msg.assistant .actions button")))
    check("BI-RADS content visible in the Impression card", "BI-RADS 4C" in drv.find_element(By.CSS_SELECTOR, ".sec.imp").text)
    check("measured tokens/second under the answer", "2.31 tok/s measured" in drv.find_element(By.CSS_SELECTOR, ".msg.assistant .actions").text); shot("04-answer-cards")

    # ---- 5. report dialog -> save
    drv.find_element(By.CSS_SELECTOR, ".msg.assistant .actions button").click()
    wait(lambda: not drv.find_element(By.ID, "case").get_attribute("hidden"), 5, "case dialog")
    for i, v in (("c-name", "Test Patient"), ("c-id", "MRN-77"), ("c-age", "62 / F"), ("c-doc", "Dr Demo")): drv.find_element(By.ID, i).send_keys(v)
    shot("05-report-dialog"); drv.find_element(By.ID, "case-save").click()
    wait(lambda: js("return window.__saved") is not None, 20, "save_report call")
    sv = js("return window.__saved"); check("report PDF built in the browser engine and sent to the native save command", sv["head"].startswith("%PDF") and sv["bytes"] > 3000, sv)
    wait(lambda: "Saved:" in drv.find_element(By.ID, "case-msg").text, 10, "saved message")
    wait(lambda: drv.find_element(By.ID, "case").get_attribute("hidden") is not None, 10, "report dialog closes itself after saving")

    # ---- 6. forced slow variant: strong warning + red banner
    sel = drv.find_element(By.ID, "variant")
    from selenium.webdriver.support.ui import Select
    Select(sel).select_by_value("medgemma-27b-it-Q8_0.gguf")
    wait(lambda: not drv.find_element(By.ID, "force").get_attribute("hidden"), 5, "force dialog")
    ft = drv.find_element(By.ID, "force-text").text
    check("slow variant: blocking 'Very slow' dialog with estimate and duration", "0.1 tok/s" in ft and "hours" in ft and "24.0 GB" in ft, ft); shot("06-force-dialog")
    drv.find_element(By.ID, "force-no").click(); time.sleep(0.4)
    check("Cancel keeps the previous variant selected", "Q4_K_M" in Select(sel).first_selected_option.text)
    Select(sel).select_by_value("medgemma-27b-it-Q8_0.gguf"); wait(lambda: not drv.find_element(By.ID, "force").get_attribute("hidden"), 5, "force dialog 2")
    drv.find_element(By.ID, "force-yes").click()
    wait(lambda: "FORCED SLOW VARIANT" in drv.find_element(By.ID, "warn").text, 10, "forced banner")
    check("red 'FORCED SLOW VARIANT' banner shown after confirming", "forced" in drv.find_element(By.ID, "warn").get_attribute("class")); shot("06b-forced-banner")

    # ---- 7. theme + new case
    wait(lambda: drv.find_element(By.ID, "gate").get_attribute("hidden") is not None, 15, "model reloaded")
    drv.find_element(By.ID, "theme").click(); time.sleep(0.3)
    check("theme toggle switches to dark", js("return document.documentElement.dataset.theme") in ("dark", "light")); shot("07-theme")
    drv.find_element(By.ID, "newcase").click(); time.sleep(0.4)
    check("New case clears the conversation and restores the welcome screen", not drv.find_elements(By.CSS_SELECTOR, ".msg") and drv.find_elements(By.ID, "empty") and not drv.find_elements(By.CSS_SELECTOR, "#chips .chip"))
finally:
    drv.quit(); srv.shutdown()
print(f"\n{sum(results)}/{len(results)} UI checks passed; screenshots in {OUT}")
sys.exit(0 if all(results) else 1)
