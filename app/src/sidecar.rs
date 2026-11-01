//! Yapay zekâ sunucuları (whisper.cpp ve llama.cpp) alt süreç olarak çalışır.
//! İkisi de Vulkan ile AMD / NVIDIA / Intel GPU'da aynı şekilde hızlanır;
//! GPU yoksa CPU'ya düşer.

use crate::settings::Settings;
use std::fs::File;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const WHISPER_PORT: u16 = 18080;
pub const LLAMA_PORT: u16 = 18081;
pub const VAD_FILE: &str = "ggml-silero-v5.1.2.bin";

#[derive(Clone)]
pub struct Paths {
    pub home: PathBuf,
}

/// Sunucular kendi klasörlerinde başlatılır (`current_dir`); göreli yol orada bozulur.
fn absolute(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

impl Paths {
    /// ECHOSUB_HOME, yoksa <exe klasörü>/runtime, yoksa ./runtime.
    pub fn resolve() -> Paths {
        if let Ok(h) = std::env::var("ECHOSUB_HOME") {
            return Paths { home: absolute(PathBuf::from(h)) };
        }
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("runtime")));
        match beside_exe {
            Some(p) if p.exists() => Paths { home: p },
            _ => Paths { home: absolute(PathBuf::from("runtime")) },
        }
    }

    fn exe(&self, sub: &str, name: &str) -> PathBuf {
        let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        self.home.join(sub).join(file)
    }
    pub fn model_dir(&self) -> PathBuf {
        self.home.join("models")
    }
    pub fn model(&self, file: &str) -> PathBuf {
        self.model_dir().join(file)
    }
    pub fn log_dir(&self) -> PathBuf {
        self.home.join("logs")
    }
}

/// Sunucunun çalışma klasörüne (`<home>/whisper`, `<home>/llama`) göre model yolu. whisper-server
/// Windows'ta ASCII dışı karakterli (Türkçe kullanıcı adı gibi) mutlak yolları açamıyor; bu yol hep ASCII.
fn model_arg(file: &str) -> PathBuf {
    Path::new("..").join("models").join(file)
}

/// Başka bir süreç (önceki bir EchoSub'dan kalan sunucu gibi) bu portu dinliyor mu?
fn port_busy(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
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

#[derive(Debug, PartialEq, Eq)]
pub enum Wait {
    Ready,
    /// Sunucu süreci hazır olmadan sonlandı (çöktü).
    Died,
    Timeout,
}

/// `url` 200 dönene kadar bekler. `alive` false dönerse süreç çökmüş demektir; zaman aşımını beklemeden çıkar.
pub fn wait_http(url: &str, timeout: Duration, mut alive: impl FnMut() -> bool) -> Wait {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if let Ok(r) = ureq::get(url).timeout(Duration::from_secs(2)).call() {
            if r.status() == 200 {
                return Wait::Ready;
            }
        }
        if !alive() {
            return Wait::Died;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    Wait::Timeout
}

impl Sidecars {
    pub fn whisper_alive(&mut self) -> bool {
        matches!(self.whisper.as_mut().map(|c| c.try_wait()), Some(Ok(None)))
    }
    pub fn llama_alive(&mut self) -> bool {
        matches!(self.llama.as_mut().map(|c| c.try_wait()), Some(Ok(None)))
    }

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
        if port_busy(WHISPER_PORT) {
            return Err(format!("{WHISPER_PORT} portu başka bir program tarafından kullanılıyor"));
        }
        let mut cmd = Command::new(&exe);
        lib_env(&mut cmd, exe.parent().unwrap());
        cmd.current_dir(exe.parent().unwrap())
            .arg("-m").arg(model_arg(wm))
            .args(["--host", "127.0.0.1", "--port", &WHISPER_PORT.to_string()])
            .arg("-nlp")
            .args(["-nth", "0.6"]);
        if paths.model(VAD_FILE).exists() {
            cmd.arg("-vm").arg(model_arg(VAD_FILE));
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
        if port_busy(LLAMA_PORT) {
            return Err(format!("{LLAMA_PORT} portu başka bir program tarafından kullanılıyor"));
        }
        let mut cmd = Command::new(&exe);
        lib_env(&mut cmd, exe.parent().unwrap());
        cmd.current_dir(exe.parent().unwrap())
            .arg("-m").arg(model_arg(lm))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_arg_stays_relative_and_points_to_the_models_folder() {
        let a = model_arg("m.bin");
        assert!(a.is_relative());
        assert_eq!(a, Path::new("..").join("models").join("m.bin"));
    }

    #[test]
    fn relative_home_becomes_absolute() {
        assert!(absolute(PathBuf::from("runtime")).is_absolute());
    }

    #[test]
    fn port_busy_sees_a_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(port_busy(port));
        drop(l);
        assert!(!port_busy(port));
    }

    #[test]
    fn wait_http_gives_up_when_the_process_dies() {
        // Dinlenmeyen port: HTTP asla yanıt vermez; süreç öldüğü an zaman aşımını beklemeden döner.
        let t0 = Instant::now();
        assert_eq!(wait_http("http://127.0.0.1:9/", Duration::from_secs(30), || false), Wait::Died);
        assert!(t0.elapsed() < Duration::from_secs(5));
    }
}
