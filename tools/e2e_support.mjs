// Pure helpers of the real-WebView2 end-to-end test (tools/e2e_webview.mjs), separated so they can be unit-tested anywhere.
import net from "node:net";

/** wry's own default WebView2 argument: passing arguments through the API REPLACES it, so it is repeated. */
export const DEFAULT_FEATURES = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

export function buildBrowserArgs(port) {
  return `${DEFAULT_FEATURES} --remote-debugging-port=${port} --remote-allow-origins=* --use-fake-device-for-media-stream --use-fake-ui-for-media-stream`;
}

/** First free TCP port >= start on 127.0.0.1 (a leftover process or another job may already own 9333). */
export async function pickFreePort(start = 9333, tries = 25) {
  for (let p = start; p < start + tries; p++) {
    const free = await new Promise((resolve) => {
      const s = net.createServer();
      s.once("error", () => resolve(false));
      s.listen(p, "127.0.0.1", () => s.close(() => resolve(true)));
    });
    if (free) return p;
  }
  throw new Error(`no free TCP port in ${start}..${start + tries - 1}`);
}

/** Chromium writes `<port>\n<browser path>` to <user-data>/EBWebView/DevToolsActivePort once remote debugging is listening. */
export function parseDevToolsActivePort(text) {
  const n = Number(String(text ?? "").split(/\r?\n/)[0].trim());
  return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
}

/**
 * Decides, from the command lines of the running processes ("<pid> <command line>" per line), WHY the debug endpoint is missing.
 * code: NO_WEBVIEW2 | ARGS_NOT_ACCEPTED | FLAG_PRESENT_NO_LISTENER
 */
export function analyzeBrowserArgs(commandLines, port) {
  const lines = String(commandLines ?? "").split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  const web = lines.filter((l) => /msedgewebview2/i.test(l));
  const portFlag = new RegExp(`--remote-debugging-port=${String(port).replace(/[^0-9]/g, "")}(?![0-9])`);   // the whole number: 933 must not match 9333
  const hasPort = web.some((l) => portFlag.test(l));
  const hasFakeDevice = web.some((l) => l.includes("--use-fake-device-for-media-stream"));
  const hasFakeUi = web.some((l) => l.includes("--use-fake-ui-for-media-stream"));
  let code, message;
  if (web.length === 0) {
    code = "NO_WEBVIEW2";
    message = "WebView2 never started: there is no msedgewebview2.exe process. Check the WebView2 runtime, a lock on the user-data folder, and app.log.";
  } else if (!hasPort) {
    code = "ARGS_NOT_ACCEPTED";
    message = `WebView2 is running but --remote-debugging-port=${port} is in NO msedgewebview2.exe command line: the browser arguments (MG_TEST_BROWSER_ARGS -> additional_browser_args in src-tauri/src/main.rs) were NOT applied. ` +
      "Likely causes: the installed app does not contain that hook; another WebView2 instance already owns this user-data folder with different arguments; or the runtime ignores the switch.";
  } else {
    code = "FLAG_PRESENT_NO_LISTENER";
    message = `--remote-debugging-port=${port} reached the browser process but nothing listens on it: the port may be blocked, or remote debugging is disabled by a Windows policy (RemoteDebuggingAllowed).`;
  }
  return { code, message, webviewProcesses: web.length, hasPort, hasFakeDevice, hasFakeUi };
}

/**
 * The single plain-language verdict of a startup that did not reach a DevTools page. Combines what the port answered (probe, from cdp.mjs
 * probeEndpoint/waitForEndpoint), what the process command lines show (analyzeBrowserArgs) and whether the app is still alive.
 * headline is what a human reads first: DEBUG PORT NOT DETECTED | WEBVIEW2 ARGS NOT APPLIED | WEBVIEW2 DID NOT START | APP EXITED ... | ...
 */
export function finalVerdict({ probe, analysis, appExited = false, appExitCode = null, port }) {
  const state = probe?.state;
  if (state === "ok") return { code: "OK", headline: "DEBUG PORT OK", explanation: `a DevTools page target answered on port ${port}`, nextSteps: [] };
  if (appExited) {
    return { code: "APP_EXITED", headline: "APP EXITED BEFORE THE DEBUG PORT OPENED", explanation: `the app process ended (exit code ${appExitCode}) before WebView2 exposed a debug port`,
      nextSteps: ["Read app.log (the app's own output).", "Start the installed exe by hand and see whether the window opens.", "Look for a crash entry in Windows Event Viewer > Windows Logs > Application."] };
  }
  if (state === "http-error" || state === "not-json") {
    return { code: "PORT_TAKEN", headline: "DEBUG PORT BELONGS TO ANOTHER PROGRAM", explanation: `port ${port} answers, but not as a DevTools endpoint: ${probe.detail}`,
      nextSteps: [`Free port ${port} or let the test choose another one (it normally does).`] };
  }
  if (state === "no-page-target") {
    return { code: "NO_PAGE", headline: "DEBUG PORT OPEN, BUT NO PAGE", explanation: probe.detail, nextSteps: ["The WebView2 window was not created: read app.log and check that create_main_window() ran (tauri.conf.json has \"create\": false)."] };
  }
  const flagWarning = analysis && analysis.hasPort && !(analysis.hasFakeDevice && analysis.hasFakeUi) ? " The fake-camera flags are missing from the browser process too." : "";
  if (analysis?.code === "NO_WEBVIEW2") {
    return { code: "NO_WEBVIEW2", headline: "WEBVIEW2 DID NOT START", explanation: analysis.message,
      nextSteps: ["Check that the WebView2 runtime is installed (Evergreen standalone).", "Check app.log.", "Make sure no other instance holds the user-data folder."] };
  }
  if (analysis?.code === "ARGS_NOT_ACCEPTED") {
    return { code: "ARGS_NOT_ACCEPTED", headline: "WEBVIEW2 ARGS NOT APPLIED", explanation: analysis.message,
      nextSteps: ["Confirm the installed exe is the build that contains create_main_window() with MG_TEST_BROWSER_ARGS (src-tauri/src/main.rs).",
        "Confirm tauri.conf.json has \"create\": false for the main window.", "Look at the command lines above: they show the arguments WebView2 really received.",
        "A different user-data folder is used for each attempt; if both attempts fail the arguments are being dropped by the app or the runtime."] };
  }
  return { code: analysis?.code === "FLAG_PRESENT_NO_LISTENER" ? "FLAG_PRESENT_NO_LISTENER" : "PORT_CLOSED", headline: "DEBUG PORT NOT DETECTED",
    explanation: `${probe?.detail ?? "nothing answered"} (probe results: ${JSON.stringify(probe?.tally ?? {})}).${analysis ? " " + analysis.message : " (the process command lines could not be read here)"}${flagWarning}`,
    nextSteps: [`Check that port ${port} is not blocked by a firewall or a Windows policy (RemoteDebuggingAllowed).`, "Check DevToolsActivePort in the user-data folder: if it is missing, remote debugging never started."] };
}
