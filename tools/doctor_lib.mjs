// MedGemma Offline DOCTOR: finds out exactly what is wrong instead of failing silently. Four checks:
//   1. Model header health      - `mg-probe doctor` (HEALTH block): per-family readiness, how every file was identified, incomplete/corrupt files
//   2. Browser args accepted    - did WebView2 receive --remote-debugging-port (and the fake-camera flags)?
//   3. Debug port listening     - what the port answers (refused / timeout / another program / no page / ok)
//   4. Camera readiness         - devices, permission, session diagnostics, and (optionally) a real open/close cycle through the app's dialog
// Pure functions + injectable dependencies, so everything is unit-tested without Windows.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, openSync, readFileSync } from "node:fs";
import path from "node:path";
import { createCdp, waitForEndpoint } from "./cdp.mjs";
import { analyzeBrowserArgs, buildBrowserArgs, finalVerdict, parseDevToolsActivePort, pickFreePort } from "./e2e_support.mjs";

export const PASS = "PASS", WARN = "WARN", FAIL = "FAIL", SKIP = "SKIP";
const check = (name, status, detail, data) => ({ name, status, detail, ...(data ? { data } : {}) });

/* ------------------------------------------------------------------ 1. model header health ------------------------------------------------------------------ */

/** Parses the HEALTH block printed by `mg-probe doctor`. Returns null when the block is missing (an old mg-probe). */
export function parseHealth(text) {
  const m = /--- HEALTH \(machine readable\) ---\r?\n([\s\S]*?)--- END HEALTH ---/.exec(String(text ?? ""));
  if (!m) return null;
  const kv = {};
  for (const line of m[1].split(/\r?\n/)) { const i = line.indexOf("="); if (i > 0) kv[line.slice(0, i)] = line.slice(i + 1); }
  const count = Number(kv["HEALTH.files"] ?? 0);
  const files = [];
  for (let n = 0; n < count; n++) {
    const g = (k) => kv[`HEALTH.file.${n}.${k}`];
    files.push({ name: g("name"), kind: g("kind"), family: g("family"), quant: g("quant"), quantSource: g("quant_source"), size: Number(g("size")),
      expectedMinSize: g("expected_min_size") === "-" ? null : Number(g("expected_min_size")), header: g("header"), state: g("state"), detail: g("detail") ?? "" });
  }
  return { verdict: kv["HEALTH.verdict"], family27: kv["HEALTH.family.27B"], family4: kv["HEALTH.family.4B"], files, raw: kv };
}

export function checkModelHealth(h) {
  const name = "Model header health";
  if (!h) return check(name, FAIL, "mg-probe printed no HEALTH block: it is an old build. Use the mg-probe.exe from the current installer (tools\\mg-probe.exe).");
  const fam = `27B: ${h.family27}, 4B: ${h.family4}`;
  const bad = h.files.filter((f) => f.state !== "ok").map((f) => `${f.name}: ${f.detail || f.state}`);
  const how = h.files.filter((f) => f.kind !== "projector").map((f) => `${f.name} = ${f.family} ${f.quant} (from ${f.quantSource === "name" ? "the file NAME" : f.quantSource === "unknown" ? "nothing: unknown precision" : "the header"})`);
  if (h.verdict === "OK") return check(name, PASS, `${fam}; ${h.files.length} file(s) identified by content. ${how.join("; ")}`, h);
  if (h.verdict === "DEGRADED") return check(name, WARN, `${fam}. ${bad.join(" | ") || "a family is incomplete (see the family states)"}`, h);
  if (h.verdict === "EMPTY") return check(name, FAIL, "the folder contains no .gguf files at all (wrong folder, or the drive is not connected)", h);
  return check(name, FAIL, `no usable model: ${fam}. ${bad.join(" | ")}`, h);
}

