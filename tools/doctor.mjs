#!/usr/bin/env node
// MedGemma Offline doctor. Examples (Windows):
//   node tools\doctor.mjs --models D:\Models --probe "C:\Program Files\MedGemmaOffline\tools\mg-probe.exe"
//   node tools\doctor.mjs --app "C:\Program Files\MedGemmaOffline\medgemma-offline.exe" --exercise-camera --models D:\Models --probe ...mg-probe.exe
//   node tools\doctor.mjs --attach-port 9333                     (an app that was started with --remote-debugging-port=9333)
// Exit code 0 = no problem found, 1 = at least one check FAILED. --json <file> writes the full result.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { formatReport, runDoctor } from "./doctor_lib.mjs";

const argv = process.argv.slice(2);
const arg = (k) => { const i = argv.indexOf(k); return i >= 0 ? argv[i + 1] : undefined; };
const flag = (k) => argv.includes(k);
if (flag("--help") || flag("-h")) {
  console.log("usage: doctor.mjs [--models <dir>] [--probe <mg-probe exe>] [--app <exe> | --attach-port <n>] [--exercise-camera] [--wait <seconds>] [--work <dir>] [--json <file>]");
  process.exit(0);
}
const workDir = path.resolve(arg("--work") ?? "doctor-work"); mkdirSync(workDir, { recursive: true });
const result = await runDoctor({
  modelsDir: arg("--models"), probeExe: arg("--probe"), appExe: arg("--app"), attachPort: arg("--attach-port") ? Number(arg("--attach-port")) : undefined,
  workDir, waitSeconds: Number(arg("--wait") ?? 60), exerciseCamera: flag("--exercise-camera"),
});
console.log(formatReport(result));
if (arg("--json")) writeFileSync(arg("--json"), JSON.stringify(result, null, 2));
process.exit(result.ok ? 0 : 1);
