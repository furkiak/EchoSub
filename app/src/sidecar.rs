//! Yapay zekâ sunucuları (whisper.cpp ve llama.cpp) alt süreç olarak çalışır.
//! İkisi de Vulkan ile AMD / NVIDIA / Intel GPU'da aynı şekilde hızlanır;
//! GPU yoksa CPU'ya düşer.

use crate::settings::Settings;
use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const WHISPER_PORT: u16 = 18080;
pub const LLAMA_PORT: u16 = 18081;

#[derive(Clone)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    /// ECHOSUB_HOME, yoksa <exe klasörü>/runtime, yoksa ./runtime.
    pub fn resolve() -> Paths {
        if let Ok(h) = std::env::var("ECHOSUB_HOME") {
            return Paths { home: PathBuf::from(h) };
        }
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("runtime")));
        match beside_exe {
            Some(p) if p.exists() => Paths { home: p },
            _ => Paths { home: PathBuf::from("runtime") },
        }
    }

    fn exe(&self, sub: &str, name: &str) -> PathBuf {
        let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        self.home.join(sub).join(file)
    }
    pub fn model(&self, file: &str) -> PathBuf {
        self.home.join("models").join(file)
    }
    pub fn log_dir(&self) -> PathBuf {
        self.home.join("logs")
    }
}

#[derive(Default)]
pub struct Sidecars {
    whisper: Option<Child>,
    llama: Option<Child>,
}

fn hide_console(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

/// Linux/macOS'ta sunucunun yanındaki paylaşımlı kütüphaneler (libggml*.so / .dylib)
/// bulunabilsin. (Windows DLL'leri zaten exe klasöründen yüklenir.)
fn lib_env(cmd: &mut Command, dir: &std::path::Path) {
    #[cfg(target_os = "linux")]
    {
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        cmd.env("LD_LIBRARY_PATH", format!("{}:{old}", dir.display()));
    }
    #[cfg(target_os = "macos")]
    {
        let old = std::env::var("DYLD_LIBRARY_PATH").unwrap_or_default();
        cmd.env("DYLD_LIBRARY_PATH", format!("{}:{old}", dir.display()));
    }
    #[cfg(windows)]
    {
        let _ = (cmd, dir);
    }
}

fn spawn_logged(mut cmd: Command, log: PathBuf) -> std::io::Result<Child> {
    if let Some(d) = log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let out = File::create(&log)?;
    let err = out.try_clone()?;
    cmd.stdin(Stdio::null()).stdout(Stdio::from(out)).stderr(Stdio::from(err));
    hide_console(&mut cmd);
    cmd.spawn()
}

pub fn wait_http(url: &str, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if let Ok(r) = ureq::get(url).timeout(Duration::from_secs(2)).call() {
            if r.status() == 200 {
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    false
}

impl Sidecars {
    pub fn stop(&mut self) {
        for c in [self.whisper.take(), self.llama.take()].into_iter().flatten() {
            let mut c = c;
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Whisper sunucusunu başlatır (hazır olmasını `wait_http` ile bekleyin).
    pub fn start_whisper(&mut self, paths: &Paths, s: &Settings) -> Result<(), String> {
        let (wm, _) = s.model_files();
        let model = paths.model(wm);
        let exe = paths.exe("whisper", "whisper-server");
        if !exe.exists() {
            return Err(format!("whisper-server bulunamadı: {}", exe.display()));
        }
        if !model.exists() {
            return Err(format!("whisper modeli bulunamadı: {}", model.display()));
        }
        let mut cmd = Command::new(&exe);
        lib_env(&mut cmd, exe.parent().unwrap());
        cmd.current_dir(exe.parent().unwrap())
            .arg("-m").arg(&model)
            .args(["--host", "127.0.0.1", "--port", &WHISPER_PORT.to_string()])
            .arg("-nlp")
            .args(["-nth", "0.6"]);
        let vad = paths.model("ggml-silero-v5.1.2.bin");
        if vad.exists() {
            cmd.arg("-vm").arg(&vad);
        }
        self.whisper = Some(
            spawn_logged(cmd, paths.log_dir().join("whisper.log")).map_err(|e| e.to_string())?,
        );
        Ok(())
    }

    /// Çeviri sunucusunu başlatır. Yürütülebilir ya da model yoksa Err döner
    /// (uygulama çeviri olmadan, yalnızca altyazıyla çalışmaya devam eder).
    pub fn start_llama(&mut self, paths: &Paths, s: &Settings) -> Result<(), String> {
        let (_, lm) = s.model_files();
        let model = paths.model(lm);
        let exe = paths.exe("llama", "llama-server");
        if !exe.exists() {
            return Err(format!("llama-server bulunamadı: {}", exe.display()));
        }
        if !model.exists() {
            return Err(format!("çeviri modeli bulunamadı: {}", model.display()));
        }
        let mut cmd = Command::new(&exe);
        lib_env(&mut cmd, exe.parent().unwrap());
        cmd.current_dir(exe.parent().unwrap())
            .arg("-m").arg(&model)
            .args(["--host", "127.0.0.1", "--port", &LLAMA_PORT.to_string()])
            .args(["-ngl", "99", "-c", "4096", "-np", "1"])
            .arg("--no-webui");
        self.llama = Some(
            spawn_logged(cmd, paths.log_dir().join("llama.log")).map_err(|e| e.to_string())?,
        );
        Ok(())
    }
}

impl Drop for Sidecars {
    fn drop(&mut self) {
        self.stop();
    }
}