export function runProbeDoctor(probeExe, modelsDir) {
  if (!probeExe || !existsSync(probeExe)) return check("Model header health", FAIL, `mg-probe was not found${probeExe ? " at " + probeExe : ""} (pass --probe <path to mg-probe.exe>)`);
  try { return checkModelHealth(parseHealth(execFileSync(probeExe, ["doctor", "--dir", modelsDir], { encoding: "utf8", timeout: 120000 }))); }
  catch (e) { return check("Model header health", FAIL, `mg-probe doctor failed to run: ${String(e.message).slice(0, 300)}`); }
}

/* ------------------------------------------------------------------ 4. camera readiness (in the page) ------------------------------------------------------------------ */

/** Evaluated INSIDE the app's page over the debug port. Reads devices, permission and the app's own camera diagnostics. */
export const CAMERA_PROBE_JS = `(async () => {
  const out = { hasMediaDevices: !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia), videoInputs: null, permission: null, diag: null, error: null };
  try { if (out.hasMediaDevices) out.videoInputs = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'videoinput').length; } catch (e) { out.error = String((e && e.message) || e); }
  try { if (navigator.permissions) out.permission = (await navigator.permissions.query({ name: 'camera' })).state; } catch (e) { out.permission = 'unsupported'; }
  try { out.diag = typeof window.__mgDiagnostics === 'function' ? window.__mgDiagnostics() : null; } catch (e) { out.error = (out.error || '') + ' diagnostics: ' + String((e && e.message) || e); }
  return out;
})()`;

export function interpretCamera(raw) {
  const name = "Camera readiness";
  if (!raw || typeof raw !== "object") return check(name, FAIL, "the page did not answer the camera probe");
  if (!raw.hasMediaDevices) return check(name, FAIL, "navigator.mediaDevices.getUserMedia does not exist in the page (insecure context, or camera support disabled in WebView2)", raw);
  if (raw.videoInputs === 0) return check(name, FAIL, "the page sees NO video input device: no camera is connected, or Windows Settings > Privacy & security > Camera blocks desktop apps", raw);
  if (raw.permission === "denied") return check(name, FAIL, "camera permission is DENIED for this app: allow it in Windows Settings > Privacy & security > Camera", raw);
  const cam = raw.diag?.camera;
  if (!raw.diag) return check(name, WARN, `${raw.videoInputs ?? "?"} video input(s), permission ${raw.permission ?? "unknown"}; this build has no __mgDiagnostics hook, so the session state cannot be read`, raw);
  if (cam && cam.leaked !== 0) return check(name, FAIL, `CAMERA LEAK: ${cam.verdict}`, raw);
  if (cam && cam.verdict !== "OK" && cam.visible) return check(name, WARN, `camera dialog is open and reports: ${cam.verdict}`, raw);
  return check(name, PASS, `${raw.videoInputs ?? "?"} video input(s), permission ${raw.permission ?? "unknown"}, session ${cam?.visible ? "open" : "idle"} and healthy (${cam?.streamsAcquired ?? 0} opened / ${cam?.streamsStopped ?? 0} stopped)`, raw);
}

export function interpretBackend(raw) {
  const name = "Backend status";
  const b = raw?.diag?.backend ?? raw?.backend;
  if (!b) return check(name, SKIP, "no backend status in the page diagnostics");
  if (b.state === "ready" || b.state === "idle") return check(name, PASS, `${b.state}: ${b.family ?? ""} ${b.quant ?? ""} on ${String(b.compute ?? "").toUpperCase()}${b.multimodal ? " with vision" : " text-only"}`.replace(/\s+/g, " "), b);
  if (b.state === "need_model_folder") return check(name, WARN, "the app is waiting for a model folder (the re-link dialog is showing)", b);
  if (b.state === "loading") return check(name, WARN, "the model is still loading", b);
  return check(name, FAIL, `backend state '${b.state}'${b.note ? ": " + b.note : ""}`, b);
}

