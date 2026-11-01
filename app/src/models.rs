//! Yapay zekâ modellerinin uygulama içinden indirilmesi (ilk açılışta ve kalite değişince).
//! Yalnızca model dosyaları ağdan gelir; ses ve çeviri her zaman yerelde işlenir.

use crate::settings::Settings;
use crate::sidecar::{Paths, VAD_FILE};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Model {
    pub file: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    /// Yaklaşık boyut (MB); yalnızca kullanıcıya göstermek için.
    pub mb: u32,
}

const CATALOG: &[Model] = &[
    Model {
        file: "ggml-small-q5_1.bin",
        label: "Konuşma tanıma (küçük)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
        mb: 190,
    },
    Model {
        file: "ggml-large-v3-turbo-q5_0.bin",
        label: "Konuşma tanıma",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        mb: 574,
    },
    Model {
        file: "translategemma-4b-it-Q4_K_M.gguf",
        label: "Çeviri (4B)",
        url: "https://huggingface.co/bullerwins/translategemma-4b-it-GGUF/resolve/main/translategemma-4b-it-Q4_K_M.gguf",
        mb: 2490,
    },
    Model {
        file: "translategemma-12b-it-Q4_K_M.gguf",
        label: "Çeviri (12B, yüksek kalite)",
        url: "https://huggingface.co/bullerwins/translategemma-12b-it-GGUF/resolve/main/translategemma-12b-it-Q4_K_M.gguf",
        mb: 7300,
    },
    Model {
        file: VAD_FILE,
        label: "Müzik/gürültü filtresi (VAD)",
        url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
        mb: 1,
    },
    Model {
        file: "UVR-MDX-NET-Voc_FT.onnx",
        label: "Vokal ayırma (şarkılar için)",
        url: "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/UVR-MDX-NET-Voc_FT.onnx",
        mb: 67,
    },
];

fn find(file: &str) -> Option<Model> {
    CATALOG.iter().find(|m| m.file == file).copied()
}

/// Geçerli ayarlarla çalışmak için gereken tüm modeller.
pub fn required(s: &Settings) -> Vec<Model> {
    let (whisper, translate) = s.model_files();
    let mut v: Vec<Model> = [whisper, translate].iter().filter_map(|f| find(f)).collect();
    if s.use_vad {
        v.extend(find(VAD_FILE));
    }
    if s.use_separation {
        v.extend(find("UVR-MDX-NET-Voc_FT.onnx"));
    }
    v
}

pub fn is_present(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

pub fn missing(paths: &Paths, s: &Settings) -> Vec<Model> {
    required(s).into_iter().filter(|m| !is_present(&paths.model(m.file))).collect()
}

enum Fail {
    Cancelled,
    Other(String),
}

/// `url` adresini `dir/file` olarak indirir. Yarım kalan `.part` dosyasından devam eder,
/// bağlantı koparsa birkaç kez yeniden dener. `progress(inen, toplam)` bayt cinsindendir.
pub fn download_url(
    url: &str,
    file: &str,
    dir: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("klasör oluşturulamadı ({}): {e}", dir.display()))?;
    let dest = dir.join(file);
    let part = dir.join(format!("{file}.part"));
    let mut last = String::new();
    for attempt in 0..4u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(2 * attempt));
        }
        match fetch(url, &part, cancel, progress) {
            Ok(()) => {
                return fs::rename(&part, &dest).map_err(|e| format!("dosya yerine konamadı: {e}"));
            }
            Err(Fail::Cancelled) => return Err("iptal edildi".into()),
            Err(Fail::Other(e)) => last = e,
        }
    }
    Err(last)
}

