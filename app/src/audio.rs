//! Sistem sesi yakalama (loopback). Çıkış cihazını girdi olarak açmak Windows'ta
//! WASAPI loopback'i, macOS'ta Core Audio tap'i (14.6+) açar. Linux'ta çıkışın
//! monitör kaynağı dinlenir (PulseAudio ya da PipeWire).
//! Çıktı: 16 kHz mono f32, `Shared` tamponuna eklenir.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::ErrorKind;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const TARGET_RATE: u32 = 16_000;
pub type Shared = Arc<Mutex<Vec<f32>>>;

struct Resampler {
    step: f64,
    pos: f64,
    prev: f32,
}

impl Resampler {
    fn new(from: u32) -> Self {
        Self { step: from as f64 / TARGET_RATE as f64, pos: 0.0, prev: 0.0 }
    }
    fn push(&mut self, x: f32, out: &mut Vec<f32>) {
        while self.pos < 1.0 {
            out.push(self.prev + (x - self.prev) * self.pos as f32);
            self.pos += self.step;
        }
        self.pos -= 1.0;
        self.prev = x;
    }
}

fn device_id(d: &cpal::Device) -> String {
    d.id().map(|i| format!("{i:?}")).unwrap_or_default()
}

/// Hoparlör sesini veren giriş aygıtı ve biçimi.
#[cfg(not(target_os = "linux"))]
fn capture_source(
    _host: &cpal::Host,
    output: &cpal::Device,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let cfg = output.default_output_config().map_err(|e| format!("cihaz ayarı okunamadı: {e}"))?;
    if cfg.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("desteklenmeyen örnek biçimi: {:?}", cfg.sample_format()));
    }
    Ok((output.clone(), cfg))
}

/// Hoparlör sesini veren giriş aygıtı ve biçimi. PulseAudio ve PipeWire her çıkış için
/// "<ad>.monitor" kaynağı sunar. Sunucu örneği f32'ye kendisi dönüştürür. Salt ALSA'da
/// karşılığı yok: çıkışı girdi olarak açmak mikrofonu dinlemek olurdu, bu yüzden açılmaz.
#[cfg(target_os = "linux")]
fn capture_source(
    host: &cpal::Host,
    output: &cpal::Device,
) -> Result<(cpal::Device, cpal::SupportedStreamConfig), String> {
    let name = |d: &cpal::Device| d.id().map(|i| i.id().to_string()).unwrap_or_default();
    let want = format!("{}.monitor", name(output));
    let source = host
        .devices()
        .map_err(|e| format!("aygıtlar okunamadı: {e}"))?
        .find(|d| name(d) == want)
        .ok_or_else(|| format!("{want} bulunamadı (PulseAudio ya da PipeWire gerekir)"))?;
    let cfg = source.default_input_config().map_err(|e| format!("cihaz ayarı okunamadı: {e}"))?;
    Ok((source, cfg))
}

pub fn spawn(shared: Shared, stop: Arc<AtomicBool>, log: impl Fn(&str) + Send + 'static) {
    std::thread::spawn(move || {
        // Aygıt yokken aynı satırla günlüğü doldurma.
        let mut last = String::new();
        let mut say = move |m: &str| {
            if m != last {
                log(m);
                last = m.to_string();
            }
        };
        while !stop.load(Ordering::Relaxed) {
            let host = cpal::default_host();
            let Some(output) = host.default_output_device() else {
                say("çıkış cihazı yok, bekleniyor");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            };
            let id = device_id(&output);
            let (device, cfg) = match capture_source(&host, &output) {
                Ok(c) => c,
                Err(e) => {
                    say(&e);
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let channels = cfg.channels() as usize;
            let mut rs = Resampler::new(cfg.sample_rate());
            let broken = Arc::new(AtomicBool::new(false));
            let broken_cb = Arc::clone(&broken);
            let shared_cb = Arc::clone(&shared);
            let mut scratch: Vec<f32> = Vec::with_capacity(4096);

            let stream = device.build_input_stream(
                cfg.config(),
                move |data: &[f32], _| {
                    scratch.clear();
                    for frame in data.chunks(channels) {
                        let mono = frame.iter().sum::<f32>() / channels as f32;
                        rs.push(mono, &mut scratch);
                    }
                    let mut b = shared_cb.lock().unwrap();
                    b.extend_from_slice(&scratch);
                    // Tüketici durursa belleğin şişmesini önle (en fazla 60 sn).
                    let max = TARGET_RATE as usize * 60;
                    if b.len() > max {
                        let cut = b.len() - max;
                        b.drain(..cut);
                    }
                },
                move |e| {
                    if matches!(
                        e.kind(),
                        ErrorKind::DeviceNotAvailable
                            | ErrorKind::DeviceChanged
                            | ErrorKind::StreamInvalidated
                    ) {
                        broken_cb.store(true, Ordering::Relaxed);
                    }
                },
                None,
            );
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    say(&format!("loopback açılamadı: {e}"));
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            if let Err(e) = stream.play() {
                say(&format!("akış başlatılamadı: {e}"));
                std::thread::sleep(Duration::from_secs(3));
                continue;
            }
            say(&format!("loopback açık: {} Hz, {} kanal", cfg.sample_rate(), channels));

            // Varsayılan cihaz değişirse ya da akış bozulursa yeniden aç.
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                if stop.load(Ordering::Relaxed) || broken.load(Ordering::Relaxed) {
                    break;
                }
                let now = host.default_output_device().map(|d| device_id(&d)).unwrap_or_default();
                if now != id {
                    say("varsayılan çıkış cihazı değişti, yeniden açılıyor");
                    break;
                }
            }
            drop(stream);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(from);
        let mut out = Vec::new();
        for &x in input {
            r.push(x, &mut out);
        }
        out
    }

    #[test]
    fn output_length_follows_rate_ratio() {
        for from in [44_100u32, 48_000, 96_000, 16_000] {
            let n = run(from, &vec![0.0; from as usize]).len() as i64;
            assert!((n - TARGET_RATE as i64).abs() <= 2, "{from} Hz -> {n} örnek");
        }
    }

    #[test]
    fn dc_signal_is_preserved_after_warmup() {
        let out = run(48_000, &vec![0.25; 4800]);
        assert!(out[2..].iter().all(|&v| (v - 0.25).abs() < 1e-5));
    }

    #[test]
    fn same_rate_keeps_length() {
        let input: Vec<f32> = (0..50).map(|i| i as f32 / 50.0).collect();
        assert_eq!(run(TARGET_RATE, &input).len(), input.len());
    }
}
