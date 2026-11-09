# MedGemma Offline - installable Windows desktop app (27B multimodal, 4B fallback)

100 % offline after install. Chat UI with **+** (attach files / camera), local llama.cpp inference, a real
Inno Setup installer built by GitHub Actions.

## 1. Architecture and why

```
medgemma-offline/
├─ crates/mg-core/      Rust lib - hardware detection, GGUF scan, "largest variant that fits", backend supervisor
├─ crates/mg-probe/     Rust CLI (~300 KB) - the installer calls it for REAL RAM/GPU detection + model choice
├─ src-tauri/           Rust/Tauri 2 shell - window, config, model-folder check + re-link dialog, starts llama-server
├─ src/ index.html      Vanilla TypeScript UI (Vite) - chat, + menu, camera, pdf.js / mammoth / UTIF in-browser
├─ installer/medgemma.iss   Inno Setup wizard (dir -> CPU/GPU -> RAM/model -> install)
├─ tools/prepare_models.py  download or convert+quantize the model folder
└─ .github/workflows/build.yml
```

| Part | Choice | Why (stability + size) |
|---|---|---|
| Shell | **Tauri 2 (Rust)** | ~5 MB exe vs ~90 MB for Electron; uses the system WebView2, no bundled Chromium. |
| UI | **Vanilla TS + Vite** | No framework runtime; whole UI bundle ~0.8 MB (+1.4 MB pdf.js worker). |
| Inference | **llama.cpp `llama-server.exe`** (prebuilt, CPU + Vulkan) | Native GGUF + multimodal (`--mmproj`), OpenAI-style API with images, no Python runtime (saves 300 MB-1 GB), separate process so a model crash can never freeze the UI. |
| GPU | **Vulkan build** | One ~small build for NVIDIA + AMD + Intel Arc, no 400 MB CUDA runtime. (CUDA is faster on NVIDIA but much bigger.) |
| Hardware logic | **Rust, shared by installer and app** | The installer (`mg-probe.exe`) and the runtime use the *same* code, so they can never disagree. |
| Installer | **Inno Setup, LZMA2 ultra** | Native custom pages; compression done once. UPX is deliberately *not* used: it saves ~nothing inside an LZMA installer and causes antivirus false positives. |
| Model | **Pre-quantized GGUF ladder** | See §3. |

Stability measures: model server is a separate process, bound to `127.0.0.1` with a random API key, one slot (`-np 1`),
killed automatically (Windows job object) even if the app crashes; the UI never blocks - loading happens on a worker thread
with a live status overlay; config is written atomically.

## 2. Installer flow (as specified)

1. **Install directory** - default `C:\MedGemmaOffline`, user can change disk/partition.
2. **Processor page** - *Run backend model on CPU* / *GPU*. On every visit `mg-probe hw` runs for real:
   * GPU = hardware DXGI adapter (software "Basic Render" rejected) with ≥ 2 GB VRAM **and** `vulkan-1.dll` loadable.
   * GPU chosen but not available → red text **on the same page**: *"GPU is not available currently in this system"*, **Next disabled**. No message box.
   * CPU → Next enabled.
   * < 4 GB *installed* RAM (`GetPhysicallyInstalledSystemMemory`) → both options disabled, red message, Next disabled.
3. **Model page** - browse to the folder (HDD/USB). `mg-probe plan` reads **free RAM**, budget = free − 1 GB, and shows the pick:
   27B multimodal (largest quant whose weights + mmproj + ~1.5 GB KV/compute fit) → else 4B multimodal → else 4B text-only (flagged).
   No fit → red message, Next disabled. Optional checkbox copies the chosen files into `{app}\models`; default is *use in place*.
4. Install; writes `{app}\install.json`; installs WebView2 offline if the bundled variant is used and it is missing.

Silent install: `Setup.exe /VERYSILENT /COMPUTE=cpu /MODELDIR=E:\Models`.

