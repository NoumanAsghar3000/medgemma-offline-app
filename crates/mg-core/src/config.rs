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
    /// The install.json text these settings were derived from. If the installer is run again (new CPU/GPU or model-folder choice) the
    /// text changes and the installer's choices replace the saved ones; otherwise the user's own in-app choices are kept.
    #[serde(default)]
    pub seed: String,
}
fn default_idle() -> u32 { 5 }
fn default_compute() -> String { "cpu".into() }

impl Default for Config { fn default() -> Self { Config { compute: default_compute(), model_dir: String::new(), idle_minutes: default_idle(), forced_model: String::new(), seed: String::new() } } }

impl Config {
    pub fn parse(s: &str) -> Option<Config> {
        // Tolerate a UTF-8 BOM (Inno Setup's SaveStringToFile can emit one).
        serde_json::from_str(s.trim_start_matches('\u{feff}')).ok()
    }
    pub fn load(p: &Path) -> Option<Config> { Config::parse(&std::fs::read_to_string(p).ok()?) }
    /// Combine the user's saved settings with the installer's `install.json` (`seed_raw` = its text).
    /// User settings win, EXCEPT when the installer has been run again since they were derived: then the installer's compute and
    /// model folder win (and a forced variant is dropped), while the memory mode the user chose is kept.
    /// Settings saved by older versions (no `seed` yet) are treated as current.
    pub fn resolve(user: Option<Config>, seed_raw: Option<&str>) -> Config {
        let stamp = seed_raw.map(|s| s.trim_start_matches('\u{feff}').trim().to_string()).unwrap_or_default();
        let seed = seed_raw.and_then(Config::parse);
        match (user, seed) {
            (Some(u), Some(s)) if !u.seed.is_empty() && u.seed != stamp =>
                Config { compute: s.compute, model_dir: s.model_dir, forced_model: String::new(), idle_minutes: u.idle_minutes, seed: stamp },
            (Some(mut u), _) => { if u.seed.is_empty() { u.seed = stamp; } u }
            (None, Some(mut s)) => { s.seed = stamp; s }
            (None, None) => Config::default(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    const INSTALL_A: &str = r#"{"compute":"cpu","model_dir":"D:\\Models"}"#;
    const INSTALL_B: &str = r#"{"compute":"gpu","model_dir":"E:\\NewModels"}"#;
    fn user(seed: &str) -> Config { Config { compute: "cpu".into(), model_dir: "F:\\Relinked".into(), idle_minutes: 30, forced_model: "x.gguf".into(), seed: seed.into() } }

    #[test] fn no_user_settings_use_the_installer_and_remember_it() {
        let c = Config::resolve(None, Some(INSTALL_A));
        assert_eq!((c.compute.as_str(), c.model_dir.as_str()), ("cpu", "D:\\Models")); assert_eq!(c.seed, INSTALL_A);
        assert_eq!(Config::resolve(None, None).compute, "cpu");
    }
    #[test] fn users_own_choices_survive_while_install_json_is_unchanged() {
        let c = Config::resolve(Some(user(INSTALL_A)), Some(INSTALL_A));
        assert_eq!((c.model_dir.as_str(), c.idle_minutes, c.forced_model.as_str()), ("F:\\Relinked", 30, "x.gguf"));
    }
    #[test] fn rerunning_the_installer_replaces_compute_and_folder_but_keeps_memory_mode() {
        let c = Config::resolve(Some(user(INSTALL_A)), Some(INSTALL_B));
        assert_eq!((c.compute.as_str(), c.model_dir.as_str(), c.forced_model.as_str(), c.idle_minutes), ("gpu", "E:\\NewModels", "", 30));
        assert_eq!(c.seed, INSTALL_B);
    }
    #[test] fn settings_from_older_versions_are_kept_and_get_stamped() {
        let c = Config::resolve(Some(user("")), Some(INSTALL_B));
        assert_eq!((c.model_dir.as_str(), c.compute.as_str()), ("F:\\Relinked", "cpu")); assert_eq!(c.seed, INSTALL_B);
    }
    #[test] fn a_bom_in_install_json_is_tolerated() {
        let raw = format!("\u{feff}{INSTALL_A}");
        assert_eq!(Config::resolve(None, Some(&raw)).model_dir, "D:\\Models");
    }
}