pub fn download(m: &Model, dir: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(), String> {
    download_url(m.url, m.file, dir, cancel, progress)
}

fn fetch(url: &str, part: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<(), Fail> {
    let io = |e: std::io::Error| Fail::Other(format!("disk hatası: {e}"));
    let have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(30))
        .build();
    let mut req = agent.get(url);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(416, _)) => {
            // Elimizdeki parça sunucudaki dosyadan büyük ya da eşit: baştan başla.
            let _ = fs::remove_file(part);
            return Err(Fail::Other("kısmi dosya geçersizdi, yeniden başlanıyor".into()));
        }
        Err(e) => return Err(Fail::Other(format!("indirilemedi: {e}"))),
    };
    let (mut done, total, mut out) = if resp.status() == 206 {
        let total = resp
            .header("Content-Range")
            .and_then(|v| v.rsplit('/').next())
            .and_then(|t| t.trim().parse::<u64>().ok())
            .unwrap_or(0);
        (have, total, OpenOptions::new().append(true).open(part).map_err(io)?)
    } else {
        let total = resp.header("Content-Length").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
        (0, total, File::create(part).map_err(io)?)
    };
    progress(done, total);
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 256 * 1024];
    let mut last_emit = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Fail::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| Fail::Other(format!("bağlantı koptu: {e}")))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(io)?;
        done += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(150) {
            progress(done, total);
            last_emit = Instant::now();
        }
    }
    out.flush().map_err(io)?;
    progress(done, total.max(done));
    if total > 0 && done != total {
        return Err(Fail::Other(format!("eksik indi ({done}/{total} bayt)")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("echosub-models-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn payload(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    /// Küçük bir HTTP sunucusu: `Range: bytes=N-` isteklerini yanıtlar.
    /// Dönen listede sunucunun gördüğü Range başlıkları bulunur.
    fn serve(data: Vec<u8>, conns: usize) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/m.bin", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ranges);
        std::thread::spawn(move || {
            for _ in 0..conns {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut rd = BufReader::new(sock.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if rd.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                    }
                }
                let head = match range {
                    Some(s) if s < data.len() => {
                        seen.lock().unwrap().push(format!("{s}-"));
                        let n = data.len() - s;
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{}/{}\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n",
                            data.len() - 1,
                            data.len()
                        )
                    }
                    Some(_) => "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                    None => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len()),
                };
                let body = &data[range.filter(|s| *s < data.len()).unwrap_or(0)..];
                let _ = sock.write_all(head.as_bytes());
                if !head.starts_with("HTTP/1.1 416") {
                    let _ = sock.write_all(body);
                }
            }
        });
        (url, ranges)
    }

    #[test]
    fn required_follows_quality_and_toggles() {
        let mut s = Settings { quality: "fast".into(), ..Settings::default() };
        let files = |s: &Settings| required(s).iter().map(|m| m.file).collect::<Vec<_>>();
        assert_eq!(
            files(&s),
            ["ggml-small-q5_1.bin", "translategemma-4b-it-Q4_K_M.gguf", "ggml-silero-v5.1.2.bin", "UVR-MDX-NET-Voc_FT.onnx"]
        );
        s.use_vad = false;
        s.use_separation = false;
        s.quality = "quality".into();
        assert_eq!(files(&s), ["ggml-large-v3-turbo-q5_0.bin", "translategemma-12b-it-Q4_K_M.gguf"]);
    }

    #[test]
    fn every_model_the_settings_can_ask_for_is_in_the_catalog() {
        for q in ["fast", "balanced", "quality"] {
            let s = Settings { quality: q.into(), ..Settings::default() };
            let (w, t) = s.model_files();
            assert!(find(w).is_some() && find(t).is_some(), "{q}");
        }
        assert!(CATALOG.iter().all(|m| m.url.starts_with("https://") && m.mb > 0));
    }

    #[test]
    fn missing_shrinks_as_files_appear() {
        let home = tmp("missing");
        let paths = Paths { home: home.clone() };
        let s = Settings::default();
        assert_eq!(missing(&paths, &s).len(), required(&s).len());
        fs::create_dir_all(paths.model_dir()).unwrap();
        fs::write(paths.model("ggml-silero-v5.1.2.bin"), b"x").unwrap();
        fs::write(paths.model("UVR-MDX-NET-Voc_FT.onnx"), b"").unwrap(); // boş dosya = yok sayılır
        assert_eq!(missing(&paths, &s).len(), required(&s).len() - 1);
    }

    #[test]
    fn downloads_a_file_and_leaves_no_part_file() {
        let data = payload(700_000);
        let (url, _) = serve(data.clone(), 1);
        let dir = tmp("full");
        let mut last = (0, 0);
        download_url(&url, "m.bin", &dir, &AtomicBool::new(false), &mut |d, t| last = (d, t)).unwrap();
        assert_eq!(fs::read(dir.join("m.bin")).unwrap(), data);
        assert!(!dir.join("m.bin.part").exists());
        assert_eq!(last, (data.len() as u64, data.len() as u64));
    }

    #[test]
    fn resumes_from_a_partial_file() {
        let data = payload(500_000);
        let (url, ranges) = serve(data.clone(), 1);
        let dir = tmp("resume");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("m.bin.part"), &data[..123_456]).unwrap();
        download_url(&url, "m.bin", &dir, &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("m.bin")).unwrap(), data);
        assert_eq!(*ranges.lock().unwrap(), vec!["123456-".to_string()]);
    }

    #[test]
    fn cancel_stops_and_keeps_the_partial_file() {
        let (url, _) = serve(payload(2_000_000), 1);
        let dir = tmp("cancel");
        let err = download_url(&url, "m.bin", &dir, &AtomicBool::new(true), &mut |_, _| {}).unwrap_err();
        assert_eq!(err, "iptal edildi");
        assert!(!dir.join("m.bin").exists());
    }
}