## 3. Preparing the model folder

```bash
pip install huggingface_hub
python tools/prepare_models.py download --out D:\MedGemmaModels          # ready-made GGUF + mmproj
# or your own / fine-tuned weights:
python tools/prepare_models.py convert --hf-dir C:\my-medgemma-27b --size 27b --out D:\MedGemmaModels --llama-cpp C:\llama.cpp
```
Folder contents (names must contain `27b`/`4b`, the quant tag, and `mmproj` for the vision file):
`medgemma-27b-it-{Q8_0,Q6_K,Q5_K_M,Q4_K_M,Q3_K_M}.gguf` + `mmproj-medgemma-27b-it-F16.gguf`, and
`medgemma-4b-it-{Q8_0,Q6_K,Q4_K_M}.gguf` + `mmproj-medgemma-4b-it-F16.gguf`.

**Why not quantize during installation?** 27B needs the 54 GB F16 file and 10-30 min of CPU; that is fragile on a consumer PC.
Instead the installer *selects* the largest pre-built rung that fits. The rungs you ship are your quality floor -
omit `Q2_K`/`Q3_*` if you do not want 27B to degrade that far (a 4B Q8_0 can be better than a 27B Q2_K).
A 27B file without its mmproj is skipped (it would not be multimodal). Text-only `*-text-it` files are ignored.

Realistic RAM: 27B Q4_K_M ≈ 16.5 GB + 0.9 + 1.5 ⇒ ~19 GB free (+1 GB headroom = 20 GB). Q3_K_M ≈ 16 GB.
CPU-only 27B generates only a few tokens/second; GPU offload (partial layers are computed from VRAM) helps a lot.

## 4. Runtime model-folder recovery

On every launch `backend::prepare` checks the saved folder (user config in `%APPDATA%`, else installer's `install.json`).
Missing / renamed / deleted / USB unplugged / no `.gguf` inside → the chat is blocked by a dialog:
**"Backend model files not found"** with the last path and **Browse for model folder…** (native folder picker).
The choice is validated (must contain MedGemma `.gguf` files), saved atomically, and the backend restarts - variant selection
runs *again* against whatever is in the new folder and current free RAM.

## 5. Build

Push to GitHub and run the **build-installer** workflow (or push a `v*` tag). Artifacts:
`MedGemmaOffline-Setup-<ver>.exe` (slim, needs WebView2 - present on Win 11 / updated Win 10) and
`...-with-WebView2.exe` (+~170 MB, installs on a clean Win 10 / LTSC with no internet). CI prints each size.
Local dev: `npm i && npm run tauri dev` (put `backend\cpu\llama-server.exe` next to the debug exe), `cargo test -p mg-core`.

## 6. Offline test plan

1. Build the installer; copy it + the model folder to a USB drive.
2. On a clean Windows VM: disable the network adapter. Install (slim variant must find WebView2; otherwise use the bundled one).
3. Wizard checks: pick GPU on a VM without GPU → red text, Next disabled. Run on a ≤ 3.9 GB-RAM VM → blocked. Pick the model folder → verify the shown variant against free RAM.
4. Launch: chat with a text question; attach a JPG, a scanned PDF, a text PDF, DOCX, CSV, TIFF; use **+ → Camera**.
5. Close app; rename the model folder; relaunch → "Backend model files not found" → browse to the folder → model loads; relaunch → no prompt.
6. Kill the app from Task Manager → `llama-server.exe` must disappear too (job object).
7. `Wireshark`/Resource Monitor: only 127.0.0.1 traffic.

## 7. Known limits

* Context is 8 192 tokens for 27B (16 384 for 4B) to bound RAM; long PDFs are truncated with a visible notice, max 6 scanned pages become images.
* `.doc` (old Word) is not supported; save as `.docx`/PDF. HEIC is not supported.
* Output is decision support, not diagnosis (also shown in the UI and system prompt).
