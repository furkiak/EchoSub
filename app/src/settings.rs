use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Çeviri hedef dili (ISO kodu, ör. "tr").
    pub target_lang: String,
    /// Konuşulan dil; "auto" = otomatik algıla.
    pub source_lang: String,
    /// "fast" | "balanced" | "quality" (model boyutlarını seçer).
    pub quality: String,
    pub font_family: String,
    /// Kullanıcı yazı boyutu çarpanı (otomatik sığdırmada üst sınırı etkiler).
    pub font_scale: f32,
    /// Pencere büyüdükçe yazı da büyüsün.
    pub auto_fit: bool,
    pub bold: bool,
    pub text_color: String,
    pub outline_color: String,
    pub outline_width: f32,
    pub bg_color: String,
    pub bg_opacity: f32,
    /// Aynı anda görünen en fazla satır (cümle) sayısı.
    pub lines: u32,
    /// Çevirinin altında orijinal metni de göster.
    pub show_original: bool,
    /// Bu kadar saniye yeni konuşma olmazsa altyazı kaybolur.
    pub hide_after_secs: f32,
    /// Sunucu tarafı Silero VAD (müzik/gürültü filtresi).
    pub use_vad: bool,
    /// Okunamayan (müzik/şarkı) parçalarda vokali ayırıp yeniden dene.
    pub use_separation: bool,
    /// Altyazı alanındaki tıklamalar alttaki pencereye geçsin; yalnızca üst
    /// şerit ve kenarlar (sürükleme/boyutlandırma/düğmeler) tıklanabilir kalır.
    pub click_through: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_lang: "tr".into(),
            source_lang: "auto".into(),
            quality: "balanced".into(),
            font_family: "Segoe UI, Arial, sans-serif".into(),
            font_scale: 1.0,
            auto_fit: true,
            bold: true,
            text_color: "#ffffff".into(),
            outline_color: "#000000".into(),
            outline_width: 3.0,
            bg_color: "#000000".into(),
            bg_opacity: 0.0,
            lines: 2,
            show_original: false,
            hide_after_secs: 6.0,
            use_vad: true,
            use_separation: true,
            click_through: true,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }

    /// (whisper model dosyası, çeviri model dosyası)
    pub fn model_files(&self) -> (&'static str, &'static str) {
        match self.quality.as_str() {
            "fast" => ("ggml-small-q5_1.bin", "translategemma-4b-it-Q4_K_M.gguf"),
            "quality" => ("ggml-large-v3-turbo-q5_0.bin", "translategemma-12b-it-Q4_K_M.gguf"),
            _ => ("ggml-large-v3-turbo-q5_0.bin", "translategemma-4b-it-Q4_K_M.gguf"),
        }
    }

    /// Sidecar yeniden başlatma gerektiren alanlar değişti mi?
    pub fn needs_restart(&self, other: &Settings) -> bool {
        self.quality != other.quality || self.use_vad != other.use_vad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("echosub-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("settings.json")
    }

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::default();
        assert_eq!((s.target_lang.as_str(), s.source_lang.as_str()), ("tr", "auto"));
        assert_eq!(s.quality, "balanced");
        assert!(s.click_through && s.use_separation && s.use_vad && s.auto_fit);
    }

    #[test]
    fn missing_or_corrupt_file_falls_back_to_defaults() {
        let p = tmp("corrupt");
        assert_eq!(Settings::load(&p), Settings::default());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ bozuk json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_fields() {
        let p = tmp("partial");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, r#"{"target_lang":"de","click_through":false}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.target_lang, "de");
        assert!(!s.click_through);
        assert_eq!(s.quality, "balanced");
    }

    #[test]
    fn save_then_load_roundtrip_creates_directories() {
        let p = tmp("roundtrip");
        let s = Settings { target_lang: "ja".into(), font_scale: 0.8, lines: 3, ..Settings::default() };
        s.save(&p);
        assert_eq!(Settings::load(&p), s);
    }

    #[test]
    fn quality_selects_models() {
        let mut s = Settings::default();
        s.quality = "fast".into();
        assert_eq!(s.model_files(), ("ggml-small-q5_1.bin", "translategemma-4b-it-Q4_K_M.gguf"));
        s.quality = "quality".into();
        assert_eq!(s.model_files().1, "translategemma-12b-it-Q4_K_M.gguf");
        s.quality = "bilinmeyen".into();
        assert_eq!(s.model_files(), Settings::default().model_files());
    }

    #[test]
    fn only_server_side_changes_need_restart() {
        let a = Settings::default();
        let mut b = a.clone();
        b.text_color = "#ff0000".into();
        b.target_lang = "en".into();
        b.click_through = false;
        assert!(!a.needs_restart(&b));
        b.quality = "quality".into();
        assert!(a.needs_restart(&b));
        let mut c = a.clone();
        c.use_vad = false;
        assert!(a.needs_restart(&c));
    }
}
