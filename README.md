# MedGemma Offline - installable Windows app (MedGemma 27B multimodal, 4B fallback)

Fully offline medical chat + report app: Tauri 2 (Rust) shell, vanilla TypeScript UI, llama.cpp `llama-server` (pinned **b12244**), Inno Setup installer built by GitHub Actions.
**You download the model files from Hugging Face yourself and point the app at the folder. The app never downloads anything.**

```
crates/mg-core/    hardware detection, GGUF header validator, TIER PLANNER, selection, backend supervisor, loopback client   (Rust, shared)
crates/mg-probe/   CLI used by the installer wizard AND by you:  mg-probe doctor --dir <models folder>
src-tauri/         native shell: config, model-folder recovery, chat proxy, memory modes, variant selector, "Save PDF"
src/               UI: chat, + menu (files/camera), pdf.js / mammoth / UTIF, CSV summary, structured report + PDF (pdf-lib)
installer/         medgemma.iss      tools/  e2e_webview.mjs, drive_wizard.ps1, verify_install.ps1, stage_engines.py, offline_audit.py, make_tiny_gemma3.py
.github/workflows/build.yml
```

## 1. Prepare your model folder

The official bases `google/medgemma-27b-it` and `google/medgemma-4b-it` are **gated** (accept the Health AI Developer Foundations terms) and are safetensors - this app needs **GGUF** files, so use one of the public GGUF repositories below.
Install the downloader once: `pip install -U "huggingface_hub[cli]"` (log in with `huggingface-cli login` if a repo asks for the terms).

**Required layout** (the `27b` and `4b` folder names matter - several repos call the vision file just `mmproj-F16.gguf`, so the family is read from the nearest folder name *and* verified from the file headers):
```
models\
  27b\   medgemma-27b-it-Q8_0.gguf   (or any other quant, several may sit side by side)
         mmproj-F16.gguf            (or mmproj-BF16.gguf / mmproj-MedGemma-27B-IT-F16.gguf / ...)
  4b\    medgemma-4b-it-Q4_K_M.gguf (or higher)
         mmproj-*.gguf
```
Point the installer / app at the `models` folder (it also works if you point it at `27b` or `4b` alone).

**A. Unsloth 27B (primary target)** - single files, no splits:
```
huggingface-cli download unsloth/medgemma-27b-it-GGUF --include "medgemma-27b-it-Q8_0.gguf" "mmproj-F16.gguf" --local-dir D:\models\27b
:: optional smaller rungs the planner can fall back to (same folder):  Q6_K 22.2 GB, Q5_K_M 19.3 GB, Q4_K_M 16.5 GB, Q3_K_M 13.4 GB, and the UD-* variants
huggingface-cli download unsloth/medgemma-27b-it-GGUF --include "medgemma-27b-it-Q6_K.gguf" "medgemma-27b-it-Q4_K_M.gguf" --local-dir D:\models\27b
```
Q8_0 = 28.7 GB; `mmproj-F16.gguf` and `mmproj-BF16.gguf` = 858 MB each (verified from the repository listing).

