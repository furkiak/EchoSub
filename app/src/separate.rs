//! Vokal ayırma (müzikten sesi ayır). UVR MDX-Net "Voc_FT" ONNX modeli.
//!
//! Model ve ayarlar: n_fft 7680, hop 1024, 3072 frekans bandı, 256 kare
//! (~5,9 sn @ 44,1 kHz), çıktı ×1,021 telafi. STFT/ISTFT burada (rustfft) yapılır;
//! sinir ağı ONNX Runtime ile çalışır: Windows'ta DirectML (AMD/NVIDIA/Intel GPU),
//! macOS'ta CoreML, diğerlerinde CPU.
//!
//! Girdi/çıktı: 16 kHz mono f32 (hattın kullandığı biçim). Model 44,1 kHz stereo
//! ister; 16 kHz'den yükseltilir (konuşma/vokal bandı < 8 kHz zaten korunur).

use ort::{ep, session::Session, value::Tensor};
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::path::Path;
use std::sync::Arc;

const N_FFT: usize = 7680;
const HOP: usize = 1024;
const DIM_F: usize = 3072;
const DIM_T: usize = 256;
const BINS: usize = N_FFT / 2 + 1;
const CHUNK: usize = HOP * (DIM_T - 1); // 261120 örnek
const TRIM: usize = N_FFT / 2;
const GEN: usize = CHUNK - 2 * TRIM; // her parçadan alınan faydalı örnek
const MODEL_SR: f64 = 44100.0;
const COMPENSATE: f32 = 1.021;

pub struct Separator {
    /// Çalıştığı donanım: "DirectML", "CoreML" ya da "CPU".
    pub provider: &'static str,
    session: Session,
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
}

