use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// "cpu" or "gpu" (chosen in the installer).
    #[serde(default = "default_compute")]
    pub compute: String,
    /// Folder holding the MedGemma .gguf files (HDD / USB / install dir).
    #[serde(default)]
    pub model_dir: String,
    /// Memory mode: minutes of inactivity before the model is unloaded from RAM.
    /// 0 = unload right after every answer, 9999 = never (keep loaded).
    #[serde(default = "default_idle")]
    pub idle_minutes: u32,
    /// File name of a variant the user picked explicitly in the app (empty = automatic policy).
    #[serde(default)]
    pub forced_model: String,
}
fn default_idle() -> u32 { 5 }
fn default_compute() -> String { "cpu".into() }

impl Default for Config { fn default() -> Self { Config { compute: default_compute(), model_dir: String::new(), idle_minutes: default_idle(), forced_model: String::new() } } }

impl Config {
    pub fn load(p: &Path) -> Option<Config> {
        // Tolerate a UTF-8 BOM (Inno Setup's SaveStringToFile can emit one).
        let s = std::fs::read_to_string(p).ok()?;
        serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
    }
    pub fn save(&self, p: &Path) -> std::io::Result<()> {
        if let Some(d) = p.parent() { std::fs::create_dir_all(d)?; }
        // write-then-rename so a crash can never leave a half-written config
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap())?;
        std::fs::rename(tmp, p)
    }
    pub fn model_path(&self) -> PathBuf { PathBuf::from(&self.model_dir) }
}
