# Optional: verifying the GitHub run and the installer with browser-use

**Not part of the CI and not required.** The workflow already proves the important behaviour deterministically on a real Windows runner
(silent install, upgrade, launch, re-link dialog, attachments, fake camera, PDF save with content assertions, uninstall with zero leftovers),
and `tools/verify_actions_run.py` + `tools/check_artifacts.py` check a finished run and its downloadable installers without any browser.

browser-use (https://github.com/browser-use/browser-use) drives a browser with an LLM. It needs an LLM API key (internet) and its steps are not deterministic,
so it is deliberately **not** wired into the workflow (it would make CI heavier and flakier without adding proof). It is useful only as a *human-style second opinion*,
run by a supervisor on their own PC. It can open GitHub pages and download files; it cannot reliably operate Windows installers, so install/uninstall checks stay with the PowerShell tests.

Example task prompt (run it from your own environment, with your own key):

> Open https://github.com/<owner>/<repo>/actions. Open the most recent run of the workflow "build-installer".
> Report whether every job (verify-linux, ui-browser, build (slim), build (with-WebView2), verify-artifacts) has a green check.
> In the run summary, download the artifacts "MedGemmaOffline-installer-slim", "MedGemmaOffline-installer-with-webview2", "test-evidence" and "ui-screenshots".
> Tell me the file names and sizes of the two .exe files and open the screenshots in "ui-screenshots" and "test-evidence" and describe what each shows.

Then run `python tools/check_artifacts.py <download folder>` for the deterministic checks. I have **not** run browser-use here.
