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
  const hasPort = web.some((l) => l.includes(`--remote-debugging-port=${port}`));
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