fn resample_linear(x: &[f32], from: f64, to: f64) -> Vec<f32> {
    let n = (x.len() as f64 * to / from) as usize;
    let ratio = from / to;
    (0..n)
        .map(|i| {
            let p = i as f64 * ratio;
            let k = p as usize;
            let f = (p - k as f64) as f32;
            let a = x[k.min(x.len() - 1)];
            let b = x[(k + 1).min(x.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

impl Separator {
    /// Önce GPU hızlandırmayı dener (hata olursa sessizce düşmek yerine hata
    /// döndürtür), olmazsa CPU'ya düşer.
    pub fn load(model: &Path) -> Result<Self, String> {
        let mut last_err = String::from("model yüklenemedi");
        for gpu in [true, false] {
            #[allow(unused_mut)]
            let mut providers = Vec::new();
            #[allow(unused_mut)]
            let mut name = "CPU";
            if gpu {
                #[cfg(windows)]
                {
                    providers.push(ep::DirectML::default().build().error_on_failure());
                    name = "DirectML";
                }
                #[cfg(target_os = "macos")]
                {
                    providers.push(ep::CoreML::default().build().error_on_failure());
                    name = "CoreML";
                }
                if providers.is_empty() {
                    continue; // bu platformda GPU yolu yok
                }
            }
            match Self::try_load(model, providers, name) {
                Ok(s) => return Ok(s),
                Err(e) => last_err = e,
            }
        }
        Err(last_err)
    }

    fn try_load(
        model: &Path,
        providers: Vec<ep::ExecutionProviderDispatch>,
        name: &'static str,
    ) -> Result<Self, String> {
        let env = ort::init()
            .with_execution_providers(providers)
            .build()
            .map_err(|e| e.to_string())?;
        let session = Session::builder(&env)
            .map_err(|e| e.to_string())?
            // DirectML için önerilen ayarlar.
            .with_memory_pattern(false)
            .map_err(|e| e.to_string())?
            .with_parallel_execution(false)
            .map_err(|e| e.to_string())?
            .commit_from_file(model)
            .map_err(|e| e.to_string())?;
        let mut planner = FftPlanner::<f32>::new();
        let window = (0..N_FFT)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / N_FFT as f32).cos())
            .collect();
        Ok(Self {
            provider: name,
            session,
            fft: planner.plan_fft_forward(N_FFT),
            ifft: planner.plan_fft_inverse(N_FFT),
            window,
        })
    }

    /// torch.stft(center=True, reflect) eşdeğeri; çıktı [kare][bin].
    fn stft(&self, x: &[f32]) -> Vec<Vec<Complex<f32>>> {
        debug_assert_eq!(x.len(), CHUNK);
        let mut padded = Vec::with_capacity(CHUNK + N_FFT);
        for i in (1..=TRIM).rev() {
            padded.push(x[i]);
        }
        padded.extend_from_slice(x);
        for i in 0..TRIM {
            padded.push(x[CHUNK - 2 - i]);
        }
        let mut frames = Vec::with_capacity(DIM_T);
        let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
        for t in 0..DIM_T {
            let seg = &padded[t * HOP..t * HOP + N_FFT];
            for (n, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(seg[n] * self.window[n], 0.0);
            }
            self.fft.process(&mut buf);
            frames.push(buf[..BINS].to_vec());
        }
        frames
    }

    /// torch.istft(center=True) eşdeğeri; girdi [kare][bin], çıktı CHUNK örnek.
    fn istft(&self, spec: &[Vec<Complex<f32>>]) -> Vec<f32> {
        let total = CHUNK + N_FFT;
        let mut out = vec![0.0f32; total];
        let mut env = vec![0.0f32; total];
        let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
        for (t, frame) in spec.iter().enumerate() {
            for k in 0..BINS {
                buf[k] = frame[k];
            }
            buf[0].im = 0.0;
            buf[N_FFT / 2].im = 0.0;
            for k in 1..N_FFT / 2 {
                buf[N_FFT - k] = frame[k].conj();
            }
            self.ifft.process(&mut buf);
            let base = t * HOP;
            for n in 0..N_FFT {
                let w = self.window[n];
                out[base + n] += buf[n].re / N_FFT as f32 * w;
                env[base + n] += w * w;
            }
        }
        (TRIM..TRIM + CHUNK)
            .map(|i| out[i] / env[i].max(1e-11))
            .collect()
    }

    /// Bir parça (CHUNK örnek, stereo) için vokal.
    fn run_chunk(&mut self, l: &[f32], r: &[f32]) -> Result<(Vec<f32>, Vec<f32>), String> {
        let sl = self.stft(l);
        let sr = self.stft(r);
        // [1, 4, DIM_F, DIM_T]; kanal sırası: L_re, L_im, R_re, R_im
        let mut input = vec![0.0f32; 4 * DIM_F * DIM_T];
        for t in 0..DIM_T {
            for f in 0..DIM_F {
                input[(0 * DIM_F + f) * DIM_T + t] = sl[t][f].re;
                input[(1 * DIM_F + f) * DIM_T + t] = sl[t][f].im;
                input[(2 * DIM_F + f) * DIM_T + t] = sr[t][f].re;
                input[(3 * DIM_F + f) * DIM_T + t] = sr[t][f].im;
            }
        }
        let tensor = Tensor::from_array(([1usize, 4, DIM_F, DIM_T], input)).map_err(|e| e.to_string())?;
        // Çıktıyı sahipli kopyaya alıp oturum ödüncünü bırak.
        let (ol, or) = {
            let outputs = self
                .session
                .run(ort::inputs![tensor])
                .map_err(|e| e.to_string())?;
            let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            let mut ol = vec![vec![Complex::new(0.0f32, 0.0); BINS]; DIM_T];
            let mut or = ol.clone();
            for t in 0..DIM_T {
                for f in 0..DIM_F {
                    ol[t][f] = Complex::new(data[(0 * DIM_F + f) * DIM_T + t], data[(1 * DIM_F + f) * DIM_T + t]);
                    or[t][f] = Complex::new(data[(2 * DIM_F + f) * DIM_T + t], data[(3 * DIM_F + f) * DIM_T + t]);
                }
            }
            (ol, or)
        };
        Ok((self.istft(&ol), self.istft(&or)))
    }

    /// 16 kHz mono -> vokal, 16 kHz mono.
    pub fn vocals(&mut self, pcm16k: &[f32]) -> Result<Vec<f32>, String> {
        if pcm16k.is_empty() {
            return Ok(Vec::new());
        }
        let x = resample_linear(pcm16k, 16000.0, MODEL_SR);
        let n = x.len();
        let n_chunks = (n + GEN - 1) / GEN;
        // [TRIM sıfır][x][sıfır dolgu]
        let mut mix = vec![0.0f32; TRIM + n_chunks * GEN + TRIM];
        mix[TRIM..TRIM + n].copy_from_slice(&x);

        let mut voc: Vec<f32> = Vec::with_capacity(n_chunks * GEN);
        for i in 0..n_chunks {
            let chunk = &mix[i * GEN..i * GEN + CHUNK];
            let (l, r) = self.run_chunk(chunk, chunk)?;
            for k in TRIM..TRIM + GEN {
                voc.push((l[k] + r[k]) * 0.5 * COMPENSATE);
            }
        }
        voc.truncate(n);
        Ok(resample_linear(&voc, MODEL_SR, 16000.0))
    }
}

#[cfg(test)]
mod tests {
    use super::resample_linear;

    #[test]
    fn upsample_length_and_monotonic_ramp() {
        let x: Vec<f32> = (0..160).map(|i| i as f32).collect();
        let y = resample_linear(&x, 16_000.0, 44_100.0);
        assert_eq!(y.len(), (160.0 * 44_100.0 / 16_000.0) as usize);
        assert_eq!(y[0], 0.0);
        assert!(y.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn roundtrip_16k_44k_16k_is_close() {
        let x: Vec<f32> = (0..1600).map(|i| (i as f32 * 0.05).sin()).collect();
        let back = resample_linear(&resample_linear(&x, 16_000.0, 44_100.0), 44_100.0, 16_000.0);
        let n = back.len().min(x.len()) - 2;
        let err = (0..n).map(|i| (back[i] - x[i]).abs()).fold(0.0f32, f32::max);
        assert!(err < 0.02, "azami hata {err}");
    }
}