/** Opens the camera through the app's real dialog, checks Capture is only enabled with frames, closes it, and checks the camera was released. */
export async function exerciseCameraCycle(cdp, { frameWaitMs = 25000 } = {}) {
  const name = "Camera open/close cycle (real dialog)";
  const diag = () => cdp.ev(`window.__mgDiagnostics ? window.__mgDiagnostics().camera : null`);
  try {
    await cdp.click("#plus"); await cdp.click("#m-cam");
    await cdp.waitFor(`!document.getElementById('cam').hidden`, 5000, "camera dialog");
    try {
      await cdp.waitFor(`!document.getElementById('cam-shot').disabled`, frameWaitMs, "camera frames + Capture enabled", `document.getElementById('cam-err').innerText.trim()`);
    } catch (e) {
      const d = await diag().catch(() => null);
      await cdp.click("#cam-cancel").catch(() => {});
      return check(name, FAIL, `${e.message}${d ? ` | the app's own verdict: ${d.verdict}` : ""}`, { diag: d });
    }
    const open = await diag();
    await cdp.click("#cam-cancel");
    const after = await diag();
    const problems = [];
    if (open && !open.live) problems.push("Capture was enabled but the session does not report live frames");
    if (after) {
      if (after.hasStream || (after.tracks || []).length) problems.push("the camera is STILL HELD after the dialog was closed (camera light stays on)");
      if (after.visible) problems.push("the dialog is still marked visible after closing");
      if (after.leaked !== 0) problems.push(`${after.leaked} camera stream(s) leaked`);
    }
    if (problems.length) return check(name, FAIL, problems.join("; "), { open, after });
    return check(name, PASS, `Capture enabled only after frames arrived; camera released on close (${open?.streamsAcquired ?? "?"} opened / ${after?.streamsStopped ?? "?"} stopped)`, { open, after });
  } catch (e) { return check(name, FAIL, e.message); }
}

/* ------------------------------------------------------------------ 2 + 3. browser args and debug port ------------------------------------------------------------------ */

const PS_LINES = `Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'msedgewebview2*' -or $_.Name -like 'medgemma*' } | ForEach-Object { ($_.ProcessId.ToString() + ' ' + $_.CommandLine) }`;
export function defaultReadCommandLines() {
  if (process.env.E2E_CMDLINES_FILE) return readFileSync(process.env.E2E_CMDLINES_FILE, "utf8");   // test seam, same as the e2e script
  if (process.platform !== "win32") return "";
  try { return execFileSync("powershell", ["-NoProfile", "-Command", PS_LINES], { encoding: "utf8", timeout: 30000 }); } catch (e) { return String(e.stdout || ""); }
}

export function launchApp({ appExe, port, userDataDir, logFile }) {
  mkdirSync(userDataDir, { recursive: true });
  const args = buildBrowserArgs(port);
  const env = { ...process.env, WEBVIEW2_USER_DATA_FOLDER: userDataDir, MG_TEST_BROWSER_ARGS: args, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: args };
  const fd = openSync(logFile, "w");
  const child = spawn(appExe, [], { env, stdio: ["ignore", fd, fd] });
  const state = { exited: false, code: null };
  child.on("exit", (c) => { state.exited = true; state.code = c; });
  child.on("error", (e) => { state.exited = true; state.code = `spawn error: ${e.message}`; });
  return { child, state, kill() { try { child.kill(); } catch {} if (process.platform === "win32" && child.pid) { try { execFileSync("taskkill", ["/F", "/T", "/PID", String(child.pid)], { stdio: "ignore" }); } catch {} } } };
}

/* ------------------------------------------------------------------ orchestration ------------------------------------------------------------------ */

/**
 * opts: { probeExe, modelsDir, appExe | attachPort, workDir, waitSeconds, exerciseCamera, readCommandLines, now }
 * Returns { checks, headline, ok, startup } - startup is the plain verdict when the debug port never came up.
 */
export async function runDoctor(opts) {
  const { probeExe, modelsDir, appExe, attachPort, workDir = ".", waitSeconds = 60, exerciseCamera = false, readCommandLines = defaultReadCommandLines } = opts;
  const checks = [];
  checks.push(modelsDir ? runProbeDoctor(probeExe, modelsDir) : check("Model header health", SKIP, "no --models folder given"));

  let startup = null;
  if (!appExe && !attachPort) {
    for (const n of ["Browser args accepted", "Debug port listening", "Camera readiness", "Backend status"]) checks.push(check(n, SKIP, "no --app or --attach-port given"));
  } else {
    let port = attachPort; let launched = null; const udf = path.join(workDir, "doctor-webview2");
    if (appExe) { port = await pickFreePort(9333); launched = launchApp({ appExe, port, userDataDir: udf, logFile: path.join(workDir, "doctor-app.log") }); }
    try {
      const followPortFile = () => {
        const f = path.join(udf, "EBWebView", "DevToolsActivePort");
        if (launched && existsSync(f)) { const p = parseDevToolsActivePort(readFileSync(f, "utf8")); if (p) port = p; }
      };
      const probe = await waitForEndpoint(() => port, waitSeconds, { isAlive: () => !launched || !launched.state.exited, onTick: followPortFile });
      const analysis = analyzeBrowserArgs(readCommandLines(), port);
      const verdict = finalVerdict({ probe, analysis, appExited: !!launched?.state.exited, appExitCode: launched?.state.code ?? null, port });
      if (probe.state !== "ok") startup = verdict;

      if (probe.state === "ok" || analysis.hasPort) {
        checks.push(check("Browser args accepted", PASS, `--remote-debugging-port=${port} reached WebView2${analysis.hasFakeDevice && analysis.hasFakeUi ? " with the fake-camera flags" : analysis.webviewProcesses ? " (fake-camera flags NOT seen)" : ""}`, analysis));
      } else {
        checks.push(check("Browser args accepted", FAIL, `${verdict.headline}: ${verdict.explanation}`, analysis));
      }
      checks.push(probe.state === "ok"
        ? check("Debug port listening", PASS, `a DevTools page answers on port ${port} (probe: ${JSON.stringify(probe.tally)})`, probe)
        : check("Debug port listening", FAIL, `${verdict.headline}: ${verdict.explanation} Next: ${verdict.nextSteps.join(" ")}`, probe));

      if (probe.state !== "ok") {
        checks.push(check("Camera readiness", SKIP, "the debug port is not available"), check("Backend status", SKIP, "the debug port is not available"));
      } else {
        const cdp = createCdp({ timeoutMs: 30000 });
        try {
          await cdp.connect(probe.wsUrl);
          const raw = await cdp.ev(CAMERA_PROBE_JS).catch((e) => ({ error: e.message }));
          checks.push(interpretCamera(raw), interpretBackend(raw));
          if (exerciseCamera) checks.push(await exerciseCameraCycle(cdp));
        } catch (e) {
          checks.push(check("Camera readiness", FAIL, `could not talk to the page: ${e.message}`));
        } finally { cdp.close(); }
      }
    } finally { launched?.kill(); }
  }

  const fails = checks.filter((c) => c.status === FAIL); const warns = checks.filter((c) => c.status === WARN);
  const headline = fails.length ? `DOCTOR: ${fails.length} PROBLEM(S) - ${fails.map((c) => c.name).join("; ")}` : warns.length ? "DOCTOR: OK WITH WARNINGS" : "DOCTOR: ALL CHECKS PASSED";
  return { checks, headline, ok: fails.length === 0, startup };
}

export function formatReport(r) {
  const icon = { PASS: "[PASS]", WARN: "[WARN]", FAIL: "[FAIL]", SKIP: "[skip]" };
  const lines = ["MedGemma Offline doctor", ""];
  for (const c of r.checks) lines.push(`${icon[c.status]} ${c.name}`, `       ${c.detail}`);
  if (r.startup) { lines.push("", `Startup verdict: ${r.startup.headline}`, `  ${r.startup.explanation}`); for (const s of r.startup.nextSteps) lines.push(`  -> ${s}`); }
  lines.push("", `=== ${r.headline} ===`);
  return lines.join("\n");
}