**B. Unsloth 4B (mandatory fallback)** - Q8_0 4.13 GB, Q6_K 3.19, Q5_K_M 2.83, Q4_K_M ~2.4-2.5 GB:
```
huggingface-cli download unsloth/medgemma-4b-it-GGUF --include "medgemma-4b-it-Q8_0.gguf" "medgemma-4b-it-Q4_K_M.gguf" "mmproj-*F16*.gguf" --local-dir D:\models\4b
```
(The 4B repo's exact vision-file name could not be read here; the `mmproj-*F16*` pattern matches `mmproj-F16.gguf` / `mmproj-BF16.gguf`-style names - run `doctor` afterwards.)

**C. Alternative clean 27B pair (`unofficial-jona`; names as given in your brief - not independently verified by me):**
```
huggingface-cli download unofficial-jona/MedGemma-27B-IT-GGUF --include "MedGemma-27B-IT-Q4_K_M.gguf" "mmproj-MedGemma-27B-IT-F16.gguf" --local-dir D:\models\27b
```
**D. bartowski (27B)** uses `google_medgemma-27b-it-<quant>.gguf` and `mmproj-google_medgemma-27b-it-bf16.gguf`; 4B vision file `mmproj-google_medgemma-4b-it-f16.gguf`.
Some of its large quantizations are **split** (`...-00001-of-0000N.gguf`): split files are rejected with an explanation - use the single-file Unsloth quantization instead.

**E. kelkalot 4B, 8-bit pair** (`kelkalot/medgemma-4b-it-GGUF`): `medgemma-4b-it-Q8_0.gguf` (4.13 GB) + `mmproj-medgemma-4b-it-Q8_0.gguf` (591 MB):
```
huggingface-cli download kelkalot/medgemma-4b-it-GGUF --include "medgemma-4b-it-Q8_0.gguf" "mmproj-medgemma-4b-it-Q8_0.gguf" --local-dir D:\models\4b
```
**F. mradermacher 27B** (`mradermacher/medgemma-27b-multimodal-GGUF`, a community re-merge of MedGemma 27B multimodal): `Q8_0` = 28.8 GB, vision files `mmproj-Q8_0` (0.7 GB) and `mmproj-f16` (1.0 GB). The repo names its static quantizations `<model>.<quant>.gguf` and `<model>.mmproj-<quant>.gguf`; use the file picker on the repo's *Files* tab for the exact names, then:
```
huggingface-cli download mradermacher/medgemma-27b-multimodal-GGUF --include "*.Q8_0.gguf" "*mmproj-f16.gguf" --local-dir D:\models\27b
```

### Your own fine-tuned models (8-bit or 16-bit) - e.g. a mammography fine-tune
The app accepts **any correctly paired Gemma-3 multimodal GGUF + mmproj**, whatever the file names, because it decides from the file *contents*:
1. Put the fine-tuned **27B** files in `models\27b\` and the **4B** files in `models\4b\`. File names are free (`mammo-27b-ft-Q8_0.gguf`, `mammo-27b-ft-f16.gguf`, `mmproj-mammo-27b-ft-F16.gguf` ...). Several precisions can sit side by side; the planner lists each and picks the largest usable one (8-bit/16-bit first, 27B first).
2. Each model needs **its own projector from the same fine-tune** (the projector's output width must equal the model width: 27B = 5376, 4B = 2560). A 4B projector next to a 27B model is rejected with a precise message; a flat folder is also fine - the pairing is then made by width.
3. The model must be `gemma3` architecture (62 layers for 27B, 34 for 4B). Fine-tunes keep this. The only soft warning you may see is "the model is named 'x', not MedGemma", which is informational for your own model.
4. 16-bit means F16/BF16 files: ~54 GB for 27B (the planner will rate that by your RAM/drive speed; 4B in 16-bit is ~8 GB and fine). Use Q8_0 (28.7 GB) for 27B on most PCs.
5. Converting from your merged Hugging Face checkpoint (needs a llama.cpp checkout and `llama-quantize`; run on your training PC):
```
python tools\prepare_models.py convert --hf-dir C:\ft\mammo-27b-merged --size 27b --prefix mammo-27b-ft --quants Q8_0,F16 --out D:\models\27b --llama-cpp C:\llama.cpp
python tools\prepare_models.py convert --hf-dir C:\ft\mammo-4b-merged  --size 4b  --prefix mammo-4b-ft  --quants Q8_0,F16 --out D:\models\4b  --llama-cpp C:\llama.cpp
```
(this runs llama.cpp's `convert_hf_to_gguf.py` for the model and with `--mmproj` for the projector, then `llama-quantize`). **Model files stay on your disk/USB; nothing is ever uploaded and no model file is part of this repository** (`*.gguf` is git-ignored; the only `.gguf` files in the repo are eight tiny (<0.4 MB) test fixtures, allowed explicitly in `.gitignore`).
Then check everything with `mg-probe.exe doctor --dir D:\models`.

**Names that are accepted** (all unit-tested, including the kelkalot / mradermacher styles above and any name for your own files when the headers are right): `medgemma-27b-it-Q8_0|Q6_K|Q5_K_M|Q4_K_M.gguf`, `...-UD-Q4_K_XL|UD-Q8_K_XL|UD-IQ2_M|UD-Q2_K_XL.gguf`, `MedGemma-27B-IT-Q4_K_M.gguf`, `google_medgemma-27b-it-*.gguf`, the same for 4B, and vision files
`mmproj-F16.gguf`, `mmproj-BF16.gguf`, `mmproj-MedGemma-27B-IT-F16.gguf`, `mmproj-google_medgemma-27b-it-bf16.gguf`, `mmproj-google_medgemma-4b-it-f16.gguf`, `mmproj-model-F16.gguf`, `medgemma-4b-it-mmproj-f16.gguf`, `mmproj-medgemma-4b-it-Q8_0.gguf`, `medgemma-4b-it.mmproj-Q8_0.gguf`, `medgemma-4b-it.F16-mmproj.gguf`. Text-only `medgemma-27b-text-it` files are ignored.

**The validator reads the real GGUF headers** (a few MB per file, never the weights) and rejects bad files with a precise message:

| Check | Source in the file | Result |
|---|---|---|
| architecture is `gemma3` | `general.architecture` | else error "this is not a Gemma 3 model" |
| layer count vs folder/name (27B = 62, 4B = 34) | `<arch>.block_count` | error when a 27B file sits in the 4B folder (or vice versa) |
| quantization | `general.file_type` (name used first) | shown in the UI / `doctor` |
| vision file is a projector | `general.architecture == clip` | else error |
| projector type | `clip.projector_type` must be `gemma3` | else error (e.g. `qwen2vl`) |
| projector output size vs model width (27B 5376, 4B 2560) | `clip.vision.projection_dim` vs `<arch>.embedding_length` | **error when both are Gemma-3 widths and differ** (4B mmproj with the 27B model); other values only warn |
| plain (non-Med) Gemma 3 | `general.name` | warning |
| split GGUF part | file name | error with advice |

Run **`mg-probe.exe doctor --dir D:\models`** (installed to `<install folder>\tools\`) to see every file, problem and the planner's estimate for each variant. The same reasons appear in the app's "Backend model files not found" dialog.

## 2. Speed on 8 GB-class machines - the honest picture

MedGemma 27B is **dense**: every weight is read for every token, so speed ~ memory bandwidth / bytes that must be read. Nothing makes 28.7 GB of 8-bit weights fast through 8 GB of RAM or VRAM.
Planner estimates (40 GB/s RAM assumed; worst - best case; real speed can differ by 2x):

| Variant / machine | est. tokens/s | re-read from drive per token |
|---|---|---|
| 27B **Q8_0**, CPU, 7 GB usable RAM, NVMe 3 GB/s | 0.10 - 0.11 | 24 GB |
| 27B Q8_0, CPU, 7 GB usable RAM, USB 200 MB/s | 0.01 | 24 GB |
| 27B Q8_0, **8 GB VRAM** GPU + 7 GB RAM, NVMe | 0.12 - 0.16 | 17 GB |
| 27B Q8_0, 24 GB VRAM GPU + 7 GB RAM, NVMe | 0.28 - 0.61 | 4 GB |
| 27B Q8_0, CPU, ~34 GB usable RAM (fully resident) | ~1.2 | 0 |
| 27B Q4_K_M, CPU, 19 GB usable RAM | ~2.3 | 0 |
| 27B Q3_K_M, 8 GB VRAM GPU + 10 GB usable RAM | ~4 | 0 |
| **4B Q8_0**, CPU, 7 GB usable RAM | ~9 | 0 |
| **4B Q8_0**, 8 GB VRAM GPU | ~45 | 0 |

**Product behaviour:** (1) automatic choice = the largest 27B multimodal quantization (8-bit first) whose *worst-case* estimate is >= 0.5 tok/s, else the best 4B; (2) estimates are shown in the installer, in the header drop-down (every installed variant) and in an orange banner;
(3) tokens/s is shown **live while streaming** and **measured** after every answer; (4) forcing a slow variant (e.g. 27B Q8) opens a blocking "Very slow on this computer" dialog with the time for 1,000 tokens and keeps a red **FORCED SLOW VARIANT** banner;
(5) memory modes: keep loaded / free RAM after 5 min idle (default) / free RAM after every answer; weights are memory-mapped from disk/USB and ~1 GB RAM is always left free.

## 3. Colibri (https://github.com/JustVugg/colibri): taken vs adapted

Colibri is a hierarchical engine for **mixture-of-experts** models. Its supported-architecture list has **no `gemma3`**, so it cannot run MedGemma, and for its own dense 27B it documents that "every weight is read for every token" (29.1 GB RAM int8, ~2.25 tok/s). Expert-streaming ideas (also kimi-k3-in-c / TurboFieldfare style) do not apply to a dense model.
**Taken:** the VRAM -> RAM -> NVMe placement and the "measure, then plan" approach (`coli plan`/`doctor`) -> tier planner + `bench.rs` + `mg-probe doctor`; mmap'd weights streamed from disk; aggressive unloading. **Adapted:** the OS page cache does the streaming and the planner uses the *worst case* (no cache hits on a cyclic scan). **Not applicable, not faked:** hot-expert pinning, router prefetch, learn-from-use. **Not enabled:** speculative decoding with the 4B as draft (could not be verified with the vision projector).

## 4. Installer (professional behaviour)

Inno Setup 6.3, built in CI as **two installers**: slim (needs WebView2 already present, never checks online) and **-with-WebView2** (bundles Microsoft's offline Evergreen runtime for clean Windows 10 / LTSC / Superlite).
* Wizard: destination (default `C:\MedGemmaOffline`) -> processor page (CPU/GPU, live probe) -> model-folder page. GPU selected but unavailable -> bold red **"GPU is not available currently in this system"** on the same page, **Next disabled**, no message box. < 4 GB installed RAM -> options disabled, red text, Next disabled. Model page shows the planner's choice (27B first, else 4B, 1 GB headroom) and estimated speed; unusable folders show the exact reason.
* **Start Menu group** (app + Uninstall), **optional Desktop shortcut**, registered in **Apps & features** (name, version, publisher, icon, uninstaller).
* **Silent**: `/VERYSILENT /SUPPRESSMSGBOXES /DIR="C:\MedGemmaOffline" /TASKS=desktopicon /COMPUTE=cpu /MODELDIR=D:\models`. No internet during install or uninstall.
* **Upgrade**: same AppId -> installs in place, detects the previous version (logged), closes a running copy first, removes the old engine/tool files so no stale DLLs survive.
* **Uninstall**: removes program files, Start Menu group, Desktop shortcut, the Apps & features entry, the copied-model folder, `install.json`, and the per-user settings/log/WebView2-profile folders (`%APPDATA%` and `%LOCALAPPDATA%\com.nouman.medgemma.offline`). **Your model folder is never touched.**
* Every launch: a missing/moved/renamed/unusable model folder shows **"Backend model files not found"** (+ reason) and a folder browser; the new path is validated, saved, and the model reloaded.
* The installer is unsigned: SmartScreen may warn on first run ("More info" -> "Run anyway").

## 5. Supervisor test plan (clean offline Windows PC)

1. Take `MedGemmaOffline-Setup-*-with-WebView2.exe` (from the workflow's artifact) and the `models` folder (section 1) on an SSD/USB. Disconnect the network.
2. Run the installer: destination defaults under `C:\`; GPU on a PC without GPU -> red text + Next disabled; CPU -> Next enabled; model page shows the automatic choice + estimated speed. Tick the Desktop shortcut. Finish.
3. *Settings -> Apps -> Installed apps*: "MedGemma Offline" with version and publisher is listed. Start Menu has the "MedGemma Offline" group.
4. Launch it. Header: open the variant list (estimates per variant); pick a "batch only" one -> red **Very slow** dialog -> Cancel.
5. Attach a JPG, a scanned PDF, a text PDF, a CSV (**+ -> Attach file**), take a photo (**+ -> Camera**), send. Expect a streamed `Findings / Impression / Recommendations` answer, live and measured tok/s.
6. **Download report (PDF)** -> form -> native Save dialog -> open the PDF (header, Findings, Impression, Recommendations, analysed files with thumbnails, model, timestamp, disclaimer).
7. Rename the models folder, relaunch -> "Backend model files not found" + folder picker -> pick it -> chat works.
8. Header memory mode "Free RAM after every answer": `llama-server.exe` disappears after the answer, returns on the next message.
9. Uninstall from *Settings -> Apps -> MedGemma Offline -> Uninstall* (or the Start Menu "Uninstall"). Check: `C:\MedGemmaOffline` gone, Start Menu group gone, Desktop shortcut gone, entry gone from Apps, `%APPDATA%\com.nouman.medgemma.offline` and `%LOCALAPPDATA%\com.nouman.medgemma.offline` gone, the models folder still intact.
10. Problems with model files: `<install folder>\tools\mg-probe.exe doctor --dir <models>`.
*If answers are very slow, that is the dense-27B physics in section 2: use the 4B or a smaller 27B quantization from the header.*

## 6. The clinical UI

Offline/private indicator, light/dark theme, welcome screen with clinical starters (mammography review, chest radiograph, labs), "+" menu (attach files / camera), attachment chips with thumbnails and click-to-enlarge, **streaming answers rendered as Findings / Impression / Recommendations cards** (numbered impressions stay numbered),
**live tokens/second while streaming and measured tokens/second after each answer**, model/variant selector with per-variant speed estimates, memory-mode selector, forced-slow-variant dialog + red banner, "Download report (PDF)" (native Save dialog), Copy, and **New case** (clears the conversation and attachments).
The UI was exercised in a **real browser engine (Firefox)** against a mocked backend (`tools/ui_check.py`, 23 checks, screenshots saved) - this found and fixed three real defects: the "+" menu was permanently open (CSS `display` overriding `hidden`), numbered lists rendered as bullets, and a missing Copy button.
It has *not* been run inside the real WebView2 by me; CI does that (section 7).

## 7. What the GitHub Actions workflow runs (any failing step fails the run)

Concurrency control (a newer push cancels an older run; tags never), per-job timeouts, download retries, pinned llama.cpp tag.
* `verify-linux`: Rust tests (planner, selection, **GGUF validator incl. all naming variants / fine-tune names / mismatched pairs / split files**, supervisor vs the **real llama-server b12244** + a tiny Gemma-3-arch model), TypeScript tests (PDF generation), UI build, **offline audit**, artifact-checker self-test.
* `ui-browser`: the built UI driven in **headless Firefox** against the mocked backend (`ui_check.py`); screenshots uploaded as `ui-screenshots`.
* `build` (real Windows runner; **slim and with-WebView2**): Rust tests on Windows, `tauri build` (Rust/Tauri + frontend), `mg-probe`, pinned engine download + trim, smoke tests (hardware probe + planner; staged `llama-server.exe` serving a Gemma-3-arch model), **Inno Setup build**, SHA-256 sums, installers uploaded first, then:
  * **slim lifecycle**: silent install -> `verify_install.ps1` (registration, shortcuts, files) -> uninstall -> zero-leftover verification (or the correct refusal when the runner lacks WebView2);
  * **bundled lifecycle**: install the **older 0.0.1** build, plant a stale file, **upgrade**, assert upgrade detection / one uninstall entry / new version / stale file gone / Desktop shortcut;
  * **E2E in the real WebView2** (`tools/e2e_webview.mjs`): installed app with a **missing model folder** -> "Backend model files not found" -> re-link -> persisted -> attach CSV/TXT/**PDF**/PNG -> **fake-camera capture** -> **one request with text + CSV + TXT + PDF + PNG + camera photo** (the CI test model has no projector, so the real server's "image input is not supported" proves the image parts arrived) -> text-only request answered (streamed, measured tok/s) -> report form -> **PDF saved and text-extracted** (title, patient, sections, file names, timestamp, disclaimer) -> no resource fetched from outside the app;
  * **uninstall + zero-leftover verification** (files, Start Menu, Desktop, registry, per-user data, processes);
  * evidence artifact `test-evidence` (e2e JSON, saved PDF, setup logs, and - best effort, non-blocking because it needs an interactive desktop - wizard screenshots + asserted wizard-state log from the REAL wizard with the REAL probe).
* `verify-artifacts`: downloads this run's installers and runs `tools/check_artifacts.py` (real Windows executables, Inno Setup signature, sizes, SHA-256 sums, bundled larger than slim).
The e2e uses two env-gated test hooks (`MG_TEST_PICK_FOLDER`, `MG_TEST_SAVE_PATH`) replacing ONLY the two native dialogs a headless test cannot click; never set for users.
**After a run**, a supervisor can confirm everything without a browser: `python tools/verify_actions_run.py --repo OWNER/REPO` (needs the GitHub CLI). browser-use is **not** used (see `docs/OPTIONAL_browser_use_verification.md` for why and an optional prompt).

## 8. Verification status (honest)

A fresh copy containing only the git-tracked files was run through the Linux CI commands (npm ci/test/build, offline audit, checker self-test, Firefox UI check, `cargo test -p mg-core`): all green, `Cargo.lock` unchanged. That simulation also caught two repo-level traps now fixed: `.gitignore` was excluding the test fixtures, and one test-server race was flaky under CPU load.

| Item | Status |
|---|---|
| Planner, selection, forced variants, GGUF validator (real gguf-py fixtures with real 27B/4B geometry), **every naming style** (Unsloth, kelkalot, mradermacher-style, bartowski, unofficial-jona names from your brief, custom fine-tune names, 16-bit), flat folders paired by projector width, split files, text-only detection | **Verified**: 42 Rust tests (38 unit + 3 integration + 1 against the real llama-server); the 38 unit tests also **executed as a Windows executable under Wine** |
| Supervisor vs the **real llama-server b12244** (Linux), incl. its answer to an image sent to a model without projector | **Verified** |
| **Real Windows `llama-server.exe`** with the trimmed file set | **Verified under Wine** (and runs on a Windows runner in CI) |
| **Installer lifecycle** with the real Inno Setup: install 0.0.1 -> upgrade 1.0.0 (detected, stale file removed, one entry) -> Apps & features key, Start Menu group, optional Desktop shortcut -> uninstall = no folder, no shortcuts, no registry entry, no per-user data | **Verified under Wine** (silent mode, 32-bit hardware-helper stand-in) |
| Installer wizard pages (red GPU text + Next disabled, CPU re-enables, <4 GB block, GPU present, model page) | **Verified under Wine** with screenshots and the wizard's own state log |
| **UI in a real browser engine** (Firefox, mocked backend): re-link dialog with reason, variant selector, attachments, camera (fake device), streaming + live tok/s, clinical cards, report -> PDF built and handed to the save command, forced-slow dialog + banner, theme, New case | **Verified**: 23 checks, screenshots |
| Whole Rust workspace incl. the Tauri shell | **Compiles for the Windows target** |
| Report generation + PDF text extraction; offline audit (negative-tested); artifact checker (self-tested); all 13 PowerShell sources, workflow YAML, Node/Python tools | **Verified** (parsed / run) |
| Real-WebView2 UI, camera in WebView2, native Save dialog, silent install/upgrade/uninstall on a real Windows runner, real probe in the wizard, Apps & features in Windows Settings | **Written and wired into CI, not executed by me** - the first Actions run is the proof (`test-evidence`, `ui-screenshots`) |
| **Loading your real fine-tuned or Hugging Face 27B / 4B weights, real mammography analysis, real speed** | **Requires your files.** Names/contents/pairing are validated and explained, the planner and `doctor` report; the real vision path is unproven until you run it |
| Speed numbers | Arithmetic from measured bandwidth, not benchmarks |
